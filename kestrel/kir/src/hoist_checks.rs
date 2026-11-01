//! Loop-invariant checks moved before their loop (spec decision 7 of the performance round,
//! `docs/superpowers/specs/2026-11-01-perf-round-design.md`).
//!
//! A check range analysis left (overflow, shift, division, narrowing) whose operands the loop
//! never writes moves out with its value: the largest invariant expression holding it goes to a
//! new `Value` local bound before the loop, and the loop reads the local. Loops are done innermost
//! first, so a check leaves every loop it is invariant in.
//!
//! The same panic, with the same line, must come first. So checks move only while nothing the
//! first pass runs before them can fail or has an effect (a call, a `print`, a list growing, an
//! inner `while`, an index check stop the walk; writes to memory don't, since a panic ends the
//! program), and the moved checks keep their order. A check before them that can fail but whose
//! operands code before the loop can work out for the first pass (`for x from 0 to n` has `x` = 0
//! then) is copied before the loop as it runs on the first pass ("peeled"), in its place in that
//! order; the loop keeps it. The checks run only when the loop runs at least once: a `for` range
//! with constant bounds that does, or behind a guard (`start < end`, a `while`'s condition) that
//! can be worked out again with no check and no effect. Any other loop is left alone.
//!
//! This runs on the build's analysed copy (see `after_marking`), so the report and contracts never
//! see it.

use std::collections::{HashMap, HashSet};

use crate::ir::*;
use crate::range::interval::Interval;
use crate::range::sites::{checks_of, is_bounds_site};
use crate::range::Arith;
use crate::visit::{walk, walk_expr, Node};
use crate::visit_mut::VisitMut;

mod walk;

use walk::Walk;

/// Moves the loop-invariant checks of every function out of their loops.
pub(crate) fn run(m: &mut Module, arith: Arith) {
    for f in m.funcs.iter_mut().filter(|f| f.native.is_none()) {
        let mut body = std::mem::take(&mut f.body);
        Loops { locals: &mut f.locals, arith }.visit_stmts(&mut body);
        f.body = body;
    }
}

/// The value of a constant integer expression: literals and `+ - * /` of them (`Proven` or not),
/// when every step fits its type.
pub(crate) fn const_int(e: &Expr) -> Option<i128> {
    let v = match &e.kind {
        ExprKind::Int(c) => i128::from(*c),
        ExprKind::Proven(x) => return const_int(x),
        ExprKind::Binary(op, a, b) => {
            let (a, b) = (const_int(a)?, const_int(b)?);
            match op {
                BinOp::Add | BinOp::AddWrap => a + b,
                BinOp::Sub | BinOp::SubWrap => a - b,
                BinOp::Mul | BinOp::MulWrap => a.checked_mul(b)?,
                BinOp::Div if b != 0 => a / b,
                _ => return None,
            }
        }
        _ => return None,
    };
    Interval::exact(v).fits(e.ty).then_some(v)
}

/// Whether `e` holds a check left: an arithmetic one, or an index.
pub(crate) fn holds_check(e: &Expr, arith: Arith) -> bool {
    let mut proven = HashSet::new();
    let mut found = false;
    walk_expr(e, &mut |n| found |= fails(&n, arith, &mut proven, false));
    found
}

/// Calls `f` for the local a node writes: by `Let`, `Bind`, `BindEnv`, `BindGlobal`, as a `for`
/// loop's variable, or through a written place (an assignment, a `var` argument, a move).
pub(crate) fn note_write(n: &Node, f: &mut impl FnMut(LocalIdx)) {
    match n {
        Node::Stmt(Stmt::Let(l, _) | Stmt::Bind(l, _) | Stmt::BindGlobal(l, _) | Stmt::BindEnv { local: l, .. } | Stmt::ForRange { var: l, .. }) => f(*l),
        Node::Place(p, true) => f(p.root),
        _ => {}
    }
}

/// Whether node `n` is a check left (`proven` collects the nodes directly inside a `Proven`, whose
/// checks are proven; `walk` reports a node before what is inside it), or, with `effects`, can
/// fail or has an effect some other way: a statement other than a binding, an assignment, an `if`,
/// a `for` range or a plain block, an expression other than a value of locals and operators, a
/// map entry.
pub(super) fn fails(n: &Node, arith: Arith, proven: &mut HashSet<*const Expr>, effects: bool) -> bool {
    match n {
        Node::Expr(x) => {
            if let ExprKind::Proven(inner) = &x.kind {
                proven.insert(&**inner);
            }
            let check = !proven.contains(&(*x as *const Expr)) && !checks_of(x, arith).is_empty();
            check || (effects && !quiet(x))
        }
        Node::Place(p, _) => p.path.iter().any(|s| is_bounds_site(s) || (effects && matches!(s, Proj::MapKey { .. } | Proj::MapValue { .. }))),
        Node::Stmt(s) => {
            effects && !matches!(s, Stmt::Let(..) | Stmt::Assign(..) | Stmt::If { .. } | Stmt::ForRange { .. } | Stmt::Prove { .. } | Stmt::Bind(..) | Stmt::SetLine(_))
        }
    }
}

