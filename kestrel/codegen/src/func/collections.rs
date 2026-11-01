//! Arrays (`{ptr, len}`) and lists (`{ptr, len, cap, arena}`): creation,
//! appending and copying. Memory comes from the runtime's arenas. A string
//! (`{ptr, len}`) copies like an array of bytes.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData, Value};
use cranelift_module::Module;
use kestrel_kir::{self as kir, KTy, Place};

use super::FuncLowerer;

pub(super) const PTR: i32 = 0;
pub(super) const LEN: i32 = 8;
pub(super) const CAP: i32 = 16;
const ARENA: i32 = 24;

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    pub(super) fn elem_size_align(&self, ty: KTy) -> (i64, i64) {
        // A string's elements are its bytes.
        if ty == KTy::Str {
            return (1, 1);
        }
        let (KTy::Array(el) | KTy::List(el)) = ty else { unreachable!("{ty:?} has no elements") };
        let kir = self.shared.kir;
        let (size, align) = kir.size_align(kir.elem(el));
        (size as i64, align as i64)
    }

    pub(super) fn store(&mut self, v: Value, addr: Value, offset: i32) {
        self.store_value(v, addr, offset);
    }

    /// A list's extra fields: its capacity and the current arena, where it will grow.
    fn finish_list(&mut self, dest: Value, cap: Value) {
        self.store(cap, dest, CAP);
        let arena = self.call_id(self.shared.rt.current_arena, &[]).expect("returns the arena");
        self.store(arena, dest, ARENA);
    }

    /// `array[T](n)`: `n` zeroed elements.
    pub(crate) fn array_new(&mut self, len: &kir::Expr, ty: KTy, line: u32, dest: Value) -> Result<(), String> {
        let (size, align) = self.elem_size_align(ty);
        let n = self.expr(len)?;
        let n = self.widen_i64(n, len.ty);
        let (s, a) = (self.b.ins().iconst(types::I64, size), self.b.ins().iconst(types::I64, align));
        let (l, f) = self.loc_args(line);
        let p = self.call_id(self.shared.rt.alloc, &[n, s, a, l, f]).expect("returns a pointer");
        self.store(p, dest, PTR);
        self.store(n, dest, LEN);
        Ok(())
    }

    /// `[a, b, c]` as an array, or as a list whose capacity is its length.
    pub(crate) fn array_lit(&mut self, items: &[kir::Expr], ty: KTy, line: u32, dest: Value) -> Result<(), String> {
        let (size, align) = self.elem_size_align(ty);
        let n = self.b.ins().iconst(types::I64, items.len() as i64);
        let p = if items.is_empty() {
            self.b.ins().iconst(self.ptr, 0)
        } else {
            let (s, a) = (self.b.ins().iconst(types::I64, size), self.b.ins().iconst(types::I64, align));
            let (l, f) = self.loc_args(line);
            self.call_id(self.shared.rt.alloc, &[n, s, a, l, f]).expect("returns a pointer")
        };
        for (k, item) in items.iter().enumerate() {
            self.store_at(item, p, (k as i64 * size) as i32)?;
        }
        self.store(p, dest, PTR);
        self.store(n, dest, LEN);
        if let KTy::List(_) = ty {
            self.finish_list(dest, n);
        }
        Ok(())
    }

    /// `list[T]()`
    pub(crate) fn list_new(&mut self, dest: Value) {
        let null = self.b.ins().iconst(self.ptr, 0);
        let zero = self.b.ins().iconst(types::I64, 0);
        self.store(null, dest, PTR);
        self.store(zero, dest, LEN);
        self.finish_list(dest, zero);
    }

    pub(crate) fn len(&mut self, p: &Place) -> Result<Value, String> {
        // A loop that keeps the list's header in variables (see `headers`).
        if let Some(c) = self.cached_place(p) {
            return Ok(self.cached_header(c).1);
        }
        let addr = self.place_addr(p)?;
        Ok(self.b.ins().load(types::I64, MemFlagsData::trusted(), addr, LEN))
    }

    /// The room of the list (its capacity) or array (its length) at `p`.
    pub(crate) fn cap(&mut self, p: &Place) -> Result<Value, String> {
        let at = match self.place_ty(p) {
            KTy::List(_) => CAP,
            KTy::Array(_) => LEN,
            other => return Err(format!("`Cap` of a {other:?}, which has no buffer of items")),
        };
        let addr = self.place_addr(p)?;
        Ok(self.b.ins().load(types::I64, MemFlagsData::trusted(), addr, at))
    }

    /// `CopyItems(dst, src)`: the source's items into the target's buffer, which has room (see
    /// `kestrel_rt_copy_items`). The source is found first, then the target.
    pub(crate) fn copy_items(&mut self, dst: &Place, src: &Place) -> Result<(), String> {
        let ty = self.place_ty(dst);
        if !matches!(ty, KTy::List(_) | KTy::Array(_)) || self.place_ty(src) != ty {
            return Err(format!("`CopyItems` from a {:?} into a {ty:?}", self.place_ty(src)));
        }
        let (size, _) = self.elem_size_align(ty);
        let from = self.place_addr(src)?;
        let ptr = self.b.ins().load(self.ptr, MemFlagsData::trusted(), from, PTR);
        let len = self.b.ins().load(types::I64, MemFlagsData::trusted(), from, LEN);
        let to = self.place_addr(dst)?;
        let size = self.b.ins().iconst(types::I64, size);
        self.call_id(self.shared.rt.copy_items, &[to, ptr, len, size]);
        Ok(())
    }

    /// `xs.add(v)`. `v` is evaluated first (it may read `xs`), then the
    /// list grows if it's full. Range analysis relies on `v` running before
    /// the place is found (`kir::range` `change_len`): keep that order.
    pub(crate) fn list_add(&mut self, list: &Place, value: &kir::Expr) -> Result<(), String> {
        let ty = self.place_ty(list);
        let (size, align) = self.elem_size_align(ty);
        let v = if value.ty.is_aggregate() {
            let tmp = self.temp_addr(value.ty);
            self.lower_into(value, tmp)?;
            tmp
        } else {
            self.expr(value)?
        };
        let header = self.place_addr(list)?;
        if let Some(c) = self.cached_place(list) {
            // A loop that adds to a list always keeps its capacity too (see `headers`).
            debug_assert!(self.add_cached(c), "a list a loop adds to is cached without its capacity");
            self.list_add_cached(c, header, v, value.ty, size, align);
            return Ok(());
        }
        let len = self.b.ins().load(types::I64, MemFlagsData::trusted(), header, LEN);
        let cap = self.b.ins().load(types::I64, MemFlagsData::trusted(), header, CAP);
        let full = self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, len, cap);
        let (grow, append) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(full, grow, &[], append, &[]);
        self.b.switch_to_block(grow);
        self.b.set_cold_block(grow);
        let (s, a) = (self.b.ins().iconst(types::I64, size), self.b.ins().iconst(types::I64, align));
        self.call_id(self.shared.rt.list_grow, &[header, s, a]);
        self.b.ins().jump(append, &[]);
        self.b.switch_to_block(append);
        let ptr = self.b.ins().load(self.ptr, MemFlagsData::trusted(), header, PTR);
        let at = self.b.ins().imul_imm_s(len, size);
        let slot = self.b.ins().iadd(ptr, at);
        if value.ty.is_aggregate() {
            self.copy_aggregate(slot, v, value.ty);
        } else {
            self.store(v, slot, 0);
        }
        let len = self.b.ins().iadd_imm_s(len, 1);
        self.store(len, header, LEN);
        Ok(())
    }

    /// Gives the collection (or string) at `p` its own copy of its elements.
    pub(crate) fn dup(&mut self, p: &Place) -> Result<(), String> {
        let ty = self.place_ty(p);
        if let KTy::Map(..) = ty {
            return self.map_dup(p);
        }
        let (size, align) = self.elem_size_align(ty);
        let header = self.place_addr(p)?;
        let ptr = self.b.ins().load(self.ptr, MemFlagsData::trusted(), header, PTR);
        let len = self.b.ins().load(types::I64, MemFlagsData::trusted(), header, LEN);
        let bytes = self.b.ins().imul_imm_s(len, size);
        let a = self.b.ins().iconst(types::I64, align);
        let copy = self.call_id(self.shared.rt.dup, &[ptr, bytes, a]).expect("returns a pointer");
        self.store(copy, header, PTR);
        if let KTy::List(_) = ty {
            self.finish_list(header, len);
        }
        Ok(())
    }
}
