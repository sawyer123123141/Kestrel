use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData, TrapCode, Value};
use cranelift_module::Module;
use kestrel_kir::{self as kir, IndexCheck, KTy, Place, Proj};
use kestrel_runtime::PANIC_BOUNDS;

use super::{Check, FuncLowerer, Slot};

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// The type of the value stored at a place.
    pub(crate) fn place_ty(&self, p: &Place) -> KTy {
        let kir = self.shared.kir;
        let mut ty = self.f.locals[p.root.0 as usize].ty;
        for proj in &p.path {
            ty = match (proj, ty) {
                (Proj::Field(fi), KTy::Struct(s)) => kir.layout(s).fields[*fi as usize].ty,
                (Proj::Variant(v, fi), KTy::Enum(e)) => kir.enum_layout(e).variants[*v as usize].fields[*fi as usize].ty,
                (Proj::Index { .. }, KTy::Array(el) | KTy::List(el)) => kir.elem(el),
                (Proj::Payload, KTy::Optional(el)) => kir.elem(el),
                (Proj::MapKey { .. }, KTy::Map(k, _)) => kir.elem(k),
                (Proj::MapValue { .. }, KTy::Map(_, v)) => kir.elem(v),
                (Proj::Lane(_), KTy::F32X4 | KTy::I32X4) => ty.lane(),
                _ => unreachable!("place path doesn't match the type"),
            };
        }
        ty
    }

    /// Address of a place. Locals kept in registers have none. Each index
    /// loads its collection's pointer and is bounds-checked.
    pub(crate) fn place_addr(&mut self, p: &Place) -> Result<Value, String> {
        let kir = self.shared.kir;
        let mut addr = match self.slots[p.root.0 as usize] {
            Slot::Stack(ss) => self.b.ins().stack_addr(self.ptr, ss, 0),
            Slot::Ptr(v) => self.b.use_var(v),
            Slot::Var(_) => unreachable!("KIR puts every addressed local in memory"),
            Slot::Unused => unreachable!("nothing refers to an unused local"),
        };
        let mut ty = self.f.locals[p.root.0 as usize].ty;
        let mut offset = 0;
        // The fields from the root so far, while the path has only fields: the list an index
        // reads an item of, whose header a loop may keep in variables (see `headers`).
        let mut fields: Option<Vec<u32>> = (!self.hdr_cache.is_empty()).then(Vec::new);
        for proj in &p.path {
            match proj {
                Proj::Field(fi) => {
                    if let Some(fs) = &mut fields {
                        fs.push(*fi);
                    }
                }
                Proj::Index { .. } => {}
                _ => fields = None,
            }
            let field = match (proj, ty) {
                (Proj::Field(fi), KTy::Struct(s)) => &kir.layout(s).fields[*fi as usize],
                (Proj::Variant(v, fi), KTy::Enum(e)) => &kir.enum_layout(e).variants[*v as usize].fields[*fi as usize],
                (Proj::Payload, KTy::Optional(el)) => {
                    offset += kir.payload_offset(el) as i32;
                    ty = kir.elem(el);
                    continue;
                }
                (Proj::Index { at, check }, KTy::Array(el) | KTy::List(el)) => {
                    let cached = fields.take().and_then(|fs| self.cached(p.root, &fs));
                    let header = self.offset(addr, offset);
                    ty = kir.elem(el);
                    addr = self.element_addr(header, at, *check, ty, cached)?;
                    offset = 0;
                    continue;
                }
                (Proj::MapKey { at }, KTy::Map(k, _)) => {
                    let header = self.offset(addr, offset);
                    addr = self.entry_part(header, ty, at, false, false)?;
                    (ty, offset) = (kir.elem(k), 0);
                    continue;
                }
                (Proj::MapValue { at, checked }, KTy::Map(_, v)) => {
                    let header = self.offset(addr, offset);
                    addr = self.entry_part(header, ty, at, true, *checked)?;
                    (ty, offset) = (kir.elem(v), 0);
                    continue;
                }
                // A lane: 4 bytes at `4 * lane` (always the place's last step).
                (Proj::Lane(i), KTy::F32X4 | KTy::I32X4) => {
                    offset += 4 * i32::from(*i);
                    ty = ty.lane();
                    continue;
                }
                _ => unreachable!("place path doesn't match the type"),
            };
            offset += field.offset as i32;
            ty = field.ty;
        }
        Ok(self.offset(addr, offset))
    }

    pub(crate) fn offset(&mut self, addr: Value, offset: i32) -> Value {
        if offset == 0 {
            addr
        } else {
            self.b.ins().iadd_imm_s(addr, offset as i64)
        }
    }

    /// The address of element `index` of the array or list whose header is at `header`, or, while
    /// a loop keeps that header in variables, in the cache entry `cached` (read after the index,
    /// which may add to the list).
    fn element_addr(&mut self, header: Value, index: &kir::Expr, check: IndexCheck, elem: KTy, cached: Option<usize>) -> Result<Value, String> {
        let i = self.expr(index)?;
        let i = self.widen_i64(i, index.ty);
        match check {
            IndexCheck::Checked => self.bounds_check(header, i, index.line, Check::Emit, cached),
            IndexCheck::Known => {}
            IndexCheck::Proven => {
                let mode = self.proven_check();
                if mode != Check::Skip {
                    self.bounds_check(header, i, index.line, mode, cached);
                }
            }
        }
        let ptr = match cached {
            Some(c) => self.cached_header(c).0,
            None => self.b.ins().load(self.ptr, MemFlagsData::trusted(), header, 0),
        };
        let (size, _) = self.shared.kir.size_align(elem);
        let at = self.b.ins().imul_imm_s(i, size as i64);
        Ok(self.b.ins().iadd(ptr, at))
    }

    /// Panics unless `0 <= i < len` for the collection whose header is at `header`; with
    /// `Verify`, fails as a compiler bug instead.
    fn bounds_check(&mut self, header: Value, i: Value, line: u32, check: Check, cached: Option<usize>) {
        let len = match cached {
            Some(c) => self.cached_header(c).1,
            None => self.b.ins().load(types::I64, MemFlagsData::trusted(), header, 8),
        };
        // Unsigned: a negative index looks huge, so one compare checks both ends.
        let bad = self.b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, i, len);
        let (fail, ok) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(bad, fail, &[], ok, &[]);
        self.b.switch_to_block(fail);
        self.b.set_cold_block(fail);
        self.call_failure(check, PANIC_BOUNDS, line, Some((i, len)));
        self.b.ins().trap(TrapCode::unwrap_user(2));
        self.b.switch_to_block(ok);
    }

    /// An integer of type `ty` as an i64 (lengths and indexes are i64).
    pub(crate) fn widen_i64(&mut self, v: Value, ty: KTy) -> Value {
        match ty {
            KTy::I64 => v,
            KTy::U8 => self.b.ins().uextend(types::I64, v),
            _ => self.b.ins().sextend(types::I64, v),
        }
    }

    /// Loads the number or vector stored at a place (a lane of a vector in a register included).
    pub(crate) fn load_place(&mut self, p: &Place) -> Result<Value, String> {
        if let Slot::Var(v) = self.slots[p.root.0 as usize] {
            let whole = self.b.use_var(v);
            return Ok(match p.path.as_slice() {
                [] => whole,
                [Proj::Lane(i)] => self.b.ins().extractlane(whole, *i),
                _ => unreachable!("a register local has no fields"),
            });
        }
        let ty = self.place_ty(p);
        let addr = self.place_addr(p)?;
        Ok(self.load_at(ty, addr, 0))
    }

    pub(crate) fn store_place(&mut self, p: &Place, val: Value) -> Result<(), String> {
        if let Slot::Var(v) = self.slots[p.root.0 as usize] {
            let val = match p.path.as_slice() {
                [] => val,
                [Proj::Lane(i)] => {
                    let whole = self.b.use_var(v);
                    self.b.ins().insertlane(whole, val, *i)
                }
                _ => unreachable!("a register local has no fields"),
            };
            self.b.def_var(v, val);
            return Ok(());
        }
        let addr = self.place_addr(p)?;
        self.store_value(val, addr, 0);
        Ok(())
    }

    /// Copies a value of aggregate type `ty` from `src` to `dst`, byte for byte.
    pub(crate) fn copy_aggregate(&mut self, dst: Value, src: Value, ty: KTy) {
        self.copy_fields(dst, src, 0, ty);
    }

    /// Copies a value of aggregate type `ty` from one place's memory to another's, as
    /// `copy_aggregate` does, except that a plain struct whose leaves are all 4-byte numbers and
    /// whose size is a multiple of 8 moves in 8-byte integer loads and stores: a bit copy, without
    /// the `aligned` flag (such a struct is 4-aligned). Only between places, whose memory wasn't
    /// just written field by field: an 8-byte load of two 4-byte stores can't take its value from
    /// the store buffer and waits for them, where 4-byte loads wouldn't.
    fn copy_place_value(&mut self, dst: Value, src: Value, ty: KTy) {
        let kir = self.shared.kir;
        let (size, _) = kir.size_align(ty);
        if size % 8 != 0 || !four_byte_leaves(kir, ty) {
            return self.copy_aggregate(dst, src, ty);
        }
        let flags = MemFlagsData::new().with_notrap();
        for at in (0..size as i32).step_by(8) {
            let v = self.b.ins().load(types::I64, flags, src, at);
            self.b.ins().store(flags, v, dst, at);
        }
    }

    /// Structs field by field; everything else as raw bytes (which enum
    /// variant's fields are live is only known at run time).
    fn copy_fields(&mut self, dst: Value, src: Value, offset: i32, ty: KTy) {
        let kir = self.shared.kir;
        if let KTy::Struct(s) = ty {
            for f in &kir.layout(s).fields {
                let at = offset + f.offset as i32;
                if f.ty.is_aggregate() {
                    self.copy_fields(dst, src, at, f.ty);
                } else {
                    let v = self.load_at(f.ty, src, at);
                    self.store_value(v, dst, at);
                }
            }
            return;
        }
        assert!(ty.is_aggregate(), "copying a non-aggregate");
        let (size, align) = kir.size_align(ty);
        let (chunk, chunk_ty) = match align {
            8.. => (8, types::I64),
            4..=7 => (4, types::I32),
            2..=3 => (2, types::I16),
            _ => (1, types::I8),
        };
        for at in (0..size as i32).step_by(chunk) {
            let v = self.b.ins().load(chunk_ty, MemFlagsData::trusted(), src, offset + at);
            self.b.ins().store(MemFlagsData::trusted(), v, dst, offset + at);
        }
    }

    /// Whether `e` is made only of constants and locals kept in registers.
    fn reads_only_locals(&self, e: &kir::Expr) -> bool {
        match &e.kind {
            kir::ExprKind::StructLit(_, fields) => fields.iter().all(|f| self.reads_only_locals(f)),
            kir::ExprKind::Int(_) | kir::ExprKind::Float(_) | kir::ExprKind::Bool(_) => true,
            kir::ExprKind::Load(q) => q.path.is_empty() && matches!(self.slots[q.root.0 as usize], Slot::Var(_)),
            _ => false,
        }
    }

    /// The scalars of `e`, a `StructLit` that `reads_only_locals` (nested ones included), read in
    /// field order as `(offset from the struct's start, value)`.
    fn read_leaves(&mut self, e: &kir::Expr, base: i32, out: &mut Vec<(i32, Value)>) -> Result<(), String> {
        match &e.kind {
            kir::ExprKind::StructLit(s, fields) => {
                let layouts = self.shared.kir;
                for (f, layout) in fields.iter().zip(&layouts.layout(*s).fields) {
                    self.read_leaves(f, base + layout.offset as i32, out)?;
                }
            }
            _ => {
                let v = self.expr(e)?;
                out.push((base, v));
            }
        }
        Ok(())
    }

    pub(crate) fn assign(&mut self, p: &Place, e: &kir::Expr) -> Result<(), String> {
        if !e.ty.is_aggregate() {
            let v = self.expr(e)?;
            return self.store_place(p, v);
        }
        let src = match &e.kind {
            // Two places of one type are the same memory or don't overlap.
            kir::ExprKind::Load(from) => {
                let src = self.place_addr(from)?;
                let dst = self.place_addr(p)?;
                self.copy_place_value(dst, src, e.ty);
                return Ok(());
            }
            // Built only from locals and constants, so it can't read the place it replaces, and
            // needs no temporary: the values are read first, then the place is found once (its
            // index expressions can change a local the value reads) and each value is stored.
            kir::ExprKind::StructLit(..) if self.reads_only_locals(e) => {
                let mut leaves = Vec::new();
                self.read_leaves(e, 0, &mut leaves)?;
                let dst = self.place_addr(p)?;
                for (offset, v) in leaves {
                    self.store_value(v, dst, offset);
                }
                return Ok(());
            }
            // Built in a temporary first: the new value may read the place it replaces.
            _ => {
                let tmp = self.temp_addr(e.ty);
                self.lower_into(e, tmp)?;
                tmp
            }
        };
        let dst = self.place_addr(p)?;
        self.copy_aggregate(dst, src, e.ty);
        Ok(())
    }
}

/// Whether every leaf of `ty`, a struct, is a 4-byte number (`int` or `float`, a `vec2`'s lanes).
fn four_byte_leaves(kir: &kir::Module, ty: KTy) -> bool {
    match ty {
        KTy::Struct(s) => kir.layout(s).fields.iter().all(|f| match f.ty {
            KTy::I32 | KTy::F32 => true,
            KTy::Struct(_) => four_byte_leaves(kir, f.ty),
            _ => false,
        }),
        _ => false,
    }
}
