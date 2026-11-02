//! Typing for the global built-ins: `sqrt(x)`, `atan2(y, x)`, `PI`,
//! `clamp(x, lo, hi)`... (with vectors, see `vec_builtins`).

use kestrel_syntax::ast::{Arg, Expr};
use kestrel_syntax::Span;

use kestrel_syntax::ast::ExprKind;
use kestrel_syntax::Diagnostic;

use super::closure::Impure;
use super::lists::written;
use super::BodyChecker;
use crate::builtins::Builtin;
use crate::snapshot::snapshot_refusal;
use crate::tables::Res;
use crate::ty::Ty;
use crate::vectors::is_lane_builtin;

/// How a call's value count reads in "`sqrt` takes one value".
fn count_words(n: usize) -> &'static str {
    match n {
        0 => "no values",
        1 => "one value",
        2 => "two values",
        3 => "three values",
        _ => unreachable!("built-in functions take at most three values"),
    }
}

/// The value `checksum` was given as written, for its messages, when it is a place (`w`, `g.level`,
/// `ws[i]`).
pub(crate) fn checksum_root(arg: &Expr) -> Option<String> {
    matches!(arg.kind, ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::Index { .. }).then(|| written(arg))
}

/// What a refusal of built-in `b` says to do with the other parts of a value ("save the fields
/// that hold the game's state instead").
pub(crate) fn snapshot_verb(b: Builtin) -> &'static str {
    match b {
        Builtin::Checksum => "checksum",
        Builtin::FirstDifference => "compare",
        Builtin::FromBytes => "read",
        _ => "save",
    }
}

/// The functions that take floats and give a float (`sqrt`...`round`, `randf_range`).
pub(super) fn is_float_function(b: Builtin) -> bool {
    use Builtin::*;
    matches!(b, Sqrt | Sin | Cos | Tan | Asin | Acos | Atan | Atan2 | Exp | Log | Pow | Floor | Ceil | Round | RandfRange)
}

/// What is wrong with the values written in a built-in call, if anything.
pub(super) fn args_problem(e: &Expr, b: Builtin, args: &[Arg]) -> Option<(Span, String)> {
    if let Some(bad) = args.iter().find(|a| a.label.is_some() || a.is_var) {
        Some((bad.span, "built-in functions take plain values, not labels or `var`".to_string()))
    } else if args.len() != b.arity() {
        Some((e.span, format!("`{}` takes {}", b.name(), count_words(b.arity()))))
    } else {
        None
    }
}

impl<'a> BodyChecker<'a> {
    /// `PI` and `TAU`: floats, as wide as the context wants.
    pub(crate) fn builtin_constant(&mut self, e: &Expr, name: &str, want: Option<Ty>) -> Option<Ty> {
        let b = Builtin::global(name).filter(|b| b.is_constant())?;
        self.t.res.insert(e.id, Res::Builtin(b));
        Some(if want == Some(Ty::F64) { Ty::F64 } else { Ty::Float })
    }

    /// A call to a global built-in function.
    pub(crate) fn check_builtin_call(&mut self, e: &Expr, b: Builtin, args: &[Arg]) -> Ty {
        if !b.is_pure() && !b.never_returns() {
            self.check_random_use(e);
        }
        let name = b.name();
        let problem = if b.is_constant() {
            Some((e.span, format!("`{name}` is a constant; write it without `(...)`")))
        } else {
            args_problem(e, b, args)
        };
        if let Some((span, msg)) = problem {
            self.error(span, msg);
            // Mistakes inside the arguments still show.
            self.check_loose_args(args);
            return Ty::Error;
        }
        match b {
            Builtin::Lerp => self.check_lerp(e, args),
            Builtin::Randf => {
                self.t.res.insert(e.id, Res::Builtin(b));
                Ty::Float
            }
            Builtin::RandiRange | Builtin::RandSeed => self.check_integer_args(e, b, args),
            Builtin::Checksum => self.check_snapshot_arg(e, b, &args[0].value).map_or(Ty::Error, |_| Ty::I64),
            Builtin::ToBytes => match self.check_snapshot_arg(e, b, &args[0].value) {
                Some(()) => Ty::List(self.t.intern_elem(Ty::U8)),
                None => Ty::Error,
            },
            Builtin::FirstDifference => self.check_first_difference(e, args),
            Builtin::Exit | Builtin::Fail => self.check_stop_call(e, b, args),
            Builtin::FromBytes => {
                self.error(e.span, super::from_bytes::NEEDS_TYPE);
                self.check_loose_args(args);
                Ty::Error
            }
            _ if is_lane_builtin(b) => self.check_lane_call(e, b, args),
            _ if is_float_function(b) => self.check_float_args(e, b, args),
            _ => self.check_met_args(e, b, args, Self::number_arg),
        }
    }

