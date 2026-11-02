//! Unary and binary operators: numbers, bit operators, comparisons, vectors and string `+`.

use kestrel_syntax::ast::{BinaryOp, Expr, ExprKind, UnaryOp};

use super::BodyChecker;
use crate::ty::Ty;

impl<'a> BodyChecker<'a> {
    pub(super) fn unary(&mut self, op: UnaryOp, inner: &Expr, want: Option<Ty>) -> Ty {
        match op {
            UnaryOp::Not => {
                self.check_value(inner, Some(Ty::Bool));
                Ty::Bool
            }
            UnaryOp::BitNot => {
                let ty = self.check_expr(inner, want.filter(|t| t.is_int()));
                if self.param_unary(inner, "~", ty) {
                    return Ty::Error;
                }
                if self.t.vector(ty).is_some_and(|k| k.has_bits()) {
                    return ty;
                }
                if !ty.is_int() && ty != Ty::Error {
                    let msg = format!("`~` needs an integer, found {}", self.tn(ty));
                    self.error(inner.span, msg);
                    return Ty::Error;
                }
                ty
            }
            UnaryOp::Neg => {
                let ty = self.check_expr(inner, want.filter(|t| t.is_numeric()));
                if self.param_unary(inner, "-", ty) {
                    return Ty::Error;
                }
                if self.t.vector(ty).is_some() {
                    return ty;
                }
                if ty == Ty::U8 {
                    self.error(inner.span, "can't negate a u8");
                    return Ty::Error;
                }
                // `-0x80000000` would negate the smallest `int`, which overflows (as `-2147483648` does not fit).
                if let (ExprKind::Int(n), Some(&v)) = (&inner.kind, self.t.wrapped_literals.get(&inner.id)) {
                    if v == i32::MIN as i64 && ty == Ty::Int || v == i64::MIN && ty == Ty::I64 {
                        let msg = format!("`-0x{n:X}` negates {v}, which overflows; write `0x{n:X}`");
                        self.error(inner.span, msg);
                        return Ty::Error;
                    }
                }
                if !ty.is_numeric() && ty != Ty::Error {
                    let msg = format!("`-` needs a number, found {}", self.tn(ty));
                    self.error(inner.span, msg);
                    return Ty::Error;
                }
                ty
            }
        }
    }

