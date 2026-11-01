//! List headers kept in Cranelift variables across a loop that can't change them (spec decision 3
//! of the performance round, `docs/superpowers/specs/2026-11-01-perf-round-design.md`).
//!
//! At a loop's entry, each list or array place the loop reads and can't change (see `scan` for the
//! rule) has its `data` and `len` loaded once into variables; inside the loop an item's address
//! (`P[i]`, a `for ... in P` item pointer), its bounds check and `P.len` use them. A list the loop
//! only reads and adds to keeps its `cap` too: `add` compares the variables, stores the new length
//! to memory at once (write-through, so any other reader of the header sees it), and on the grow
//! path calls the runtime and reloads `data` and `cap`. Code after the loop reads memory again.

mod scan;

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData, Value};
use cranelift_frontend::Variable;
use cranelift_module::Module;
use kestrel_kir::{self as kir, KTy, LocalIdx, Place, Proj};

use super::collections::{CAP, LEN, PTR};
use super::{FuncLowerer, Slot};
use scan::{bound_before, fields_only, Binds, Scan, ADD_MAX};

/// A list place whose header a loop keeps in variables: the root local, the fields from it to the
/// list, and the variables holding the header's `data`, `len` and (for a list the loop adds to)
/// `cap`.
pub(crate) struct Cached {
    pub root: LocalIdx,
    pub fields: Vec<u32>,
    pub data: Variable,
    pub len: Variable,
    pub cap: Option<Variable>,
}

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// Before a loop with this condition (`None` for a `for` range) and body is lowered: loads the
    /// header of each list place the loop can't change into variables, pushes it on `hdr_cache`,
    /// and returns how many it pushed, for `uncache` after the loop.
    pub(super) fn cache_headers(&mut self, cond: Option<&kir::Expr>, body: &[kir::Stmt]) -> usize {
        let scan = Scan::new(cond, body);
        if scan.opaque || scan.candidates.is_empty() {
            return 0;
        }
        // Only a reference local (not a parameter) is ever bound, so without one the function's
        // bindings needn't be looked for.
        let bound_ref = |l: LocalIdx| matches!(self.slots[l.0 as usize], Slot::Ptr(_)) && !self.f.params.contains(&l);
        let binds = if scan.roots().any(bound_ref) { Binds::new(&self.f.body) } else { Binds::default() };
        // The reference locals bound before the loop on every way to it, found only when needed.
        let mut before: Option<Vec<LocalIdx>> = None;
        let mut keep = Vec::new();
        for c in &scan.candidates {
            if self.cached(c.root, &c.fields).is_some() || scan.changes(c, &binds) {
                continue;
            }
            match self.slots[c.root.0 as usize] {
                Slot::Stack(_) => {}
                Slot::Ptr(_) if self.f.params.contains(&c.root) => {}
                // A reference is read here, before the loop: only when it is bound by then.
                Slot::Ptr(_) => {
                    let ready = before.get_or_insert_with(|| {
                        let mut acc = Vec::new();
                        let found = !body.is_empty() && bound_before(&self.f.body, body, &mut acc);
                        if found {
                            acc
                        } else {
                            Vec::new()
                        }
                    });
                    if !ready.contains(&c.root) {
                        continue;
                    }
                }
                Slot::Var(_) | Slot::Unused => continue,
            }
            let place = Place { root: c.root, path: c.fields.iter().map(|&f| Proj::Field(f)).collect() };
            match self.place_ty(&place) {
                KTy::List(_) => {}
                KTy::Array(_) if !c.add => {}
                _ => continue,
            }
            keep.push((place, c.add));
        }
        // Too many added-to lists: cache only the ones the loop reads.
        if keep.iter().filter(|(_, add)| *add).count() > ADD_MAX {
            keep.retain(|(_, add)| !add);
        }
        let n = keep.len();
        for (place, add) in keep {
            self.cache_one(place, add);
        }
        n
    }

    /// Loads the header of `place` (a field-only place, which can't fail) into new variables.
    fn cache_one(&mut self, place: Place, add: bool) {
        let header = self.place_addr(&place).expect("a place of fields only has an address");
        let flags = MemFlagsData::trusted();
        let (data, len) = (self.b.declare_var(self.ptr), self.b.declare_var(types::I64));
        let v = self.b.ins().load(self.ptr, flags, header, PTR);
        self.b.def_var(data, v);
        let v = self.b.ins().load(types::I64, flags, header, LEN);
        self.b.def_var(len, v);
        let cap = add.then(|| {
            let cap = self.b.declare_var(types::I64);
            let v = self.b.ins().load(types::I64, flags, header, CAP);
            self.b.def_var(cap, v);
            cap
        });
        let fields = fields_only(&place.path).expect("fields only");
        self.hdr_cache.push(Cached { root: place.root, fields, data, len, cap });
    }

    /// After the loop: drops the `n` places `cache_headers` pushed for it.
    pub(super) fn uncache(&mut self, n: usize) {
        let keep = self.hdr_cache.len() - n;
        self.hdr_cache.truncate(keep);
    }

    /// The cache entry of the list at `root` and then `fields`, while a loop keeps it.
    pub(super) fn cached(&self, root: LocalIdx, fields: &[u32]) -> Option<usize> {
        self.hdr_cache.iter().position(|c| c.root == root && c.fields == fields)
    }

    /// The cache entry of a field-only list place.
    pub(super) fn cached_place(&self, p: &Place) -> Option<usize> {
        if self.hdr_cache.is_empty() {
            return None;
        }
        self.cached(p.root, &fields_only(&p.path)?)
    }

    /// The `data` and `len` of the cached list `c`, as they are here.
    pub(super) fn cached_header(&mut self, c: usize) -> (Value, Value) {
        let (data, len) = (self.hdr_cache[c].data, self.hdr_cache[c].len);
        (self.b.use_var(data), self.b.use_var(len))
    }

    /// Whether the cached list `c` keeps its capacity: the loop adds to it.
    pub(super) fn add_cached(&self, c: usize) -> bool {
        self.hdr_cache[c].cap.is_some()
    }

    /// Appends `v` (a scalar, or the address of an aggregate of type `ty`) to the add-cached list
    /// `c` whose header is at `header`: grows it when `len` reaches `cap` (reloading `data` and
    /// `cap`), stores the item, and stores the new length to memory at once.
    pub(super) fn list_add_cached(&mut self, c: usize, header: Value, v: Value, ty: KTy, size: i64, align: i64) {
        let (data, len_var) = (self.hdr_cache[c].data, self.hdr_cache[c].len);
        let cap = self.hdr_cache[c].cap.expect("an add-cached list keeps its capacity");
        let flags = MemFlagsData::trusted();
        let len = self.b.use_var(len_var);
        let room = self.b.use_var(cap);
        let full = self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, len, room);
        let (grow, append) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(full, grow, &[], append, &[]);
        self.b.switch_to_block(grow);
        self.b.set_cold_block(grow);
        let (s, a) = (self.b.ins().iconst(types::I64, size), self.b.ins().iconst(types::I64, align));
        self.call_id(self.shared.rt.list_grow, &[header, s, a]);
        let new_data = self.b.ins().load(self.ptr, flags, header, PTR);
        self.b.def_var(data, new_data);
        let new_cap = self.b.ins().load(types::I64, flags, header, CAP);
        self.b.def_var(cap, new_cap);
        self.b.ins().jump(append, &[]);
        self.b.switch_to_block(append);
        let ptr = self.b.use_var(data);
        let at = self.b.ins().imul_imm_s(len, size);
        let slot = self.b.ins().iadd(ptr, at);
        if ty.is_aggregate() {
            self.copy_aggregate(slot, v, ty);
        } else {
            self.store(v, slot, 0);
        }
        let len = self.b.ins().iadd_imm_s(len, 1);
        self.store(len, header, LEN);
        self.b.def_var(len_var, len);
    }
}