    /// The value of `checksum(x)` or `to_bytes(x)` (built-in `b`): any value that can be copied and
    /// checksummed (see `snapshot`). `None` after an error. A placeholder's value is judged at each
    /// copy (`generic::copies`).
    fn check_snapshot_arg(&mut self, e: &Expr, b: Builtin, arg: &Expr) -> Option<()> {
        let ty = self.check_expr(arg, None);
        let ty = self.reject_never(arg, ty);
        if ty == Ty::Error {
            return None;
        }
        self.snapshot_value(e, b, arg, ty)
    }

    /// `arg`, of type `ty` (not an error), as the value built-in `b` takes: a value that can be
    /// copied and checksummed. `None` after an error.
    fn snapshot_value(&mut self, e: &Expr, b: Builtin, arg: &Expr, ty: Ty) -> Option<()> {
        let name = b.name();
        if ty == Ty::Unit {
            self.error(arg.span, format!("`{name}` needs a value, and this gives nothing"));
            return None;
        }
        if let Some((msg, help)) = snapshot_refusal(self.t, ty, checksum_root(arg).as_deref(), name, snapshot_verb(b)) {
            let d = Diagnostic::error(e.span, msg);
            self.diags.push(match help {
                Some(h) => d.with_help(h),
                None => d,
            });
            return None;
        }
        self.t.res.insert(e.id, Res::Builtin(b));
        Some(())
    }

    /// `first_difference(a, b)`: two values of one type that can be copied and checksummed (the
    /// second converts to the first's type, as a literal or a `T` going into a `T?` does). Its
    /// value is a `string?`.
    fn check_first_difference(&mut self, e: &Expr, args: &[Arg]) -> Ty {
        let (a, b) = (&args[0].value, &args[1].value);
        let ta = self.check_expr(a, None);
        let ta = self.reject_never(a, ta);
        let tb = self.check_expr(b, Some(ta).filter(|&t| t != Ty::Error));
        let tb = self.reject_never(b, tb);
        if ta == Ty::Error || tb == Ty::Error {
            return Ty::Error;
        }
        if ta == Ty::Unit {
            // Says the first value gives nothing.
            self.snapshot_value(e, Builtin::FirstDifference, a, ta);
            return Ty::Error;
        }
        if ta != tb && tb != Ty::Unit {
            let into_optional = matches!(ta, Ty::Optional(inner) if tb.coerces_to(self.t.elem(inner)));
            if !tb.coerces_to(ta) && !into_optional {
                let msg = format!("`first_difference` compares two values of one type; these are `{}` and `{}`", self.tn(ta), self.tn(tb));
                self.error(e.span, msg);
                return Ty::Error;
            }
            self.coerce(b, tb, ta);
        }
        let first = Builtin::FirstDifference;
        if tb == Ty::Unit {
            // Says the second value gives nothing.
            self.snapshot_value(e, first, b, tb);
            return Ty::Error;
        }
        match self.snapshot_value(e, first, a, ta) {
            Some(()) => Ty::Optional(self.t.intern_elem(Ty::Str)),
            None => Ty::Error,
        }
    }

    /// The random numbers read and change the generator, so no `pure` function
    /// and no `par for` body may use them. Like `print`, they make the
    /// enclosing function do I/O.
    fn check_random_use(&mut self, e: &Expr) {
        // In a closure this makes the closure impure, not the function around it.
        if self.closure_impure(e.span, Impure::Random) {
            return;
        }
        if self.is_pure {
            self.error(e.span, "pure functions can't use random numbers: each call changes the generator");
        } else if !self.par.is_empty() {
            self.error(e.span, "`par for` bodies can't use random numbers: iterations run at the same time");
        }
        if let Some(f) = self.fn_id {
            self.t.fns[f.0 as usize].io = true;
        }
    }

    /// `ty`, the type of `e`, which a built-in needs to be a number.
    pub(super) fn require_number(&mut self, b: Builtin, e: &Expr, ty: Ty) -> Ty {
        if ty.is_numeric() || ty == Ty::Error {
            return ty;
        }
        let msg = format!("`{}` needs a number here, found {}", b.name(), self.tn(ty));
        self.error(e.span, msg);
        Ty::Error
    }

    /// An argument that must be a number; its type (`Error` if it isn't one).
    /// A literal takes the type `hint` if it fits.
    fn number_arg(&mut self, b: Builtin, a: &Arg, hint: Option<Ty>) -> Ty {
        let ty = self.check_expr(&a.value, hint);
        let ty = self.reject_never(&a.value, ty);
        self.require_number(b, &a.value, ty)
    }

