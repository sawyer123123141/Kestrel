//! `try r` (the language round, decision 9). On a `Result[T, E]` in a function or closure whose
//! result is a `Result[U, E]` with the same `E`, it is the `Ok` value, or else a return of the
//! error; on a `T?` in one whose result is an optional, the value, or else a return of `none`. It
//! is a `return`: it leaves `arena { }` and `in` blocks, so the error it passes up can't be memory
//! one of them made. KIR finds the result type it returns in `Tables::try_rets`.

use kestrel_syntax::ast::Expr;

use super::ok_wrap::article;
use super::regions::{bit, Ctx};
use super::BodyChecker;
use crate::ty::Ty;

impl<'a> BodyChecker<'a> {
    /// `try inner` (the expression `e`): its type is the value inside `inner`.
    pub(crate) fn check_try(&mut self, e: &Expr, inner: &Expr) -> Ty {
        let found = self.check_expr(inner, None);
        let found = self.reject_never(inner, found);
        if found == Ty::Error {
            return Ty::Error;
        }
        let (value, error) = match (self.t.result_parts(found), found) {
            (Some((value, error)), _) => (value, Some(error)),
            (None, Ty::Optional(elem)) => (self.t.elem(elem), None),
            _ => {
                let msg = format!("`try` takes a `Result` or an optional, found {}", self.tn(found));
                self.error(e.span, msg);
                return Ty::Error;
            }
        };
        self.consume(inner);
        if let Some(msg) = self.try_place_error().or_else(|| self.try_result_error(error)) {
            self.error(e.span, msg);
            return value;
        }
        self.t.try_rets.insert(e.id, self.ret);
        if let Some(error) = error {
            self.check_try_region(e, inner, error);
        }
        value
    }

    /// Why `try` can't leave the code it is in, if it can't: a constant or a field default, a
    /// `par for` body, or a closure whose result type isn't known.
    fn try_place_error(&self) -> Option<String> {
        if self.fn_id.is_none() && !self.in_closure() {
            return Some("`try` goes inside a function".to_string());
        }
        if self.par_inside_frame() {
            return Some("`try` can't leave a `par for` body; handle the error with `let ... else why { }`".to_string());
        }
        if self.infers_result() {
            return Some("write the closure's result type, like `-> Result[int, string]`, to use `try` in it".to_string());
        }
        None
    }

    /// Why the function (or closure) can't return what `try` passes up, if it can't: `error` is
    /// the `Result`'s error type, or `None` for an optional.
    fn try_result_error(&self, error: Option<Ty>) -> Option<String> {
        let ret = self.ret;
        if ret == Ty::Error || error == Some(Ty::Error) {
            return None;
        }
        let who = match self.fn_id.filter(|_| !self.in_closure()) {
            Some(f) => format!("`{}`", self.t.func(f).name),
            None => "this closure".to_string(),
        };
        let returns_optional = matches!(ret, Ty::Optional(_));
        let Some(error) = error else {
            if returns_optional {
                return None;
            }
            let instead = if self.t.result_parts(ret).is_some() {
                "return Result.Err(error: \"...\")"
            } else if ret == Ty::Unit {
                "return"
            } else {
                "..."
            };
            return Some(format!("`try` on an optional needs a function that returns an optional; write `let x = opt else {{ {instead} }}`"));
        };
        if ret == Ty::Unit {
            return Some(format!(
                "`try` returns the error from {who}, which returns nothing; handle it here, like `let x = r else why {{ print(why); return }}`"
            ));
        }
        if returns_optional {
            return Some("`try` on a `Result` needs a function that returns a `Result`; write `let x = r else { return none }`".to_string());
        }
        match self.t.result_parts(ret) {
            Some((_, theirs)) if theirs == error || theirs == Ty::Error => None,
            _ => {
                let name = self.tn(error);
                Some(format!(
                    "`try` passes the error up, but this error is {} `{name}` and {who} returns `{}`; handle it with `match` or `let ... else why {{ }}`",
                    article(&name),
                    self.tn(ret)
                ))
            }
        }
    }

    /// The error `try` passes up leaves every block it is in, so it can't be memory an `arena { }`
    /// or `in` block around it made.
    fn check_try_region(&mut self, e: &Expr, inner: &Expr, error: Ty) {
        if !self.t.has_heap(error) {
            return;
        }
        let r = self.region(inner, true) & !bit(0);
        let Some(i) = (1..self.ctxs.len()).find(|&i| r & bit(i) != 0) else { return };
        let msg = match &self.ctxs[i] {
            Ctx::Owned(p) => format!("can't return memory allocated with `in`: it belongs to `{}`", self.path_name(p)),
            Ctx::Scratch(callee) => format!("can't return memory from inside a call to `{callee}`: it's freed when the call returns"),
            _ => "`try` would return this error out of the `arena { }` block that made it, which frees it; handle it inside the block with `let ... else why { }`".to_string(),
        };
        self.error(e.span, msg);
    }
}
