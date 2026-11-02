//! Lowering of the global built-ins.

use kestrel_check::Builtin;
use kestrel_syntax::ast;

use super::FnLowerer;
use crate::ir::*;

/// The value of `PI` or `TAU`; `None` for any other built-in.
pub(crate) fn constant_value(b: Builtin) -> Option<f64> {
    match b {
        Builtin::Pi => Some(std::f64::consts::PI),
        Builtin::Tau => Some(std::f64::consts::TAU),
        _ => None,
    }
}

pub(super) fn node(kind: ExprKind, ty: KTy, line: u32) -> Expr {
    Expr { kind, ty, line }
}

/// The whole number `n` as a value of the number type `ty`.
pub(super) fn number(n: i64, ty: KTy, line: u32) -> Expr {
    node(if ty.is_float() { ExprKind::Float(n as f64) } else { ExprKind::Int(n) }, ty, line)
}

/// `l op r`, as a value of type `ty`.
pub(super) fn binary(op: BinOp, l: &Expr, r: &Expr, ty: KTy, line: u32) -> Expr {
    node(ExprKind::Binary(op, Box::new(l.clone()), Box::new(r.clone())), ty, line)
}

/// `if cond { yes } else { no }` as a value.
pub(super) fn select(cond: Expr, yes: Expr, no: Expr) -> ExprKind {
    ExprKind::If {
        cond: Box::new(cond),
        then_body: Vec::new(),
        then_val: Some(Box::new(yes)),
        else_body: Vec::new(),
        else_val: Some(Box::new(no)),
    }
}

/// The `N` argument values the checker guarantees.
fn values<const N: usize>(vals: Vec<Expr>) -> [Expr; N] {
    vals.try_into().unwrap_or_else(|v: Vec<Expr>| unreachable!("expected {N} values, found {}", v.len()))
}

/// `sign(x)` for a number `x` that can be read more than once: 1, -1 or 0
/// (an unsigned number is never negative).
fn sign(x: &Expr, ty: KTy, line: u32) -> ExprKind {
    let zero = number(0, ty, line);
    let positive = binary(BinOp::Gt, x, &zero, KTy::Bool, line);
    let rest = if ty == KTy::U8 {
        zero
    } else {
        let negative = binary(BinOp::Lt, x, &zero, KTy::Bool, line);
        node(select(negative, number(-1, ty, line), zero), ty, line)
    };
    select(positive, number(1, ty, line), rest)
}

impl<'a> FnLowerer<'a> {
    /// `PI` or `TAU`, as a float literal of the expression's type.
    pub(crate) fn builtin_constant(b: Builtin) -> ExprKind {
        ExprKind::Float(constant_value(b).unwrap_or_else(|| unreachable!("{b:?} isn't a constant")))
    }

