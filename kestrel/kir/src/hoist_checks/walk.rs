//! One pass of one loop, walked in the order codegen evaluates it (see `super`).

use std::collections::{HashMap, HashSet};

use super::{fails, holds_check, note_write};
use crate::ir::*;
use crate::range::sites::checks_of;
use crate::range::Arith;
use crate::visit::{walk, walk_expr, Node};
use crate::visit_mut::VisitMut;

/// The largest first-pass value kept for a temporary (see `Walk::first`), in nodes: inlined
/// bodies reuse their temporaries, so the copies could otherwise grow without bound.
const MAX_FIRST: usize = 32;

/// The name of each local this pass makes.
const CHECKED: &str = "__checked";

/// One pass of one loop, walked in the order codegen evaluates it, while nothing that can fail or
/// has an effect has been met (`open`).
pub(super) struct Walk<'a> {
    locals: &'a mut Vec<Local>,
    arith: Arith,
    /// How many times the loop writes each local (see `note_write`).
    writes: HashMap<LocalIdx, usize>,
    /// A `for` range's variable, and its start when code before the loop can work it out.
    var: Option<(LocalIdx, Option<Expr>)>,
    /// The temporaries bound once per pass to a value that is the same on every pass, each with
    /// that value as code before the loop reads it.
    steady: HashMap<LocalIdx, Expr>,
    /// The temporaries bound once per pass whose first-pass value code before the loop can work
    /// out, each with that value.
    first: HashMap<LocalIdx, Expr>,
    /// The locals written so far in the pass.
    dirty: HashSet<LocalIdx>,
    /// The locals read so far in the pass.
    seen: HashSet<LocalIdx>,
    open: bool,
    /// What goes before the loop, in order: moved checks and peeled ones.
    out: Vec<Stmt>,
    /// How many statements of `out` there are up to the last moved check.
    moved: usize,
}

impl<'a> Walk<'a> {
    pub(super) fn new(locals: &'a mut Vec<Local>, arith: Arith, writes: HashMap<LocalIdx, usize>, var: Option<(LocalIdx, Option<Expr>)>) -> Self {
        let (steady, first, dirty, seen) = (HashMap::new(), HashMap::new(), HashSet::new(), HashSet::new());
        Walk { locals, arith, writes, var, steady, first, dirty, seen, open: true, out: Vec::new(), moved: 0 }
    }

    /// What goes before the loop: the moved checks and the peeled ones before them (a peeled
    /// check after the last moved one guards nothing).
    pub(super) fn out(self) -> Vec<Stmt> {
        let mut out = self.out;
        out.truncate(self.moved);
        out
    }