    pub(super) fn binary(&mut self, e: &Expr, op: BinaryOp, lhs: &Expr, rhs: &Expr, want: Option<Ty>) -> Ty {
        match op {
            BinaryOp::And | BinaryOp::Or => {
                self.check_value(lhs, Some(Ty::Bool));
                self.check_sometimes(rhs, Some(Ty::Bool));
                Ty::Bool
            }
            // A shift's result has the left side's type; the amount is any integer.
            BinaryOp::Shl | BinaryOp::Shr => {
                let lt = self.check_expr(lhs, want.filter(|t| t.is_int()));
                let rt = self.check_expr(rhs, Some(lt).filter(|t| t.is_int()));
                if self.param_operator(e, op.symbol(), lt, rt) {
                    return Ty::Error;
                }
                if self.either_vector(lt, rt) {
                    return self.vec_binary(e, op, lhs, lt, rhs, rt);
                }
                self.bit_operands(op, lhs, lt, rhs, rt);
                if lt.is_int() { lt } else { Ty::Error }
            }
            _ if op.is_bitwise() => {
                let lt = self.check_expr(lhs, want.filter(|t| t.is_int()));
                let rt = self.check_expr(rhs, Some(lt).filter(|t| t.is_int()));
                if self.param_operator(e, op.symbol(), lt, rt) {
                    return Ty::Error;
                }
                if self.either_vector(lt, rt) {
                    return self.vec_binary(e, op, lhs, lt, rhs, rt);
                }
                if !self.bit_operands(op, lhs, lt, rhs, rt) {
                    return Ty::Error;
                }
                self.operands(e, op, lhs, lt, rhs, rt)
            }
            _ if op.is_comparison() => {
                let lt = self.check_expr(lhs, None);
                // (A struct or enum on the left tells the right its type: `r == Result.Ok(value: 3)`.)
                let record = matches!(lt, Ty::Struct(_) | Ty::Enum(_)) && self.t.vector(lt).is_none();
                let rt = self.check_expr(rhs, Some(lt).filter(|t| t.is_numeric() || record));
                // A `T` compares only with the rule for it, and only with another `T`.
                if let Some(ty) = self.param_comparison(e, op, lt, rt) {
                    return ty;
                }
                if self.either_vector(lt, rt) {
                    self.vec_binary(e, op, lhs, lt, rhs, rt);
                    return Ty::Bool;
                }
                // (Rule 7b's `holds_fn` stays separate: it doesn't look inside interface values.)
                // Testing against `none` compares no values (it asks only whether there is one), so
                // `d == none` is not a comparison of interface values.
                let against_none = matches!(lhs.kind, ExprKind::None) || matches!(rhs.kind, ExprKind::None);
                let functions = [lt, rt].iter().any(|&t| self.t.holds_fn(t));
                if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && [lt, rt].iter().any(|&t| self.t.holds_uncomparable(t)) && (functions || !against_none) {
                    // A `T` without `equatable` in `T?`, `list[T]` or `Box[T]` needs the rule.
                    let param = [lt, rt].iter().find_map(|&t| if self.t.holds_uncomparable(t) { self.t.uncomparable_param(t) } else { None });
                    let msg = match param {
                        _ if functions => "function values can't be compared".to_string(),
                        Some(p) => {
                            let pn = self.t.param_name(p);
                            format!("`{}` on `{pn}` needs the rule `equatable`: write `[{pn}: equatable]`", op.symbol())
                        }
                        None => "interface values can't be compared; compare what a method returns".to_string(),
                    };
                    self.error(e.span, msg);
                    return Ty::Bool;
                }
                // Two values of one struct or enum compare field by field when every field can
                // (decision 19); the refusal names the first that can't.
                if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && lt == rt && matches!(lt, Ty::Struct(_) | Ty::Enum(_)) {
                    if let Some(why) = self.t.equatable_refusal(lt) {
                        let msg = format!("`{}` can't compare `{}`s: {why}; compare the fields you mean", op.symbol(), self.tn(lt));
                        self.error(e.span, msg);
                    }
                    return Ty::Bool;
                }
                if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && [lt, rt].iter().any(|t| matches!(t, Ty::Bool | Ty::Str)) {
                    if lt != rt && lt != Ty::Error && rt != Ty::Error {
                        let msg = format!("`{}` needs two numbers, two bools or two strings, found {} and {}", op.symbol(), self.tn(lt), self.tn(rt));
                        self.error(e.span, msg);
                    }
                    return Ty::Bool;
                }
                // `<`, `<=`, `>` and `>=` also order two strings or two bools (G5); a mix, or any
                // other pair that isn't two numbers, is refused here with the ordering's wording.
                if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) && (!lt.is_numeric() || !rt.is_numeric()) {
                    let ordered_pair = lt == rt && matches!(lt, Ty::Bool | Ty::Str);
                    if !ordered_pair && lt != Ty::Error && rt != Ty::Error {
                        let msg = format!("`{}` needs two numbers, two strings or two bools, found {} and {}", op.symbol(), self.tn(lt), self.tn(rt));
                        self.error(e.span, msg);
                    }
                    return Ty::Bool;
                }
                self.operands(e, op, lhs, lt, rhs, rt);
                Ty::Bool
            }
            _ => {
                let hint = want.filter(|t| t.is_numeric());
                let lt = self.check_expr(lhs, hint);
                let rt = self.check_expr(rhs, Some(lt).filter(|t| t.is_numeric()).or(hint));
                if self.param_operator(e, op.symbol(), lt, rt) {
                    return Ty::Error;
                }
                if op.is_wrapping() {
                    return self.wrap_binary(e, op, lhs, lt, rhs, rt);
                }
                if op == BinaryOp::Add && (lt == Ty::Str || rt == Ty::Str) {
                    return self.str_concat(e, lt, rt);
                }
                if self.either_vector(lt, rt) {
                    return self.vec_binary(e, op, lhs, lt, rhs, rt);
                }
                self.operands(e, op, lhs, lt, rhs, rt)
            }
        }
    }

    /// Whether either operand is a vector (see `vec_binary`).
    pub(super) fn either_vector(&self, lt: Ty, rt: Ty) -> bool {
        self.t.vector(lt).is_some() || self.t.vector(rt).is_some()
    }

    /// Bit operators take integers. Returns whether both sides are.
    fn bit_operands(&mut self, op: BinaryOp, lhs: &Expr, lt: Ty, rhs: &Expr, rt: Ty) -> bool {
        for (e, t) in [(lhs, lt), (rhs, rt)] {
            if !t.is_int() && t != Ty::Error {
                let msg = format!("`{}` needs integers, found {}", op.symbol(), self.tn(t));
                self.error(e.span, msg);
                return false;
            }
        }
        lt.is_int() && rt.is_int()
    }

    /// Both operands must be numbers that meet at a common type.
    pub(super) fn operands(&mut self, e: &Expr, op: BinaryOp, lhs: &Expr, lt: Ty, rhs: &Expr, rt: Ty) -> Ty {
        if lt == Ty::Error || rt == Ty::Error {
            return Ty::Error;
        }
        let sym = op.symbol();
        if !lt.is_numeric() || !rt.is_numeric() {
            let msg = format!("`{sym}` needs numbers, found {} and {}", self.tn(lt), self.tn(rt));
            self.error(e.span, msg);
            return Ty::Error;
        }
        match Ty::common(lt, rt) {
            Some(c) => {
                // (A hex literal is read at `c`: see `widen_hex`.)
                self.coerce(lhs, lt, c);
                self.coerce(rhs, rt, c);
                c
            }
            None => {
                let msg = format!("`{sym}` can't mix {} and {}", self.tn(lt), self.tn(rt));
                self.error_help(e.span, msg, "convert one side explicitly, e.g. `int(x)` or `u8(x)`");
                Ty::Error
            }
        }
    }
}