    /// A call to a global built-in function; the call's type is `ty`.
    pub(crate) fn builtin_call(&mut self, b: Builtin, args: &[ast::Arg], ty: KTy, line: u32) -> ExprKind {
        if b == Builtin::Checksum {
            return self.checksum(&args[0].value, line);
        }
        if b == Builtin::FirstDifference {
            return self.difference_call(args, ty);
        }
        if b == Builtin::ToBytes {
            return self.bytes_call(&args[0].value, ty);
        }
        // On vectors, lane by lane (`lerp` combines whole vectors); a SIMD vector is one operation.
        if ty.is_vector() && b != Builtin::Lerp {
            return self.simd_lane_call(b, args, ty, line);
        }
        if ty.is_aggregate() && b != Builtin::Lerp {
            return self.vec_lane_call(b, args, ty, line);
        }
        let op = match b {
            Builtin::Sqrt => MathOp::Sqrt,
            Builtin::Sin => MathOp::Sin,
            Builtin::Cos => MathOp::Cos,
            Builtin::Tan => MathOp::Tan,
            Builtin::Asin => MathOp::Asin,
            Builtin::Acos => MathOp::Acos,
            Builtin::Atan => MathOp::Atan,
            Builtin::Atan2 => MathOp::Atan2,
            Builtin::Exp => MathOp::Exp,
            Builtin::Log => MathOp::Log,
            Builtin::Pow => MathOp::Pow,
            Builtin::Floor => MathOp::Floor,
            Builtin::Ceil => MathOp::Ceil,
            Builtin::Round => MathOp::Round,
            Builtin::Lerp => return self.lerp(args, ty, line),
            Builtin::Randf => return ExprKind::RandFloat { ranged: false },
            Builtin::RandfRange => return self.randf_range(args, ty, line),
            Builtin::RandiRange => {
                let [lo, hi] = values(args.iter().map(|a| self.expr(&a.value)).collect());
                return ExprKind::RandInt(Box::new(lo), Box::new(hi));
            }
            Builtin::Abs | Builtin::Sign | Builtin::Min | Builtin::Max | Builtin::Clamp => {
                return self.number_call(b, args, ty, line);
            }
            other => unreachable!("{other:?} isn't a function that gives a value"),
        };
        ExprKind::Math(op, args.iter().map(|a| self.expr(&a.value)).collect())
    }

    /// A call to a global built-in whose value is discarded. Only the random
    /// numbers can be, and `fail` and `exit`, which give none: the rest are pure, which the
    /// checker rejects.
    pub(crate) fn builtin_stmt(&mut self, b: Builtin, e: &ast::Expr, args: &[ast::Arg], out: &mut Vec<Stmt>) {
        if matches!(b, Builtin::Fail | Builtin::Exit) {
            self.stop_stmt(b, e, args, out);
        } else if b == Builtin::RandSeed {
            let seed = self.expr(&args[0].value);
            out.push(Stmt::RandSeed(seed));
        } else {
            // The number is thrown away, but drawing it still moves the generator on.
            let v = self.expr(e);
            let tmp = self.temp(v.ty);
            out.push(Stmt::Let(tmp, v));
        }
    }

    /// `v` evaluated once, in order: a literal can be repeated, anything else goes through a temporary.
    pub(super) fn once(&mut self, v: Expr, pre: &mut Vec<Stmt>) -> Expr {
        match v.kind {
            ExprKind::Int(_) | ExprKind::Float(_) => v,
            _ => self.spill(v, pre),
        }
    }

    /// `abs sign min max clamp` of numbers of type `ty`. Floats use the
    /// float instructions; integers pick with an `if`. Each argument is
    /// evaluated once, left to right.
    fn number_call(&mut self, b: Builtin, args: &[ast::Arg], ty: KTy, line: u32) -> ExprKind {
        let vals: Vec<Expr> = args.iter().map(|a| self.expr(&a.value)).collect();
        // An unsigned number is never negative.
        if b == Builtin::Abs && ty == KTy::U8 {
            let [x] = values(vals);
            return x.kind;
        }
        let mut pre = Vec::new();
        let kind = if ty.is_float() {
            self.float_call(b, vals, ty, line, &mut pre)
        } else {
            let vals = vals.into_iter().map(|v| self.once(v, &mut pre)).collect();
            self.int_call(b, vals, ty, line, &mut pre)
        };
        Self::with_pre(pre, kind, ty, line)
    }

    pub(super) fn float_call(&mut self, b: Builtin, vals: Vec<Expr>, ty: KTy, line: u32, pre: &mut Vec<Stmt>) -> ExprKind {
        match b {
            Builtin::Abs => ExprKind::Math(MathOp::Abs, vals),
            Builtin::Min => ExprKind::Math(MathOp::Min, vals),
            Builtin::Max => ExprKind::Math(MathOp::Max, vals),
            Builtin::Clamp => {
                let [x, lo, hi] = values(vals);
                let floor = node(ExprKind::Math(MathOp::Max, vec![x, lo]), ty, line);
                ExprKind::Math(MathOp::Min, vec![floor, hi])
            }
            Builtin::Sign => {
                let [x] = values(vals);
                let x = self.once(x, pre);
                sign(&x, ty, line)
            }
            other => unreachable!("{other:?} isn't a plain number function"),
        }
    }