    pub(super) fn stmts(&mut self, list: &mut [Stmt]) {
        for s in list {
            if !self.open {
                return;
            }
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match s {
            Stmt::Let(l, e) => {
                let first = self.eval(e);
                let l = *l;
                let temp = self.locals[l.0 as usize].kind == LocalKind::Value && self.writes.get(&l) == Some(&1) && !self.seen.contains(&l);
                if temp && self.invariant(e) {
                    let v = self.subst(e);
                    self.first.insert(l, v.clone());
                    self.steady.insert(l, v);
                } else if let Some(v) = first.filter(|v| temp && size(v) <= MAX_FIRST) {
                    self.first.insert(l, v);
                }
                self.dirty.insert(l);
            }
            Stmt::Assign(p, e) => {
                self.eval(e);
                self.place(p, true);
            }
            Stmt::If { cond, then_body, else_body } => {
                self.eval(cond);
                self.branch(|mut f| walk(then_body, &mut f));
                self.branch(|mut f| walk(else_body, &mut f));
            }
            Stmt::ForRange { var, start, end, body } => {
                self.eval(start);
                self.eval(end);
                self.dirty.insert(*var);
                self.branch(|mut f| walk(body, &mut f));
            }
            Stmt::Prove { body, .. } | Stmt::Assume { check: AssumeCheck::Trust, body, .. } => self.stmts(body),
            Stmt::Bind(l, p) => {
                self.place(p, false);
                self.dirty.insert(*l);
            }
            Stmt::SetLine(_) => {}
            _ => self.open = false,
        }
    }

    /// Code that runs on some passes only, or more than once (walked by `each`): it stops the walk
    /// when it can fail or has an effect, and what it reads and writes is noted.
    fn branch<'n>(&mut self, each: impl FnOnce(&mut dyn FnMut(Node<'n>))) {
        let (arith, mut proven, mut bad) = (self.arith, HashSet::new(), false);
        each(&mut |n| {
            bad |= fails(&n, arith, &mut proven, true);
            if let Node::Place(p, _) = n {
                self.seen.insert(p.root);
            }
            note_write(&n, &mut |l| {
                self.dirty.insert(l);
            });
        });
        if bad {
            self.open = false;
        }
    }

    /// Walks `e` in evaluation order, moving the invariant parts that hold a check and peeling the
    /// checks before them; returns its first-pass value when code before the loop can work it out.
    pub(super) fn eval(&mut self, e: &mut Expr) -> Option<Expr> {
        self.eval_node(e, false)
    }

    /// `eval`; `proven` when `e` is the node directly inside a `Proven` (its checks are proven).
    fn eval_node(&mut self, e: &mut Expr, proven: bool) -> Option<Expr> {
        if !self.open {
            return None;
        }
        if !proven && self.invariant(e) {
            let value = self.subst(e);
            if !holds_check(e, self.arith) {
                return Some(value);
            }
            let load = self.bind(value);
            self.moved = self.out.len();
            *e = load.clone();
            return Some(load);
        }
        let own = !proven && !checks_of(e, self.arith).is_empty();
        let (ty, line, moves) = (e.ty, e.line, matches!(e.kind, ExprKind::Move(_)));
        let made = |kind| Expr { kind, ty, line };
        let first = match &mut e.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => Some(made(e.kind.clone())),
            ExprKind::Load(p) if p.path.is_empty() => {
                self.seen.insert(p.root);
                self.first_value(p.root)
            }
            ExprKind::Load(p) | ExprKind::Len(p) | ExprKind::Tag(p) | ExprKind::Move(p) => {
                self.place(p, moves);
                None
            }
            ExprKind::Proven(x) => self.eval_node(x, true).map(|v| made(ExprKind::Proven(Box::new(v)))),
            ExprKind::Unary(op, x) => {
                let op = *op;
                self.eval(x).map(|v| made(ExprKind::Unary(op, Box::new(v))))
            }
            ExprKind::Cast(x) => self.eval(x).map(|v| made(ExprKind::Cast(Box::new(v)))),
            ExprKind::Binary(op, a, b) => {
                let op = *op;
                let (a, b) = (self.eval(a), self.eval(b));
                a.zip(b).map(|(a, b)| made(ExprKind::Binary(op, Box::new(a), Box::new(b))))
            }
            ExprKind::And(a, b) | ExprKind::Or(a, b) => {
                self.eval(a);
                let b = &**b;
                self.branch(|mut f| walk_expr(b, &mut f));
                None
            }
            ExprKind::If { cond, then_body, then_val, else_body, else_val } => {
                self.eval(cond);
                self.branch(|mut f| {
                    walk(then_body, &mut f);
                    walk(else_body, &mut f);
                    then_val.iter().chain(else_val.iter()).for_each(|v| walk_expr(v, &mut f));
                });
                None
            }
            ExprKind::Seq(stmts, value) => {
                self.stmts(stmts);
                let v = self.eval(value);
                // A temporary of a `Seq` counts only inside it: codegen may evaluate the rest of the
                // expression holding it in another order.
                for st in stmts.iter() {
                    if let Stmt::Let(l, _) = st {
                        self.steady.remove(l);
                        self.first.remove(l);
                    }
                }
                v
            }
            ExprKind::Math(_, xs) | ExprKind::Hash(_, xs) | ExprKind::VecLit(xs) | ExprKind::StructLit(_, xs) | ExprKind::VariantLit(_, _, xs) => {
                for x in xs {
                    self.eval(x);
                }
                None
            }
            ExprKind::Splat(x) | ExprKind::Shuffle(x, _) | ExprKind::LaneOf(x, _) | ExprKind::FieldOf(x, _) => {
                self.eval(x);
                None
            }
            _ => {
                self.open = false;
                None
            }
        };
        if !own || !self.open {
            return first;
        }
        match first {
            // The check as the first pass runs it, peeled before the loop.
            Some(v) => Some(self.bind(v)),
            None => {
                self.open = false;
                None
            }
        }
    }

    /// The index expressions of place `p`, in order; a checked index or a map entry stops the walk.
    /// `written` when the place is written.
    fn place(&mut self, p: &mut Place, written: bool) {
        self.seen.insert(p.root);
        for step in &mut p.path {
            match step {
                Proj::Index { at, check } => {
                    self.eval(at);
                    if *check == IndexCheck::Checked {
                        self.open = false;
                    }
                }
                Proj::MapKey { .. } | Proj::MapValue { .. } => self.open = false,
                Proj::Field(_) | Proj::Variant(..) | Proj::Payload | Proj::Lane(_) => {}
            }
        }
        if written {
            self.dirty.insert(p.root);
        }
    }

    /// Local `l`'s value at this point of the first pass, as code before the loop works it out: a
    /// `for` range's start, a temporary's value, or the local itself while the pass hasn't written
    /// it.
    fn first_value(&self, l: LocalIdx) -> Option<Expr> {
        if let Some((var, start)) = &self.var {
            if *var == l {
                return start.clone().filter(|_| !self.dirty.contains(&l));
            }
        }
        if let Some(v) = self.steady.get(&l).or_else(|| self.first.get(&l)) {
            return Some(v.clone());
        }
        let local = &self.locals[l.0 as usize];
        (local.kind == LocalKind::Value && !self.dirty.contains(&l)).then(|| Expr { kind: ExprKind::Load(Place::local(l)), ty: local.ty, line: 0 })
    }

    /// Whether `e` gives the same value on every pass, and nothing in it but its checks can fail or
    /// has an effect: literals, steady locals, and scalar operators and math of them.
    fn invariant(&self, e: &Expr) -> bool {
        if !matches!(e.ty, KTy::Bool | KTy::U8 | KTy::I32 | KTy::I64 | KTy::F32 | KTy::F64) {
            return false;
        }
        match &e.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => true,
            ExprKind::Load(p) => {
                let l = p.root;
                p.path.is_empty() && (self.steady.contains_key(&l) || (self.locals[l.0 as usize].kind == LocalKind::Value && !self.writes.contains_key(&l)))
            }
            ExprKind::Unary(_, x) | ExprKind::Cast(x) | ExprKind::Proven(x) => self.invariant(x),
            ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) => self.invariant(a) && self.invariant(b),
            ExprKind::Math(_, xs) => xs.iter().all(|x| self.invariant(x)),
            _ => false,
        }
    }

    /// `e` with each steady temporary replaced by its value.
    fn subst(&self, e: &Expr) -> Expr {
        let mut v = e.clone();
        Subst(&self.steady).visit_expr(&mut v);
        v
    }

    /// Binds `value` to a new local before the loop; a load of the local.
    fn bind(&mut self, value: Expr) -> Expr {
        self.locals.push(Local { name: CHECKED.into(), ty: value.ty, kind: LocalKind::Value });
        let l = LocalIdx(self.locals.len() as u32 - 1);
        let load = Expr { kind: ExprKind::Load(Place::local(l)), ty: value.ty, line: value.line };
        self.out.push(Stmt::Let(l, value));
        load
    }
}

/// Replaces each load of a steady temporary with its value.
struct Subst<'a>(&'a HashMap<LocalIdx, Expr>);

impl VisitMut for Subst<'_> {
    fn visit_expr(&mut self, e: &mut Expr) {
        if let ExprKind::Load(p) = &e.kind {
            if let Some(v) = p.path.is_empty().then(|| self.0.get(&p.root)).flatten() {
                *e = v.clone();
                return;
            }
        }
        crate::visit_mut::walk_expr(self, e);
    }
}

/// The number of nodes in `e`.
fn size(e: &Expr) -> usize {
    let mut n = 0;
    walk_expr(e, &mut |_| n += 1);
    n
}