/// Whether evaluating `x` itself (not its operands) can't fail and has no effect.
fn quiet(x: &Expr) -> bool {
    matches!(
        x.kind,
        ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Load(_)
            | ExprKind::Move(_)
            | ExprKind::Len(_)
            | ExprKind::Tag(_)
            | ExprKind::Proven(_)
            | ExprKind::Unary(..)
            | ExprKind::Binary(..)
            | ExprKind::Cast(_)
            | ExprKind::And(..)
            | ExprKind::Or(..)
            | ExprKind::Math(..)
            | ExprKind::Hash(..)
            | ExprKind::VecLit(_)
            | ExprKind::Splat(_)
            | ExprKind::Shuffle(..)
            | ExprKind::LaneOf(..)
            | ExprKind::FieldOf(..)
            | ExprKind::StructLit(..)
            | ExprKind::VariantLit(..)
            | ExprKind::Seq(..)
            | ExprKind::If { .. }
    )
}

struct Loops<'a> {
    locals: &'a mut Vec<Local>,
    arith: Arith,
}

impl VisitMut for Loops<'_> {
    fn visit_stmts(&mut self, list: &mut Vec<Stmt>) {
        let mut i = 0;
        while i < list.len() {
            // Inner loops first: what leaves them lands in this list, where the outer loop's pass
            // may move it on.
            self.visit_stmt(&mut list[i]);
            if matches!(list[i], Stmt::While { .. } | Stmt::ForRange { .. }) {
                let before = hoist_loop(&mut list[i], self.locals, self.arith);
                let n = before.len();
                list.splice(i..i, before);
                i += n;
            }
            i += 1;
        }
    }
}

/// Moves what can leave loop `s`, and returns the statements that go just before it.
fn hoist_loop(s: &mut Stmt, locals: &mut Vec<Local>, arith: Arith) -> Vec<Stmt> {
    let Some(guard) = runs(s, locals, arith) else { return Vec::new() };
    let mut writes = HashMap::new();
    walk(std::slice::from_ref(s), &mut |n| note_write(&n, &mut |l| *writes.entry(l).or_insert(0) += 1));
    let var = match s {
        Stmt::ForRange { var, start, .. } => Some((*var, plain(start, locals, arith).then(|| start.clone()))),
        _ => None,
    };
    let mut w = Walk::new(locals, arith, writes, var);
    match s {
        Stmt::ForRange { body, .. } => w.stmts(body),
        Stmt::While { cond, body } => {
            w.eval(cond);
            w.stmts(body);
        }
        _ => unreachable!("not a loop"),
    }
    let out = w.out();
    match guard {
        Some(cond) if !out.is_empty() => {
            // Each local gets a value on both paths; only the guarded one is ever read.
            let mut before: Vec<Stmt> = out.iter().map(zero_let).collect();
            let then_body = out.into_iter().map(|st| if let Stmt::Let(l, e) = st { Stmt::Assign(Place::local(l), e) } else { st }).collect();
            before.push(Stmt::If { cond, then_body, else_body: Vec::new() });
            before
        }
        _ => out,
    }
}

/// When loop `s` runs at least once: `Some(None)` always, `Some(Some(cond))` when `cond` holds,
/// `None` when that can't be said cheaply (or it never runs).
fn runs(s: &Stmt, locals: &[Local], arith: Arith) -> Option<Option<Expr>> {
    match s {
        Stmt::ForRange { start, end, .. } => match (const_int(start), const_int(end)) {
            (Some(a), Some(b)) => (a < b).then_some(None),
            _ if plain(start, locals, arith) && plain(end, locals, arith) => {
                let kind = ExprKind::Binary(BinOp::Lt, Box::new(start.clone()), Box::new(end.clone()));
                Some(Some(Expr { kind, ty: KTy::Bool, line: start.line }))
            }
            _ => None,
        },
        Stmt::While { cond, .. } if matches!(cond.kind, ExprKind::Bool(true)) => Some(None),
        Stmt::While { cond, .. } if plain(cond, locals, arith) => Some(Some(cond.clone())),
        _ => None,
    }
}

/// Whether `e` gives the same value when worked out again right before the loop, with no check
/// and no effect: literals, whole `Value` locals, lengths of places reached by fields, operators.
fn plain(e: &Expr, locals: &[Local], arith: Arith) -> bool {
    let mut ok = true;
    walk_expr(e, &mut |n| {
        if let Node::Expr(x) = n {
            ok &= match &x.kind {
                ExprKind::Load(p) => p.path.is_empty() && locals[p.root.0 as usize].kind == LocalKind::Value,
                ExprKind::Len(p) => p.path.iter().all(|s| matches!(s, Proj::Field(_))),
                ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Float(_) | ExprKind::Proven(_) => true,
                ExprKind::Unary(..) | ExprKind::Binary(..) | ExprKind::Cast(_) | ExprKind::And(..) | ExprKind::Or(..) => true,
                _ => false,
            };
        }
    });
    ok && !holds_check(e, arith)
}

/// A `let` giving the local `st` binds the zero of its type.
fn zero_let(st: &Stmt) -> Stmt {
    let Stmt::Let(l, e) = st else { unreachable!("the pass binds only with `let`") };
    let kind = match e.ty {
        KTy::Bool => ExprKind::Bool(false),
        KTy::F32 | KTy::F64 => ExprKind::Float(0.0),
        _ => ExprKind::Int(0),
    };
    Stmt::Let(*l, Expr { kind, ty: e.ty, line: e.line })
}