    /// `vals` have been through `once`, so each can be read more than once.
    pub(super) fn int_call(&mut self, b: Builtin, vals: Vec<Expr>, ty: KTy, line: u32, pre: &mut Vec<Stmt>) -> ExprKind {
        let flag = KTy::Bool;
        match b {
            Builtin::Abs => {
                let [x] = values(vals);
                let negative = binary(BinOp::Lt, &x, &number(0, ty, line), flag, line);
                // `Neg` is checked: the smallest integer panics with "integer overflow".
                let negated = node(ExprKind::Unary(UnOp::Neg, Box::new(x.clone())), ty, line);
                select(negative, negated, x)
            }
            Builtin::Sign => {
                let [x] = values(vals);
                sign(&x, ty, line)
            }
            Builtin::Min => {
                let [x, y] = values(vals);
                select(binary(BinOp::Le, &x, &y, flag, line), x, y)
            }
            Builtin::Max => {
                let [x, y] = values(vals);
                select(binary(BinOp::Ge, &x, &y, flag, line), x, y)
            }
            Builtin::Clamp => {
                let [x, lo, hi] = values(vals);
                let floor = node(select(binary(BinOp::Gt, &x, &lo, flag, line), x, lo), ty, line);
                // The floor is read twice, so it gets a temporary.
                let floor = self.spill(floor, pre);
                select(binary(BinOp::Lt, &floor, &hi, flag, line), floor, hi)
            }
            other => unreachable!("{other:?} isn't a plain number function"),
        }
    }

    /// `randf_range(lo, hi)`: `lo + (hi - lo) * RandFloat`. Both ends are
    /// evaluated once, in order, before the number is drawn.
    fn randf_range(&mut self, args: &[ast::Arg], ty: KTy, line: u32) -> ExprKind {
        let mut pre = Vec::new();
        let vals: Vec<Expr> = args.iter().map(|a| self.expr(&a.value)).collect();
        let vals = vals.into_iter().map(|v| self.once(v, &mut pre)).collect();
        let [lo, hi] = values(vals);
        let width = binary(BinOp::Sub, &hi, &lo, ty, line);
        let drawn = node(ExprKind::RandFloat { ranged: true }, ty, line);
        let offset = binary(BinOp::Mul, &width, &drawn, ty, line);
        Self::with_pre(pre, ExprKind::Binary(BinOp::Add, Box::new(lo), Box::new(offset)), ty, line)
    }

    /// `lerp(a, b, t)`: `a + (b - a) * t` for numbers, component by component
    /// for vectors. Each argument is evaluated once, left to right.
    fn lerp(&mut self, args: &[ast::Arg], ty: KTy, line: u32) -> ExprKind {
        let mut pre = Vec::new();
        let vals: Vec<Expr> = args.iter().map(|a| self.expr(&a.value)).collect();
        let vals = vals.into_iter().map(|v| self.once(v, &mut pre)).collect();
        let [x, y, t] = values(vals);
        let kind = if ty.is_aggregate() || ty.is_vector() {
            let diff = node(self.vec_combine(BinOp::Sub, y, x.clone(), ty, line), ty, line);
            let scaled = node(self.vec_combine(BinOp::Mul, diff, t, ty, line), ty, line);
            self.vec_combine(BinOp::Add, x, scaled, ty, line)
        } else {
            let diff = binary(BinOp::Sub, &y, &x, ty, line);
            let scaled = binary(BinOp::Mul, &diff, &t, ty, line);
            ExprKind::Binary(BinOp::Add, Box::new(x), Box::new(scaled))
        };
        Self::with_pre(pre, kind, ty, line)
    }
}
