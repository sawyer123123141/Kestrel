//! `exit(code)` and `fail(message)`: the built-ins that never return (language round, decisions 1
//! and 3). Their type is `never`, as `return`'s, so a call ends a block; `fail` is allowed in
//! `pure` code and `par for` bodies, and `exit` is an effect, like `print`.

use kestrel_syntax::ast::{Arg, Expr, ExprKind};
use kestrel_syntax::Span;

use super::closure::Impure;
use super::BodyChecker;
use crate::builtins::Builtin;
use crate::tables::Res;
use crate::ty::Ty;

/// The built-in that never returns which the call `e` names, if it is one: `fail(...)` or
/// `exit(...)`. (No local or function can take either name.)
fn stop_builtin(e: &Expr) -> Option<Builtin> {
    match &e.kind {
        ExprKind::Call { name, .. } => Builtin::global(name).filter(|b| b.never_returns()),
        _ => None,
    }
}

/// The first call of `fail` or `exit` inside the expression `e` of a constant or a field default,
/// with the error it gets: "the value of a constant can't call `fail`; call it inside a function".
pub(crate) fn stop_in_constant(e: &Expr) -> Option<(Span, String)> {
    if let Some(b) = stop_builtin(e) {
        return Some((e.span, format!("the value of a constant can't call `{}`; call it inside a function", b.name())));
    }
    let in_args = |args: &[Arg]| args.iter().find_map(|a| stop_in_constant(&a.value));
    match &e.kind {
        ExprKind::Call { args, .. } | ExprKind::TypeCall { args, .. } => in_args(args),
        ExprKind::MethodCall { receiver, args, .. } => stop_in_constant(receiver).or_else(|| in_args(args)),
        ExprKind::Field { base, .. } => stop_in_constant(base),
        ExprKind::Unary { expr, .. } => stop_in_constant(expr),
        ExprKind::Binary { lhs, rhs, .. } => stop_in_constant(lhs).or_else(|| stop_in_constant(rhs)),
        ExprKind::ArrayLit(items) => items.iter().find_map(stop_in_constant),
        ExprKind::MapLit(entries) => entries.iter().find_map(|(k, v)| stop_in_constant(k).or_else(|| stop_in_constant(v))),
        _ => None,
    }
}

impl<'a> BodyChecker<'a> {
    /// `exit(code)` or `fail(message)` (built-in `b`), with one plain value (`args_problem` has
    /// passed). Its type is `never`.
    pub(super) fn check_stop_call(&mut self, e: &Expr, b: Builtin, args: &[Arg]) -> Ty {
        self.t.res.insert(e.id, Res::Builtin(b));
        if b == Builtin::Exit {
            self.check_exit_use(e);
        }
        let arg = &args[0].value;
        let want = if b == Builtin::Exit { Ty::Int } else { Ty::Str };
        let ty = self.check_expr(arg, Some(want));
        let ty = self.reject_never(arg, ty);
        let fits = if b == Builtin::Exit { ty.is_int() && ty.coerces_to(Ty::Int) } else { ty == Ty::Str };
        if fits {
            self.coerce(arg, ty, want);
        } else if ty != Ty::Error {
            let msg = if b == Builtin::Exit {
                format!("`exit` takes an `int` exit code, found {}", self.tn(ty))
            } else {
                format!("`fail` takes the message as a string, found {}", self.tn(ty))
            };
            self.error(arg.span, msg);
        }
        Ty::Never
    }

    /// `exit` ends the program, so no `pure` function and no `par for` body may call it. Like
    /// `print`, it makes the enclosing function do I/O.
    fn check_exit_use(&mut self, e: &Expr) {
        // In a closure this makes the closure impure, not the function around it.
        if self.closure_impure(e.span, Impure::Exit) {
            return;
        }
        if self.is_pure {
            self.error(e.span, "pure functions can't call `exit`; return a value and let the caller decide");
        } else if !self.par.is_empty() {
            self.error(e.span, "`par for` bodies can't call `exit`: iterations run at the same time");
        }
        if let Some(f) = self.fn_id {
            self.t.fns[f.0 as usize].io = true;
        }
    }

    /// `let name = fail("x")`: the value is checked and the error says the call never returns.
    /// `None` (nothing checked) when `value` isn't a call of `fail` or `exit`.
    pub(super) fn check_stop_value(&mut self, value: &Expr, name: &str) -> Option<Ty> {
        let b = stop_builtin(value)?;
        let ty = self.check_expr(value, None);
        if ty == Ty::Never {
            self.error(value.span, format!("`{}` never returns, so `{name}` would have no value", b.name()));
        }
        Some(Ty::Error)
    }

    /// The error for a value of type `never` used where a value is needed: an `if` whose branches
    /// all leave, or a call of `fail` or `exit`.
    pub(super) fn never_message(&self, e: &Expr) -> String {
        match self.t.res.get(&e.id) {
            Some(Res::Builtin(b)) if b.never_returns() => format!("`{}` never returns, so this has no value", b.name()),
            _ => "this `if` always leaves early, so it has no value".to_string(),
        }
    }
}
