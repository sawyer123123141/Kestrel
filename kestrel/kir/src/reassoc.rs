//! Xor, and, or chains regrouped so a loop's invariant operands combine outside it (spec
//! decision 9 of the performance round, `docs/superpowers/specs/2026-11-01-perf-round-design.md`).
//!
//! Cranelift 0.137 no longer rebalances `((a ^ b) ^ c) ^ d`, so the voxel world's hash
//! `x * A ^ y * B ^ z * C ^ s * D` combines the three parts the sample loop never changes inside
//! it, once per sample. In a chain of one of `^ & |` on one integer type whose operands have no
//! check left and no effect (so the order they run in can't be seen), the operands the innermost
//! loop around the chain never changes are grouped, in written order, into one subexpression after
//! the others: `x * A ^ ((y * B ^ z * C) ^ s * D)`, which Cranelift then computes before the loop.
//! Every bit is the same: the three operators are associative and commutative. `+` and `*` are
//! left alone (a checked build's overflow points would move), and so is every float.
//!
//! A temporary the loop binds once to an invariant value (an inlined call's argument) counts as
//! invariant. Which operands are invariant only decides how fast the result is, never what it is.

use std::collections::{HashMap, HashSet};

use crate::hoist_checks::{holds_check, note_write};
use crate::ir::*;
use crate::range::Arith;
use crate::visit::{walk, walk_expr, Node};
use crate::visit_mut::{walk_expr as walk_expr_mut, walk_stmt, VisitMut};

/// Regroups the chains in every loop of every function.
pub(crate) fn run(m: &mut Module, arith: Arith) {
    for f in m.funcs.iter_mut().filter(|f| f.native.is_none()) {
        let mut body = std::mem::take(&mut f.body);
        Loops { locals: &f.locals, arith }.visit_stmts(&mut body);
        f.body = body;
    }
}

struct Loops<'a> {
    locals: &'a [Local],
    arith: Arith,
}

impl VisitMut for Loops<'_> {
    fn visit_stmt(&mut self, s: &mut Stmt) {
        walk_stmt(self, s);
        if matches!(s, Stmt::While { .. } | Stmt::ForRange { .. }) {
            regroup_loop(s, self.locals, self.arith);
        }
    }
}

/// Regroups the chains of loop `s` that no inner loop holds.
fn regroup_loop(s: &mut Stmt, locals: &[Local], arith: Arith) {
    let mut writes: HashMap<LocalIdx, usize> = HashMap::new();
    let mut lets = Vec::new();
    walk(std::slice::from_ref(&*s), &mut |n| {
        note_write(&n, &mut |l| *writes.entry(l).or_insert(0) += 1);
        if let Node::Stmt(Stmt::Let(l, e)) = n {
            lets.push((*l, e));
        }
    });
    // The temporaries bound once to an invariant value, which may read earlier ones.
    let mut steady = HashSet::new();
    loop {
        let before = steady.len();
        for (l, e) in &lets {
            let fixed = |x: LocalIdx| steady.contains(&x) || (locals[x.0 as usize].kind == LocalKind::Value && !writes.contains_key(&x));
            let once = writes.get(l) == Some(&1) && locals[l.0 as usize].kind == LocalKind::Value;
            if once && !steady.contains(l) && invariant(e, &fixed) {
                steady.insert(*l);
            }
        }
        if steady.len() == before {
            break;
        }
    }
    let fixed = |x: LocalIdx| steady.contains(&x) || (locals[x.0 as usize].kind == LocalKind::Value && !writes.contains_key(&x));
    let mut r = Regroup { fixed: &fixed, arith };
    match s {
        Stmt::While { cond, body } => {
            r.visit_expr(cond);
            r.visit_stmts(body);
        }
        Stmt::ForRange { body, .. } => r.visit_stmts(body),
        _ => unreachable!("not a loop"),
    }
}

/// Whether `e` is the same on every pass of the loop and pure: literals, locals `fixed` says
/// don't change, and operators and math of them.
fn invariant(e: &Expr, fixed: &impl Fn(LocalIdx) -> bool) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => true,
        ExprKind::Load(p) => p.path.is_empty() && fixed(p.root),
        ExprKind::Unary(_, x) | ExprKind::Cast(x) | ExprKind::Proven(x) => invariant(x, fixed),
        ExprKind::Binary(_, a, b) => invariant(a, fixed) && invariant(b, fixed),
        ExprKind::Math(_, xs) => xs.iter().all(|x| invariant(x, fixed)),
        _ => false,
    }
}

