//! Lowering statements, including discarded expression statements and `print` arguments.

use kestrel_check::{Res, Ty};
use kestrel_syntax::ast;

use super::{binop, helpers, kty, shift_amount, strings, FnLowerer};
use crate::ir::*;

impl<'a> FnLowerer<'a> {
    /// Lowers `s` into `out`; a loop with a `@prove` goes into one `Stmt::Prove` (see `prove`).
    pub(crate) fn stmt(&mut self, s: &ast::Stmt, out: &mut Vec<Stmt>) {
        if !self.annotated_loop(s, out) {
            self.plain_stmt(s, out);
        }
    }

    /// Lowers `s` into `out`, whatever annotation it has.
    pub(crate) fn plain_stmt(&mut self, s: &ast::Stmt, out: &mut Vec<Stmt>) {
        let lowered = match s {
            ast::Stmt::Let { bind, value, else_name, else_block: Some(otherwise), .. } => {
                return self.let_else(bind, value, else_name.as_ref(), otherwise, out);
            }
            ast::Stmt::Let { bind, value, .. } => {
                let idx = self.bind_local(bind);
                let value = self.value(value);
                out.push(Stmt::Let(idx, value));
                return self.declared(idx);
            }
            ast::Stmt::Assign { target: ast::Expr { kind: ast::ExprKind::Index { base, index }, .. }, value, .. }
                if matches!(self.ty(base.id), Ty::Map(..)) =>
            {
                return self.map_set(base, index, value, out);
            }
            ast::Stmt::Assign { target, value, .. } if helpers::owning(kty(self.ty(target.id)), self.t) => {
                return self.assign_owning(target, value, out);
            }
            ast::Stmt::Assign { target, op, value, .. } => {
                let mut place = self.place(target).expect("checker requires an assignable place");
                let ty = kty(self.ty(target.id));
                let rhs = self.expr(value);
                let value = match op.binary() {
                    // Over an existing value: its storage is reused where it has room.
                    None if self.reuses(&place, &rhs, ty) => return self.assign_reusing(place, rhs, out),
                    None => self.owned(rhs, false),
                    Some(bop) => {
                        // The value first, then the place (read and written), once.
                        let indexed = place.path.iter().any(|p| matches!(p, Proj::Index { at, .. } if !matches!(at.kind, ExprKind::Int(_))));
                        let rhs = if indexed {
                            let rhs = self.spill(rhs, out);
                            self.stabilize(&mut place, out);
                            rhs
                        } else {
                            rhs
                        };
                        let line = rhs.line;
                        // (A vector's shift amount becomes its lane type in `vec_combine`.)
                        let rhs = if ty.is_aggregate() { rhs } else { shift_amount(binop(bop), rhs, ty) };
                        let cur = Expr { kind: ExprKind::Load(place.clone()), ty, line };
                        let kind = if ty == KTy::Str {
                            // `s += t` is `s = s + t`: a new string, built before it replaces `s`.
                            let mut parts = vec![PrintArg::Value(cur)];
                            parts.extend(strings::concat_pieces(rhs));
                            ExprKind::Format(parts)
                        } else if ty.is_aggregate() {
                            self.vec_combine(binop(bop), cur, rhs, ty, line)
                        } else if ty.is_vector() {
                            // `v *= 2.0`: the number fills every lane.
                            Self::simd_binary(binop(bop), cur, rhs, ty, line)
                        } else {
                            ExprKind::Binary(binop(bop), Box::new(cur), Box::new(rhs))
                        };
                        Expr { kind, ty, line }
                    }
                };
                Stmt::Assign(place, value)
            }
            ast::Stmt::While { cond, body, .. } => Stmt::While { cond: self.expr(cond), body: self.loop_block(body) },
            ast::Stmt::ForRange { var, start, end, body, .. } => Stmt::ForRange {
                var: self.bind_local(var),
                start: self.expr(start),
                end: self.expr(end),
                body: self.loop_block(body),
            },
            ast::Stmt::ForIn { par, index, var, iter, body, .. } => return self.for_in(*par, index.as_ref(), var, iter, body, out),
            ast::Stmt::Arena { body, .. } => return self.arena_block(None, body, out),
            ast::Stmt::In { arena, body, .. } => return self.arena_block(Some(arena), body, out),
            ast::Stmt::Break(_) => return self.jump(Stmt::Break, out),
            ast::Stmt::Continue(_) => return self.jump(Stmt::Continue, out),
            ast::Stmt::Return { value, .. } => return self.return_stmt(value.as_ref(), out),
            ast::Stmt::Remove { span, .. } => return self.remove_stmt(*span, out),
            // `block_rest` lowers an assumption together with the rest of its block.
            ast::Stmt::Assume { .. } => unreachable!("an assumption is lowered with its block"),
            ast::Stmt::Expr(e) => return self.expr_stmt(e, out),
        };
        out.push(lowered);
    }

