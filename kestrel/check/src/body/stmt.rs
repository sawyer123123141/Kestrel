//! Blocks and statements: `check_block`, and `check_stmt` for each kind of statement.

use std::collections::HashMap;

use kestrel_syntax::ast::{Block, ExprKind, Stmt};

use super::loop_vars::LoopVar;
use super::narrow::Site;
use super::{regions, BodyChecker, Expect};
use crate::ty::Ty;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_block(&mut self, block: &Block, expect: Expect) -> Ty {
        self.scopes.push(HashMap::new());
        let mut ty = Ty::Unit;
        let last = block.stmts.len().saturating_sub(1);
        for (i, stmt) in block.stmts.iter().enumerate() {
            ty = match (stmt, expect) {
                (Stmt::Expr(e), Expect::Value(want)) if i == last => self.check_tail(e, want),
                (Stmt::Expr(e), Expect::Infer) if i == last => self.check_tail_infer(e),
                _ => self.check_stmt(stmt),
            };
        }
        self.scopes.pop();
        // The block's `let ... else` views end with it.
        let depth = self.scopes.len();
        self.let_guards.retain(|g| g.depth <= depth);
        ty
    }

    /// Checks a statement; returns `Never` when control can't continue past it.
    pub(super) fn check_stmt(&mut self, s: &Stmt) -> Ty {
        match s {
            Stmt::Let { is_var, bind, ty, value, else_name, else_block: Some(otherwise), .. } => {
                self.check_let_else(*is_var, bind, ty.as_ref(), value, else_name.as_ref(), otherwise);
                Ty::Unit
            }
            Stmt::Let { is_var, bind, ty, value, .. } => {
                // `let n = fail("x")`: says the call never returns.
                if let Some(local_ty) = self.check_stop_value(value, &bind.name) {
                    self.declare(bind, local_ty, *is_var, false, false);
                    return Ty::Unit;
                }
                let ann = ty.as_ref().map(|t| self.resolve(t));
                let mut local_ty = self.going_into(value, Site::Variable, |c| c.check_value(value, ann));
                if matches!(local_ty, Ty::Unit | Ty::Never) {
                    self.error(value.span, "this expression has no value to store");
                    local_ty = Ty::Error;
                }
                self.consume(value);
                self.declare(bind, local_ty, *is_var, false, false);
                Ty::Unit
            }
            Stmt::Assign { target, op, value, .. } => {
                self.check_assign(target, *op, value);
                Ty::Unit
            }
            Stmt::While { cond, body, .. } => {
                // The condition runs each time around, so the loop starts before it.
                let entry = self.moved.clone();
                self.check_value(cond, Some(Ty::Bool));
                let forever = matches!(cond.kind, ExprKind::Bool(true));
                // The loop ends when the condition is false, after it has run (and moved what it moves).
                let after_cond = if forever { None } else { Some(self.moved.clone()) };
                let broke = self.loop_body(entry, after_cond, body);
                // `while true` with no `break` can only be left by `return`.
                if forever && !broke {
                    Ty::Never
                } else {
                    Ty::Unit
                }
            }
            Stmt::ForRange { var, start, end, body, .. } => {
                let st = self.check_expr(start, None);
                let st = self.reject_never(start, st);
                let et = self.check_expr(end, if st.is_int() { Some(st) } else { None });
                let et = self.reject_never(end, et);
                let var_ty = match Ty::common(st, et) {
                    _ if st == Ty::Error || et == Ty::Error => Ty::Error,
                    Some(t) if t.is_int() => {
                        self.coerce(start, st, t);
                        self.coerce(end, et, t);
                        t
                    }
                    _ => {
                        let msg = format!("`for ... from ... to` needs integers, found {} and {}", self.tn(st), self.tn(et));
                        self.error(start.span.to(end.span), msg);
                        Ty::Error
                    }
                };
                self.scopes.push(HashMap::new());
                let counter = self.declare(var, var_ty, false, false, false);
                self.note_loop_var(counter, LoopVar::Counter);
                let entry = self.moved.clone();
                self.loop_body(entry.clone(), Some(entry), body);
                self.scopes.pop();
                Ty::Unit
            }
            Stmt::ForIn { par, index, var, iter, body, .. } => {
                self.check_for_in(*par, index.as_ref(), var, iter, body);
                Ty::Unit
            }
            // A block that always leaves (say, ends in `return`) never finishes.
            Stmt::Arena { body, .. } => {
                self.ctxs.push(regions::Ctx::Scoped);
                let ty = self.check_block(body, Expect::Discard);
                self.ctxs.pop();
                if ty == Ty::Never { Ty::Never } else { Ty::Unit }
            }
            Stmt::In { arena, body, .. } => self.check_in(arena, body),
            Stmt::Break(span) | Stmt::Continue(span) => {
                let is_break = matches!(s, Stmt::Break(_));
                let word = if is_break { "break" } else { "continue" };
                if let Some(msg) = self.closure_jump_error(word) {
                    self.error(*span, msg);
                    return Ty::Never;
                }
                if is_break && self.par.last().is_some_and(|p| p.depth == self.loop_breaks.len()) {
                    self.error(*span, "`break` can't stop a `par for`: iterations run at the same time");
                }
                match self.loop_breaks.last_mut() {
                    Some(broke) => *broke |= is_break,
                    None => {
                        self.error(*span, format!("`{word}` outside a loop"));
                    }
                }
                if is_break {
                    self.note_break();
                } else {
                    self.back_edge();
                }
                Ty::Never
            }
            Stmt::Return { value, span } => {
                if self.par_inside_frame() {
                    self.error(*span, "`return` can't leave a `par for` body");
                }
                let what = if self.in_closure() { "closure" } else { "function" };
                if self.infers_result() {
                    self.infer_return(value.as_ref(), *span);
                    return Ty::Never;
                }
                match (value, self.ret) {
                    (Some(v), Ty::Unit) => {
                        self.check_expr(v, None);
                        self.error(v.span, format!("this {what} doesn't return a value"));
                    }
                    (Some(v), r) => {
                        self.going_into(v, Site::Return, |c| c.check_value(v, Some(r)));
                        self.consume(v);
                        self.check_return_region(v);
                    }
                    (None, Ty::Unit | Ty::Error) => {}
                    (None, r) => {
                        let msg = format!("this {what} must return {}", self.tn(r));
                        self.error(*span, msg);
                    }
                }
                Ty::Never
            }
            Stmt::Remove { name, name_span, span } => {
                self.check_remove(name, *name_span, *span);
                Ty::Unit
            }
            Stmt::Assume { cond, .. } => {
                self.check_assume(cond);
                Ty::Unit
            }
            Stmt::Expr(e) => self.check_discarded(e),
        }
    }
}

impl<'a> BodyChecker<'a> {
    /// The last expression of a closure body whose result type isn't known (`Expect::Infer`).
    pub(crate) fn check_tail_infer(&mut self, e: &kestrel_syntax::ast::Expr) -> Ty {
        let ty = match &e.kind {
            ExprKind::If { .. } => self.check_if(e, Expect::Infer),
            ExprKind::Match { .. } => self.check_match(e, Expect::Infer),
            _ => return self.check_tail(e, None),
        };
        self.t.expr_ty[e.id.0 as usize] = ty;
        ty
    }
}
