//! Lowering KIR statements and control flow: `if`, `while`, `for` ranges, `break`, `return`.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, InstBuilder, TrapCode};
use cranelift_module::Module;
use kestrel_kir::{self as kir, LocalIdx, Place};

use super::{FuncLowerer, Slot};

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// Lowers statements; returns whether control can reach the end.
    pub(super) fn stmts(&mut self, list: &[kir::Stmt]) -> Result<bool, String> {
        for s in list {
            if !self.stmt(s)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn stmt(&mut self, s: &kir::Stmt) -> Result<bool, String> {
        match s {
            kir::Stmt::Let(l, e) => {
                if e.ty.is_aggregate() {
                    let dest = self.place_addr(&Place::local(*l))?;
                    self.lower_into(e, dest)?;
                } else {
                    let v = self.expr(e)?;
                    self.store_place(&Place::local(*l), v)?;
                }
            }
            kir::Stmt::Assign(p, e) => self.assign(p, e)?,
            kir::Stmt::If { cond, then_body, else_body } => return self.if_stmt(cond, then_body, else_body),
            kir::Stmt::Assume { conds, check, body } => return self.assume(conds, *check, body),
            kir::Stmt::Prove { body, .. } => return self.stmts(body),
            kir::Stmt::While { cond, body } => self.while_loop(cond, body)?,
            kir::Stmt::ForRange { var, start, end, body } => self.for_range(*var, start, end, body)?,
            kir::Stmt::Break => {
                let (_, brk) = *self.loops.last().expect("break inside a loop");
                self.b.ins().jump(brk, &[]);
                return Ok(false);
            }
            kir::Stmt::Continue => {
                let (cont, _) = *self.loops.last().expect("continue inside a loop");
                self.b.ins().jump(cont, &[]);
                return Ok(false);
            }
            kir::Stmt::Return(value) => {
                match value {
                    Some(e) if e.ty.is_aggregate() => {
                        let dest = self.sret.expect("struct-returning function has a result pointer");
                        self.lower_into(e, dest)?;
                        self.b.ins().return_(&[]);
                    }
                    Some(e) => {
                        let v = self.expr(e)?;
                        self.b.ins().return_(&[v]);
                    }
                    None => {
                        self.b.ins().return_(&[]);
                    }
                }
                return Ok(false);
            }
            kir::Stmt::Call(fi, args, _) => {
                self.call(*fi, args, None)?;
            }
            kir::Stmt::CallIndirect { callee, args, sig } => {
                self.call_indirect(callee, args, sig, None)?;
            }
            kir::Stmt::CallVirtual { recv, slot, args, sig, .. } => {
                self.call_virtual(recv, *slot, args, sig, None)?;
            }
            kir::Stmt::CopyIface(p) => self.copy_iface(p)?,
            kir::Stmt::Print(args) => self.print(args, true)?,
            kir::Stmt::Write(args) => self.print(args, false)?,
            kir::Stmt::RandSeed(n) => self.rand_seed(n)?,
            kir::Stmt::ListAdd(list, value) => self.list_add(list, value)?,
            kir::Stmt::List(op, list, args) => self.list_op(*op, list, args)?,
            kir::Stmt::SetLen(list, len) => self.set_len(list, len)?,
            kir::Stmt::Dup(p) => self.dup(p)?,
            kir::Stmt::CopyItems(dst, src) => self.copy_items(dst, src)?,
            kir::Stmt::MapSet(map, key, value, copy_key) => self.map_set(map, key, value, *copy_key, key.line)?,
            kir::Stmt::MapUnlock(map, saved) => self.map_unlock(map, saved)?,
            kir::Stmt::ParFor { func, args, count } => self.par_for(*func, args, count)?,
            kir::Stmt::PushArena(..) | kir::Stmt::PopArena { .. } | kir::Stmt::FreeArena(_) | kir::Stmt::DropGpu(_) => {
                self.arena_stmt(s)?
            }
            kir::Stmt::Bind(l, p) => {
                let addr = self.place_addr(p)?;
                self.bind_ref(*l, addr);
            }
            kir::Stmt::BindEnv { local, env, offset } => self.bind_env(*local, env, *offset)?,
            kir::Stmt::CopyFnEnv(p) => self.copy_fn_env(p)?,
            kir::Stmt::BindGlobal(l, g) => self.bind_global(*l, *g),
            kir::Stmt::SetLine(line) => self.set_line(*line),
            kir::Stmt::Trap => {
                let kind = self.b.ins().iconst(types::I32, kestrel_runtime::PANIC_ABSTRACT as i64);
                let (line, file) = self.loc_args(kir::pack_line(kir::NO_FILE as usize, 0));
                self.call_id(self.shared.rt.panic, &[kind, line, file]);
                self.b.ins().trap(TrapCode::unwrap_user(2));
                return Ok(false);
            }
            kir::Stmt::Fail(msg) => {
                let (p, n) = self.string(msg)?;
                self.call_id(self.shared.rt.fail, &[p, n]);
                self.b.ins().trap(TrapCode::unwrap_user(2));
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Points reference local `l` at `addr`.
    pub(crate) fn bind_ref(&mut self, l: LocalIdx, addr: cranelift_codegen::ir::Value) {
        let Slot::Ptr(v) = self.slots[l.0 as usize] else { unreachable!("binds a reference local") };
        self.b.def_var(v, addr);
    }

    /// An `if` statement: straight-line code with selects when it is small enough (see `select`),
    /// else a branch.
    fn if_stmt(&mut self, cond: &kir::Expr, then_body: &[kir::Stmt], else_body: &[kir::Stmt]) -> Result<bool, String> {
        let sides = [(then_body, None), (else_body, None)];
        if let Some(locals) = self.selectable(sides) {
            let t = self.test(cond, true)?;
            self.select_sides(&t, sides, &locals)?;
            return Ok(true);
        }
        let t = self.test(cond, false)?;
        let (then_b, else_b, merge) = (self.b.create_block(), self.b.create_block(), self.b.create_block());
        self.b.ins().brif(t.flag, then_b, &[], else_b, &[]);
        let then_falls = self.branch_side(&t, true, then_b, sides[0])?.is_some();
        if then_falls {
            self.b.ins().jump(merge, &[]);
        }
        let else_falls = self.branch_side(&t, false, else_b, sides[1])?.is_some();
        if else_falls {
            self.b.ins().jump(merge, &[]);
        }
        self.b.switch_to_block(merge);
        if !then_falls && !else_falls {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(false);
        }
        Ok(true)
    }

    fn while_loop(&mut self, cond: &kir::Expr, body: &[kir::Stmt]) -> Result<(), String> {
        let cached = self.cache_headers(Some(cond), body);
        let (header, body_b, exit) = (self.b.create_block(), self.b.create_block(), self.b.create_block());
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        let c = self.expr(cond)?;
        self.b.ins().brif(c, body_b, &[], exit, &[]);
        self.b.switch_to_block(body_b);
        self.loops.push((header, exit));
        let falls = self.stmts(body)?;
        self.loops.pop();
        if falls {
            self.b.ins().jump(header, &[]);
        }
        self.b.switch_to_block(exit);
        self.uncache(cached);
        Ok(())
    }

    fn for_range(&mut self, var: LocalIdx, start: &kir::Expr, end: &kir::Expr, body: &[kir::Stmt]) -> Result<(), String> {
        let start_v = self.expr(start)?;
        let end_v = self.expr(end)?;
        let place = Place::local(var);
        self.store_place(&place, start_v)?;
        let cached = self.cache_headers(None, body);
        let ty = self.local_ty(var);
        let (header, body_b, step, exit) =
            (self.b.create_block(), self.b.create_block(), self.b.create_block(), self.b.create_block());
        self.b.ins().jump(header, &[]);

        self.b.switch_to_block(header);
        let i = self.load_place(&place)?;
        let cc = if ty.is_signed() { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan };
        let more = self.b.ins().icmp(cc, i, end_v);
        self.b.ins().brif(more, body_b, &[], exit, &[]);

        self.b.switch_to_block(body_b);
        self.loops.push((step, exit));
        let falls = self.stmts(body)?;
        self.loops.pop();
        if falls {
            self.b.ins().jump(step, &[]);
        }

        // `i < end` held, so `i + 1` can't overflow.
        self.b.switch_to_block(step);
        let i = self.load_place(&place)?;
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.store_place(&place, next)?;
        self.b.ins().jump(header, &[]);

        self.b.switch_to_block(exit);
        self.uncache(cached);
        Ok(())
    }
}