    /// An expression whose value is discarded.
    fn expr_stmt(&mut self, e: &ast::Expr, out: &mut Vec<Stmt>) {
        if matches!(self.res(e.id), Some(Res::CallValue(_))) {
            return self.call_value_stmt(e, out);
        }
        let (receiver, args) = match &e.kind {
            ast::ExprKind::If { cond, then_block, else_block } => {
                let cond = self.expr(cond);
                let then_body = self.block_stmts(then_block);
                let else_body = match else_block {
                    Some(b) => self.block_stmts(b),
                    None => Vec::new(),
                };
                out.push(Stmt::If { cond, then_body, else_body });
                return;
            }
            ast::ExprKind::Match { .. } => return self.match_stmt(e, out),
            ast::ExprKind::Try(inner) => return self.try_stmt(e, inner, out),
            ast::ExprKind::Call { args, .. } => (None, args),
            // `fill[int](var s, 1, 2)`: a user `TypeCall` is the call it stands for (P28).
            ast::ExprKind::TypeCall { args, .. } if !super::expr::builtin_type_call(e) => (None, args),
            ast::ExprKind::MethodCall { receiver, args, .. } => (Some(&**receiver), args),
            _ => unreachable!("checker rejects statements with no effect"),
        };
        match self.resolved(e.id) {
            Res::Print => out.push(Stmt::Print(args.iter().map(|a| self.print_arg(&a.value)).collect())),
            Res::Builtin(b) => self.builtin_stmt(b, e, args, out),
            Res::MapRemove => {
                let (receiver, name) = match &e.kind {
                    ast::ExprKind::MethodCall { receiver, name, .. } => (receiver, name),
                    _ => unreachable!("`remove` is a method"),
                };
                let line = self.line(e.span);
                let kind = self.map_method(receiver, name, args, KTy::Bool, line);
                let tmp = self.temp(KTy::Bool);
                out.push(Stmt::Let(tmp, Expr { kind, ty: KTy::Bool, line }));
            }
            Res::List(method) => self.list_stmt(method, e, receiver.expect("a list method is a method"), args, out),
            Res::ListAdd => {
                let v = self.value(&args[0].value);
                let list = self.place(receiver.expect("`add` is a method")).expect("checker requires a place for `add`");
                out.push(Stmt::ListAdd(list, v));
            }
            Res::Fn(f) if self.wants_scratch(f, args) => self.scratch_call_stmt(e, f, args, self.line(e.span), out),
            Res::Fn(f) => {
                let (pre, fi, args) = self.call_parts(e, f, args, receiver);
                out.extend(pre);
                self.call_stmt(fi, args, self.t.func(f).ret, self.line(e.span), out);
            }
            Res::IfaceCall { iface, slot } => self.iface_call_stmt(e, iface, slot, out),
            other => unreachable!("checker rejects a discarded {other:?}"),
        }
    }

    fn print_arg(&mut self, e: &ast::Expr) -> PrintArg {
        match &e.kind {
            ast::ExprKind::Str(s) => PrintArg::Str(s.clone()),
            ast::ExprKind::Interp(parts) => PrintArg::Interp(self.interp_parts(parts)),
            _ => {
                let v = self.expr(e);
                self.helpers.ensure_print(v.ty, self.t);
                PrintArg::Value(self.held(v))
            }
        }
    }

    /// The pieces of an interpolated string, in order. An interpolation
    /// inside another is flattened into it, so the result has only
    /// `PrintArg::Value` and `PrintArg::Str`.
    pub(super) fn interp_parts(&mut self, parts: &[ast::InterpPart]) -> Vec<PrintArg> {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                ast::InterpPart::Lit(s) => out.push(PrintArg::Str(s.clone())),
                ast::InterpPart::Expr(x) => match self.print_arg(x) {
                    PrintArg::Interp(inner) => out.extend(inner),
                    piece => out.push(piece),
                },
            }
        }
        out
    }
}
