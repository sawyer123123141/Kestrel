//! Accumulator recursion as a loop, in `--release` only (spec decision 6 of the performance
//! round, `docs/superpowers/specs/2026-11-01-perf-round-design.md`).
//!
//! A function whose value is `if c { base } else { E }` (as its one expression, or as an `if`
//! whose branches return), where `E` is `f(a) op f(b)`, `x op f(a)` or `f(a) op x` with `op` one
//! of `+ * ^ & |` on its integer result, becomes
//!
//! ```text
//! acc = identity
//! loop {
//!     if c { return acc op base }
//!     acc = acc op f(a)        (or `acc op x`)
//!     params = b               (or `a`)
//! }
//! ```
//!
//! For `fib` that is `acc += fib(n - 1); n -= 2`: one call per two levels. Everything runs in the
//! order the recursion ran it: the condition, then `f(a)` (its arguments, then the call), then
//! `b`'s arguments, which the next pass's condition reads as the parameters. Only an `x` written
//! after the call moves before it, so it must have no check left and no effect. The values are
//! equal because these operators wrap in `--release` and are associative and commutative there:
//! `acc op (f(a) op rest)` is `(acc op f(a)) op rest`. Every expression keeps the condition that
//! guarded it, so range analysis's marks stay true. Checked builds keep the recursion (their
//! overflow points would move).
//!
//! A recursion that never ends overflows the stack; a rewritten one-call recursion loops instead.

use crate::ir::*;
use crate::range::Arith;
use crate::visit::{walk, Node};

/// The name of the accumulator local.
const ACC: &str = "__acc";
/// The name of each parameter's next value.
const NEXT: &str = "__next";

/// Rewrites every accumulator recursion of a `--release` build as a loop.
pub(crate) fn run(m: &mut Module, arith: Arith) {
    if arith.checked() {
        return;
    }
    for (i, f) in m.funcs.iter_mut().enumerate() {
        if f.native.is_none() {
            rewrite(FuncIdx(i as u32), f, arith);
        }
    }
}

/// What a pass does after the condition fails: the arguments of the recursive call it folds in
/// now (with the value `x` folded in instead, for a one-call recursion), and the arguments of the
/// call the next pass stands for.
struct Step {
    op: BinOp,
    now: Folded,
    next: Vec<Expr>,
}

enum Folded {
    Call(Vec<Arg>),
    Value(Expr),
}

/// The parts of `if c { base } else { E }`: the condition, each branch's statements, `base` and
/// `E`.
struct Parts {
    cond: Expr,
    then_body: Vec<Stmt>,
    base: Expr,
    else_body: Vec<Stmt>,
    value: Expr,
}

fn rewrite(me: FuncIdx, f: &mut Func, arith: Arith) {
    let Some(ret) = f.ret.filter(|t| t.is_int()) else { return };
    let plain_params = f.params.iter().all(|p| f.locals[p.0 as usize].kind == LocalKind::Value);
    if f.mutates_params || !plain_params {
        return;
    }
    let Some(parts) = parts(&f.body) else { return };
    let Some(step) = step(me, &parts.value, ret, arith) else { return };
    let line = parts.value.line;
    let local = |f: &mut Func, name: &str, ty: KTy| {
        f.locals.push(Local { name: name.into(), ty, kind: LocalKind::Value });
        LocalIdx(f.locals.len() as u32 - 1)
    };
    let acc = local(f, ACC, ret);
    let load = |l: LocalIdx, ty: KTy| Expr { kind: ExprKind::Load(Place::local(l)), ty, line };
    let fold = |x: Expr| Expr { kind: ExprKind::Binary(step.op, Box::new(load(acc, ret)), Box::new(x)), ty: ret, line };
    let mut then_body = parts.then_body;
    then_body.push(Stmt::Return(Some(fold(parts.base))));
    let mut body = vec![Stmt::If { cond: parts.cond, then_body, else_body: Vec::new() }];
    body.extend(parts.else_body);
    let now = match step.now {
        Folded::Call(args) => Expr { kind: ExprKind::Call(me, args), ty: ret, line },
        Folded::Value(x) => x,
    };
    body.push(Stmt::Assign(Place::local(acc), fold(now)));
    // The next parameters are all worked out before any is set: each may read the old ones.
    let params = f.params.clone();
    let mut sets = Vec::new();
    for (p, x) in params.into_iter().zip(step.next) {
        let ty = f.locals[p.0 as usize].ty;
        let next = local(f, NEXT, ty);
        body.push(Stmt::Let(next, x));
        sets.push(Stmt::Assign(Place::local(p), load(next, ty)));
    }
    body.extend(sets);
    let identity = match step.op {
        BinOp::Mul | BinOp::MulWrap => 1,
        BinOp::BitAnd => -1,
        _ => 0,
    };
    let true_ = Expr { kind: ExprKind::Bool(true), ty: KTy::Bool, line };
    f.body = vec![
        Stmt::Let(acc, Expr { kind: ExprKind::Int(identity), ty: ret, line }),
        Stmt::While { cond: true_, body },
        // Never reached: the loop only ends by its `return`.
        Stmt::Return(Some(load(acc, ret))),
    ];
}