    /// An argument that must be an integer; its type (`Error` if it isn't one).
    fn integer_arg(&mut self, b: Builtin, a: &Arg, hint: Option<Ty>) -> Ty {
        let ty = self.check_expr(&a.value, hint);
        let ty = self.reject_never(&a.value, ty);
        if ty.is_int() || ty == Ty::Error {
            return ty;
        }
        let what = if b == Builtin::RandSeed { "an integer" } else { "integers" };
        let msg = format!("`{}` needs {what}, found {}", b.name(), self.tn(ty));
        self.error(a.value.span, msg);
        Ty::Error
    }

    /// Converts every argument to the type they all meet at, which is the result's.
    /// `tys` are their types, all numbers.
    pub(super) fn meet_args(&mut self, e: &Expr, b: Builtin, args: &[Arg], tys: &[Ty]) -> Ty {
        let result = tys.iter().copied().reduce(|x, y| Ty::common(x, y).expect("numbers meet at one of them")).unwrap_or(Ty::Float);
        for (a, &ty) in args.iter().zip(tys) {
            self.coerce(&a.value, ty, result);
        }
        self.t.res.insert(e.id, Res::Builtin(b));
        result
    }

    /// The float functions: every argument is a number, integers become
    /// `float`, and the arguments meet at one float type, which is the result's.
    fn check_float_args(&mut self, e: &Expr, b: Builtin, args: &[Arg]) -> Ty {
        let mut tys = Vec::with_capacity(args.len());
        for a in args {
            let mut ty = self.number_arg(b, a, None);
            if ty.is_int() {
                self.coerce(&a.value, ty, Ty::Float);
                ty = Ty::Float;
            }
            tys.push(ty);
        }
        if tys.contains(&Ty::Error) {
            return Ty::Error;
        }
        self.meet_args(e, b, args, &tys)
    }

    /// Checks each argument with `arg` (a literal takes the type of the value
    /// before it, like `+`) and converts them to the type they all meet at,
    /// which is the result's. `abs`, `sign`, `min`, `max` and `clamp` use this
    /// with numbers.
    fn check_met_args(&mut self, e: &Expr, b: Builtin, args: &[Arg], arg: fn(&mut Self, Builtin, &Arg, Option<Ty>) -> Ty) -> Ty {
        let mut tys = Vec::with_capacity(args.len());
        let mut met: Option<Ty> = None;
        for a in args {
            let ty = arg(self, b, a, met);
            met = Some(met.and_then(|m| Ty::common(m, ty)).unwrap_or(ty)).filter(|t| t.is_numeric());
            tys.push(ty);
        }
        if tys.contains(&Ty::Error) {
            return Ty::Error;
        }
        self.meet_args(e, b, args, &tys)
    }

    /// `randi_range(lo, hi)`: integers that meet at one type, which is the
    /// result's. `rand_seed(n)`: any integer, and it gives nothing.
    fn check_integer_args(&mut self, e: &Expr, b: Builtin, args: &[Arg]) -> Ty {
        let ty = self.check_met_args(e, b, args, Self::integer_arg);
        if b == Builtin::RandSeed && ty != Ty::Error {
            Ty::Unit
        } else {
            ty
        }
    }

    /// `lerp(a, b, t)`: two numbers (the result is a float, even for integers) and a number `t`
    /// of the result's type; with a vector, see `check_vec_lerp`.
    fn check_lerp(&mut self, e: &Expr, args: &[Arg]) -> Ty {
        let mut tys = [Ty::Error; 3];
        for (slot, a) in tys.iter_mut().zip(args) {
            let ty = self.check_expr(&a.value, None);
            *slot = self.reject_never(&a.value, ty);
        }
        let [first, second, t] = tys;
        let t = self.require_number(Builtin::Lerp, &args[2].value, t);
        if self.either_vector(first, second) {
            return self.check_vec_lerp(e, args, [first, second, t]);
        }
        if !first.is_numeric() || !second.is_numeric() {
            self.lerp_mismatch(e, first, second);
            return Ty::Error;
        }
        if t == Ty::Error {
            return Ty::Error;
        }
        // Integers have no halfway points: they meet at `float`.
        let common = Ty::common(first, second).expect("numbers meet at one of them");
        let result = if common.is_int() { Ty::Float } else { common };
        self.coerce(&args[0].value, first, result);
        self.coerce(&args[1].value, second, result);
        self.coerce(&args[2].value, t, result);
        self.t.res.insert(e.id, Res::Builtin(Builtin::Lerp));
        result
    }

    /// `lerp`'s first two values are neither two numbers nor two vectors of one type.
    pub(super) fn lerp_mismatch(&mut self, e: &Expr, first: Ty, second: Ty) {
        if first != Ty::Error && second != Ty::Error {
            let (a, b) = (self.tn(first), self.tn(second));
            let msg = format!("`lerp` needs two numbers or two vectors of the same size, found {a} and {b}");
            self.error(e.span, msg);
        }
    }
}