/// Whether running `e` earlier or later can't be seen: no check left, nothing but reads of places
/// whose indexes need no check, and operators.
fn pure(e: &Expr, arith: Arith) -> bool {
    let mut ok = true;
    walk_expr(e, &mut |n| match n {
        Node::Expr(x) => {
            ok &= matches!(
                x.kind,
                ExprKind::Int(_)
                    | ExprKind::Float(_)
                    | ExprKind::Bool(_)
                    | ExprKind::Load(_)
                    | ExprKind::Len(_)
                    | ExprKind::Tag(_)
                    | ExprKind::Proven(_)
                    | ExprKind::Unary(..)
                    | ExprKind::Binary(..)
                    | ExprKind::Cast(_)
                    | ExprKind::Math(..)
            )
        }
        Node::Place(p, _) => ok &= p.path.iter().all(|s| matches!(s, Proj::Field(_) | Proj::Lane(_) | Proj::Index { .. })),
        Node::Stmt(_) => ok = false,
    });
    ok && !holds_check(e, arith)
}

/// The operator of a chain `e` heads: `^`, `&` or `|` on an integer type.
fn chain_op(e: &Expr) -> Option<BinOp> {
    match e.kind {
        ExprKind::Binary(op @ (BinOp::BitXor | BinOp::BitAnd | BinOp::BitOr), ..) if e.ty.is_int() => Some(op),
        _ => None,
    }
}

/// The operands of the chain of `op` that `e` heads, in written order.
fn operands<'e>(e: &'e Expr, op: BinOp, ty: KTy, out: &mut Vec<&'e Expr>) {
    match &e.kind {
        ExprKind::Binary(o, a, b) if *o == op && e.ty == ty => {
            operands(a, op, ty, out);
            operands(b, op, ty, out);
        }
        _ => out.push(e),
    }
}

/// `operands`, moved out of `e`.
fn take_operands(e: Expr, op: BinOp, ty: KTy, out: &mut Vec<Expr>) {
    match e.kind {
        ExprKind::Binary(o, a, b) if o == op && e.ty == ty => {
            take_operands(*a, op, ty, out);
            take_operands(*b, op, ty, out);
        }
        kind => out.push(Expr { kind, ..e }),
    }
}

/// The chain of `op` over `xs`, left to right.
fn fold(xs: Vec<Expr>, op: BinOp, ty: KTy, line: u32) -> Expr {
    let mut it = xs.into_iter();
    let first = it.next().expect("a chain has operands");
    it.fold(first, |a, b| Expr { kind: ExprKind::Binary(op, Box::new(a), Box::new(b)), ty, line })
}

/// Regroups the chains of one loop's body, inner loops left out.
struct Regroup<'a, F: Fn(LocalIdx) -> bool> {
    fixed: &'a F,
    arith: Arith,
}

impl<F: Fn(LocalIdx) -> bool> VisitMut for Regroup<'_, F> {
    fn visit_stmt(&mut self, s: &mut Stmt) {
        match s {
            // An inner loop's chains are its own; its bounds run on this loop's passes.
            Stmt::ForRange { start, end, .. } => {
                self.visit_expr(start);
                self.visit_expr(end);
            }
            Stmt::While { .. } => {}
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &mut Expr) {
        let Some(op) = chain_op(e) else { return walk_expr_mut(self, e) };
        let (ty, line) = (e.ty, e.line);
        let mut refs = Vec::new();
        operands(e, op, ty, &mut refs);
        let steady: Vec<bool> = refs.iter().map(|x| invariant(x, self.fixed)).collect();
        let worth = steady.iter().filter(|&&s| s).count() >= 2 && steady.contains(&false) && refs.iter().all(|x| pure(x, self.arith));
        if !worth {
            return walk_expr_mut(self, e);
        }
        let mut xs = Vec::new();
        let whole = std::mem::replace(e, Expr { kind: ExprKind::Int(0), ty, line });
        take_operands(whole, op, ty, &mut xs);
        for x in &mut xs {
            self.visit_expr(x);
        }
        let (mut changing, mut same) = (Vec::new(), Vec::new());
        for (x, s) in xs.into_iter().zip(steady) {
            if s {
                same.push(x);
            } else {
                changing.push(x);
            }
        }
        changing.push(fold(same, op, ty, line));
        *e = fold(changing, op, ty, line);
    }
}
