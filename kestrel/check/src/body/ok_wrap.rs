//! A `T` going in where a `Result[T, E]` is expected (the language round, decision 8): it is wrapped
//! as `Result.Ok(value: v)`, exactly where a `T` would go into a `T?` (`coerce`), after converting
//! to `T` (a widening, `T?` when `T` is an optional, an interface the type lists). KIR builds the
//! `Ok` from the `coerce` entry. An `E` is never wrapped, even when it would convert to `T`, and
//! when `T` and `E` are one type nothing is: an error is always written.

use kestrel_syntax::ast::{Expr, ExprKind};

use super::BodyChecker;
use crate::ty::Ty;

impl<'a> BodyChecker<'a> {
    /// The type to check `e` against where `want` is expected: `T` for a value that needs its
    /// expected type to have one at all or to have the right one (`none`, `[]`, `{}`, a closure, and
    /// a number when `E` isn't one) where `want` is a `Result[T, E]`, so `let r: Result[int?,
    /// string] = none` goes in as `Ok(value: none)` and `let b: Result[u8, string] = 7` as a `u8`;
    /// `want` itself otherwise.
    pub(super) fn ok_hint(&self, e: &Expr, want: Option<Ty>) -> Option<Ty> {
        let Some((value, error)) = want.and_then(|w| self.t.result_parts(w)) else { return want };
        let hinted = match e.kind {
            ExprKind::None | ExprKind::ArrayLit(_) | ExprKind::MapLit(_) | ExprKind::Closure(_) => true,
            ExprKind::Int(_) | ExprKind::Float(_) => !error.is_numeric(),
            _ => false,
        };
        if hinted {
            Some(value)
        } else {
            want
        }
    }

    /// Where `to` is a `Result[T, E]`: records `e` (a `from`) going in as `Result.Ok(value: e)`
    /// when it converts to `T`, or reports why it doesn't when it is an `E`. Returns whether it did
    /// either; `false` leaves `coerce`'s own "expected" message.
    pub(super) fn ok_wrap(&mut self, e: &Expr, from: Ty, to: Ty) -> bool {
        let Some((value, error)) = self.t.result_parts(to) else { return false };
        if value == error {
            if !self.converts_to(from, value) {
                return false;
            }
            let msg = format!("`{}` has one type for its value and its error; write `Result.Ok(value: ...)` or `Result.Err(error: ...)`", self.tn(to));
            self.error(e.span, msg);
        } else if from == error {
            let name = self.tn(error);
            let msg = format!("this is {} `{name}`, the error type of `{}`; write `Result.Err(error: ...)` for an error", article(&name), self.tn(to));
            self.error(e.span, msg);
        } else if self.converts_to(from, value) {
            self.t.coerce.insert(e.id, to);
        } else {
            return false;
        }
        true
    }

    /// Whether a `from` goes where a `to` is expected: the same type, a widening, a value into
    /// its optional, a struct or enum into an interface it lists (or that interface's optional).
    fn converts_to(&self, from: Ty, to: Ty) -> bool {
        from == to || from.coerces_to(to) || self.iface_target(from, to).is_some() || matches!(to, Ty::Optional(inner) if from.coerces_to(self.t.elem(inner)))
    }
}

/// `a` or `an` before a type's name, as it is said: "an `int`", "a `u8`", "a `string`".
fn article(name: &str) -> &'static str {
    if name.starts_with(['a', 'e', 'i', 'o', 'A', 'E', 'I', 'O']) {
        "an"
    } else {
        "a"
    }
}