/// The parts of a body that is `return if c { .. } else { .. }`, `if c { ..; return base } else
/// { ..; return E }`, or `if c { ..; return base }` followed by `..; return E`. No other
/// `return` may be inside.
fn parts(body: &[Stmt]) -> Option<Parts> {
    let parts = match body {
        [Stmt::Return(Some(Expr { kind: ExprKind::If { cond, then_body, then_val: Some(t), else_body, else_val: Some(e) }, .. }))] => Parts {
            cond: (**cond).clone(),
            then_body: then_body.clone(),
            base: (**t).clone(),
            else_body: else_body.clone(),
            value: (**e).clone(),
        },
        [Stmt::If { cond, then_body, else_body }] => {
            let ((then_body, base), (else_body, value)) = (returning(then_body)?, returning(else_body)?);
            Parts { cond: cond.clone(), then_body, base, else_body, value }
        }
        [Stmt::If { cond, then_body, else_body }, rest @ ..] if else_body.is_empty() => {
            let ((then_body, base), (else_body, value)) = (returning(then_body)?, returning(rest)?);
            Parts { cond: cond.clone(), then_body, base, else_body, value }
        }
        _ => return None,
    };
    let mut returns = 0;
    let check = [
        Stmt::Let(LocalIdx(0), parts.cond.clone()),
        Stmt::Let(LocalIdx(0), parts.base.clone()),
        Stmt::Let(LocalIdx(0), parts.value.clone()),
    ];
    for list in [&parts.then_body[..], &parts.else_body[..], &check[..]] {
        walk(list, &mut |n| returns += usize::from(matches!(n, Node::Stmt(Stmt::Return(_)))));
    }
    (returns == 0).then_some(parts)
}

/// A block ending in `return value`: its other statements and the value.
fn returning(list: &[Stmt]) -> Option<(Vec<Stmt>, Expr)> {
    match list {
        [init @ .., Stmt::Return(Some(e))] => Some((init.to_vec(), e.clone())),
        _ => None,
    }
}

/// What `e` folds and recurses on, when it is `f(a) op f(b)`, `x op f(a)` or `f(a) op x` with `op`
/// one of `+ * ^ & |` on the result type.
fn step(me: FuncIdx, e: &Expr, ret: KTy, arith: Arith) -> Option<Step> {
    let e = match &e.kind {
        ExprKind::Proven(x) => x,
        _ => e,
    };
    let ExprKind::Binary(op, l, r) = &e.kind else { return None };
    let folds = matches!(op, BinOp::Add | BinOp::AddWrap | BinOp::Mul | BinOp::MulWrap | BinOp::BitXor | BinOp::BitAnd | BinOp::BitOr);
    if !folds || e.ty != ret || l.ty != ret || r.ty != ret {
        return None;
    }
    let call = |x: &Expr| match &x.kind {
        ExprKind::Call(fi, args) if *fi == me => args.iter().map(|a| if let Arg::Value(v) = a { Some(v.clone()) } else { None }).collect::<Option<Vec<_>>>(),
        _ => None,
    };
    let args = |xs: &[Expr]| xs.iter().cloned().map(Arg::Value).collect();
    let (now, next) = match (call(l), call(r)) {
        (Some(a), Some(b)) => (Folded::Call(args(&a)), b),
        (None, Some(a)) => (Folded::Value((**l).clone()), a),
        // `x` runs before the call now: nothing in it may be seen to.
        (Some(a), None) if crate::reassoc::pure(r, arith) => (Folded::Value((**r).clone()), a),
        _ => return None,
    };
    Some(Step { op: *op, now, next })
}
