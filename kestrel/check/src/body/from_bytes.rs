//! `from_bytes[T](bytes)`: the value of type `T` that `to_bytes` wrote, or the reason the bytes
//! aren't one (the language round, decision 13). The type in the brackets is the only way to say
//! what to read (the parser takes `from_bytes`'s brackets as types, as it does `list`'s), and it
//! must be one `checksum` can take (see `snapshot`). The value is a `list[u8]`; the result is a
//! `Result[T, string]`, the prelude's `Result` (which may hold an optional, so `from_bytes[int?]`
//! gives a `Result[int?, string]`).

use kestrel_syntax::ast::{Arg, Expr, TypeExpr};
use kestrel_syntax::{Diagnostic, Span};

use super::BodyChecker;
use crate::builtins::Builtin;
use crate::snapshot::snapshot_refusal;
use crate::tables::Res;
use crate::ty::Ty;

/// What `from_bytes` without its brackets says, called or named as a value.
pub(crate) const NEEDS_TYPE: &str = "`from_bytes` needs the type to read, like `from_bytes[Game](bytes)`";

impl BodyChecker<'_> {
    /// `from_bytes[T](bytes)`, whose type arguments are `types` (written at `name_span`).
    pub(crate) fn check_from_bytes(&mut self, e: &Expr, name_span: Span, types: &[TypeExpr], args: &[Arg]) -> Ty {
        let ty = match types {
            [t] => self.resolve(t),
            _ => {
                self.error(name_span, "`from_bytes[...]` takes one type, the type to read");
                Ty::Error
            }
        };
        let bytes = Ty::List(self.t.intern_elem(Ty::U8));
        match args {
            [a] if a.label.is_none() && !a.is_var => {
                let got = self.check_expr(&a.value, Some(bytes));
                let got = self.reject_never(&a.value, got);
                self.coerce(&a.value, got, bytes);
            }
            _ => {
                self.error(e.span, "`from_bytes[T](bytes)` takes one list of bytes");
                self.check_loose_args(args);
            }
        }
        if ty == Ty::Error {
            return Ty::Error;
        }
        // A placeholder has no fault here: each copy of a generic judges its own type.
        if let Some((msg, help)) = snapshot_refusal(self.t, ty, None, "from_bytes", "read") {
            let d = Diagnostic::error(e.span, msg);
            self.diags.push(match help {
                Some(h) => d.with_help(h),
                None => d,
            });
            return Ty::Error;
        }
        // The driver always adds the prelude; a program checked without it has no `Result`.
        let Some(result) = self.t.result else {
            self.error(e.span, "`from_bytes` gives a `Result`, and this program has no prelude to define it");
            return Ty::Error;
        };
        self.t.res.insert(e.id, Res::Builtin(Builtin::FromBytes));
        let args = self.t.intern_ty_list(vec![ty, Ty::Str]);
        Ty::Enum(self.t.intern_enum(result, args))
    }
}
