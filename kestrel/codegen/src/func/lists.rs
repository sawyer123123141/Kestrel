//! Removing, inserting and clearing list items, compacting a list after a loop that removes
//! some, and sorting (`Stmt::List`); and setting a list's length (`Stmt::SetLen`).
//! The runtime moves the items; the list's header is `{ptr, len, cap, arena}` (see `collections`).

use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData};
use cranelift_module::{FuncId, Module};
use kestrel_kir::{self as kir, ExprKind, KTy, Place};

use super::collections::LEN;
use super::FuncLowerer;

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// `xs.remove_at(i)`, `xs.insert(i, v)`, `xs.clear()`, and the compaction that ends a loop that
    /// removes items. The arguments are evaluated first (`v` may read `xs`, and `i` may be any
    /// integer type), then the runtime moves the items. Range analysis relies on the arguments
    /// running before the place is found (`kir::range` `change_len`): keep that order.
    pub(crate) fn list_op(&mut self, op: kir::ListOp, list: &Place, args: &[kir::Expr]) -> Result<(), String> {
        if matches!(op, kir::ListOp::Reserve | kir::ListOp::WriteBytes | kir::ListOp::WriteCount) {
            return self.bytes_op(op, list, args);
        }
        let ty = self.place_ty(list);
        let (size, align) = self.elem_size_align(ty);
        if op == kir::ListOp::Clear {
            let header = self.place_addr(list)?;
            let size_v = self.b.ins().iconst(types::I64, size);
            self.call_id(self.shared.rt.list_clear, &[header, size_v]);
            return Ok(());
        }
        match op {
            kir::ListOp::Sort => return self.sort_items(list, args, size, self.shared.rt.sort, false),
            kir::ListOp::SortBy(kir::SortKey::Int) => return self.sort_items(list, args, size, self.shared.rt.sort_keyed_int, true),
            kir::ListOp::SortBy(kir::SortKey::Float) => return self.sort_items(list, args, size, self.shared.rt.sort_keyed_float, true),
            _ => {}
        }
        if op == kir::ListOp::Compact {
            let kept = self.expr(&args[0])?;
            let next = self.expr(&args[1])?;
            let header = self.place_addr(list)?;
            let size_v = self.b.ins().iconst(types::I64, size);
            self.call_id(self.shared.rt.list_compact, &[header, kept, next, size_v]);
            return Ok(());
        }
        let at = self.expr(&args[0])?;
        let at = self.widen_i64(at, args[0].ty);
        let (l, f) = self.loc_args(args[0].line);
        let size_v = self.b.ins().iconst(types::I64, size);
        match op {
            kir::ListOp::RemoveAt => {
                let header = self.place_addr(list)?;
                self.call_id(self.shared.rt.list_remove, &[header, at, size_v, l, f]);
            }
            kir::ListOp::Clear
            | kir::ListOp::Compact
            | kir::ListOp::Sort
            | kir::ListOp::SortBy(_)
            | kir::ListOp::Reserve
            | kir::ListOp::WriteBytes
            | kir::ListOp::WriteCount => unreachable!("handled above"),
            kir::ListOp::Insert => {
                let value = &args[1];
                let v = if value.ty.is_aggregate() {
                    let tmp = self.temp_addr(value.ty);
                    self.lower_into(value, tmp)?;
                    tmp
                } else {
                    self.expr(value)?
                };
                let header = self.place_addr(list)?;
                let align_v = self.b.ins().iconst(types::I64, align);
                let slot = self.call_id(self.shared.rt.list_insert, &[header, at, size_v, align_v, l, f]).expect("returns the slot");
                if value.ty.is_aggregate() {
                    self.copy_aggregate(slot, v, value.ty);
                } else {
                    self.store(v, slot, 0);
                }
            }
        }
        Ok(())
    }

    /// `Sort` and `SortBy`: the runtime function `sort` sorts the items in place by the generated
    /// compare or fill function `args[0]`, which gets as its environment word the address of a copy
    /// of the function value `args[1]` (kept in this function's frame until the sort is done), or 0
    /// without one. A fill function (`keyed`) also gets the list's address.
    fn sort_items(&mut self, list: &Place, args: &[kir::Expr], size: i64, sort: FuncId, keyed: bool) -> Result<(), String> {
        let ExprKind::FnRef(compare) = &args[0].kind else { unreachable!("the compare or key function is a `FnRef`") };
        let compare = self.func_address(*compare);
        let env = match args.get(1) {
            Some(key) => {
                let slot = self.temp_addr(KTy::Fn);
                self.lower_into(key, slot)?;
                slot
            }
            None => self.b.ins().iconst(self.ptr, 0),
        };
        let header = self.place_addr(list)?;
        let items = self.b.ins().load(self.ptr, MemFlagsData::trusted(), header, 0);
        let len = self.b.ins().load(types::I64, MemFlagsData::trusted(), header, LEN);
        let size = self.b.ins().iconst(types::I64, size);
        if keyed {
            self.call_id(sort, &[items, len, size, compare, env, header]);
        } else {
            self.call_id(sort, &[items, len, size, compare, env]);
        }
        Ok(())
    }

    /// Sets the length of the list at `list`: the new length first, then the place (range
    /// analysis relies on that order, see `kir::range` `change_len`).
    pub(crate) fn set_len(&mut self, list: &Place, len: &kir::Expr) -> Result<(), String> {
        let n = self.expr(len)?;
        let header = self.place_addr(list)?;
        self.store(n, header, LEN);
        Ok(())
    }
}
