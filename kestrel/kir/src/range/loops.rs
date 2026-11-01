//! Loops, analysed as `while` loops: the state at the top is found by repeating the body to a
//! fixed point, widening the ranges that still grow and dropping the facts that got weaker; then
//! one more pass, with that state, decides the checks. A check is decided only in the final pass
//! of every loop around it, so an inner loop skips its deciding pass unless every loop around it
//! is deciding. Also the `Prove` block an annotated loop is lowered into (`region`).

use std::collections::HashMap;

use super::interval::Interval;
use super::state::State;
use super::tracked::Tracked;
use super::{writes, Cx};
use crate::contract::prove_copies;
use crate::hoist_checks::const_int;
use crate::ir::*;
use crate::visit::{walk, Node};

/// After this many rounds, a loop's top keeps only each term's bounds, so its repeats end.
const SETTLE_ROUNDS: u32 = 8;

impl Cx<'_> {
    /// A `Prove` block `s`: a plain block, whose checks left carry it (see `CheckLeft::regions`).
    pub(super) fn region(&mut self, st: State, s: &Stmt, (id, at): (u32, u32), body: &[Stmt]) -> State {
        let f = self.f;
        let copy = self.copies.get_or_insert_with(|| prove_copies(&f.body))[&(s as *const Stmt as usize)];
        self.regions.push(Region { id, at, copy });
        let end = self.stmts(st, body);
        self.regions.pop();
        end
    }

    pub(super) fn while_loop(&mut self, entry: State, cond: &Expr, body: &[Stmt]) -> State {
        self.repeat(entry, cond.line, |cx, top| {
            let (t, f) = cx.condition(top.clone(), cond);
            let (end, frame) = cx.body(t, body);
            let back = joined(&cx.t, end, frame.continues);
            (back, joined(&cx.t, f, frame.breaks))
        })
    }

    /// `for var from start to end`: `var = start; end = <the end, worked out once>; while var <
    /// end { body; var = var + 1 }`, where the step wraps (as codegen emits it) and a `continue`
    /// goes through it. The end is a hidden term, fresh each time the loop is entered.
    pub(super) fn for_range(&mut self, entry: State, s: &Stmt, var: LocalIdx, (start, end): (&Expr, &Expr), body: &[Stmt]) -> State {
        let claims = self.counters.active.len();
        self.claim_counters(&entry, s);
        // The start and the end, once, as a simple statement that assigns `var`.
        let mut st = entry;
        let mut w = writes::of_expr(start);
        w.extend(writes::of_expr(end));
        self.forget(&mut st, &w);
        let mark = self.learned.len();
        let (first, last) = self.within(&w, |cx| (cx.eval_v(&st, start, true), cx.eval_v(&st, end, true)));
        self.apply_learned(&mut st, mark);
        self.forget(&mut st, &w);
        let hidden = self.hidden.get(&(s as *const Stmt as usize)).and_then(|&i| self.t.hidden(i));
        if let Some(h) = hidden {
            st.forget_node(&self.t, h);
            st.bound(&self.t, h, last.r);
            // The end as it was worked out: not over a term the start, the end or setting `var` wrote.
            let mut all = w.clone();
            all.push(writes::Write::Local(var));
            if let Some(x) = last.lin.filter(|x| !self.touched(&all, x.node)) {
                st.equal(&self.t, h, x.node, x.c);
            }
        }
        self.assign(&mut st, var, Some(first), &w);
        let last = last.r;
        let var_node = self.t.node(var);
        let at = s as *const Stmt as usize;
        let exit = self.repeat(st, start.line, |cx, top| {
            // `var < end` decides each round; the end was worked out once, so its range holds.
            let (mut t, mut f) = (top.clone(), top.clone());
            t.narrow(&cx.t, var, Interval::new(i128::MIN / 2, last.hi - 1));
            for &(inner, c, r) in &cx.counters.active {
                if inner == at {
                    t.narrow(&cx.t, c, r);
                }
            }
            f.narrow(&cx.t, var, Interval::new(last.lo, i128::MAX / 2));
            if let (Some(v), Some(h)) = (var_node, hidden) {
                t.fact(&cx.t, v, h, -1);
                f.fact(&cx.t, h, v, 0);
            }
            let (end, frame) = cx.body(t, body);
            let back = cx.step(joined(&cx.t, end, frame.continues), var);
            (back, joined(&cx.t, f, frame.breaks))
        });
        self.counters.active.truncate(claims);
        exit
    }

    /// The nest counter rule: for each counter `s` heads a nest of (see `nests`), adds what the
    /// counter is at the start of each pass of the loop that bumps it, from its range at the
    /// nest's entry: at most `hi + n - 1` after `n` bumps at most, when `hi + n` fits its type (so
    /// no bump wraps). The claims last while `s` is being analysed.
    fn claim_counters(&mut self, entry: &State, s: &Stmt) {
        let f = self.f;
        let nests = self.counters.nests.get_or_insert_with(|| nests(f));
        let Some(found) = nests.get(&(s as *const Stmt as usize)) else { return };
        for &(c, n, inner) in found {
            let ty = f.locals[c.0 as usize].ty;
            if let Some(r) = entry.get(&self.t, c).filter(|r| Interval::new(r.lo, r.hi + n).fits(ty)) {
                self.counters.active.push((inner, c, Interval::new(r.lo, r.hi + n - 1)));
            }
        }
    }

    /// `var = var + 1`, wrapping: a `for` range's step. Its facts move with it while it can't
    /// wrap.
    fn step(&self, mut st: State, var: LocalIdx) -> State {
        if let Some(v) = st.get(&self.t, var) {
            let ty = self.f.locals[var.0 as usize].ty;
            let next = Interval::new(v.lo + 1, v.hi + 1);
            let value = next.fits(ty).then(|| {
                let lin = self.t.node(var).map(|node| super::bounds::Lin { node, c: 1 });
                super::bounds::Val::exact(next, lin)
            });
            self.assign(&mut st, var, value, &[]);
        }
        st
    }

    /// Repeats `round` (which runs the condition and the body from the state at the top, giving
    /// the state going back to the top and the state leaving the loop) to a fixed point, then once
    /// more to decide the checks when every loop around is deciding. The state after the loop.
    fn repeat(&mut self, entry: State, line: u32, round: impl Fn(&mut Self, &State) -> (State, State)) -> State {
        if entry.is_dead() {
            // Never reached: in a deciding pass its checks are listed as kept.
            if self.deciding && !self.gave_up {
                self.keep += 1;
                let top = self.top.clone();
                self.loop_lines.push(line);
                let _ = round(self, &top);
                self.loop_lines.pop();
                self.keep -= 1;
            }
            return entry;
        }
        self.loop_lines.push(line);
        let outer = self.deciding;
        self.deciding = false;
        let mut top = entry.clone();
        let mut rounds = 0;
        let mut exit = loop {
            let (back, exit) = round(self, &top);
            if self.gave_up {
                break exit;
            }
            rounds += 1;
            let next = top.widen(&self.t, &entry.join(&self.t, &back), rounds >= SETTLE_ROUNDS);
            if next == top {
                break exit;
            }
            top = next;
        };
        self.deciding = outer;
        if outer && !self.gave_up {
            exit = round(self, &top).1;
        }
        self.loop_lines.pop();
        exit
    }
}

/// `st` joined with what a frame's slot carried (nothing, when no path took it).
fn joined(t: &Tracked, st: State, other: Option<State>) -> State {
    match other {
        Some(o) => st.join(t, &o),
        None => st,
    }
}

/// The nest counters of a function (found once, at its first `for` range) and the claims of the
/// nests being analysed: the loop whose passes a claim is about (by address), the counter and the
/// range it is in at the start of each pass.
#[derive(Default)]
pub(super) struct Counters {
    nests: Option<HashMap<usize, Vec<Nest>>>,
    pub(super) active: Vec<(usize, LocalIdx, Interval)>,
}

/// A nest counter: the counter, the most times it is bumped, and the address of the loop whose
/// body bumps it.
type Nest = (LocalIdx, i128, usize);

/// Every nest counter of `f`, by the address of the `for` range that heads its nest: the counter,
/// the most times it is bumped, and the loop whose body bumps it. A nest is a `for` range with
/// constant bounds whose body holds, at its top level, more such ranges, down to one whose body
/// holds `c = c + 1` at its top level; the bump runs once per pass of that body, at most the
/// product of the trip counts on the way down. The counter must be an integer `Value` local that
/// nothing else in the nest writes, and the nest holds no `break`, `continue` or `return`.
fn nests(f: &Func) -> HashMap<usize, Vec<Nest>> {
    let mut out = HashMap::new();
    walk(&f.body, &mut |n| {
        if let Node::Stmt(s) = n {
            if let Some(trips) = trips(s) {
                let mut found = Vec::new();
                bumps(s, trips, &mut found);
                found.retain(|&(c, _, _)| {
                    let l = &f.locals[c.0 as usize];
                    l.kind == LocalKind::Value && l.ty.is_int() && writes_in(s, c) == 1
                });
                if !found.is_empty() && !leaves(s) {
                    out.insert(s as *const Stmt as usize, found);
                }
            }
        }
    });
    out
}

/// The trip count of a `for` range with constant bounds that runs at least once.
fn trips(s: &Stmt) -> Option<i128> {
    let Stmt::ForRange { start, end, .. } = s else { return None };
    let n = const_int(end)? - const_int(start)?;
    (n > 0).then_some(n)
}

/// The bumps at the top level of loop `s`'s body (passes: `n`), and of each constant range at its
/// top level, recursively.
fn bumps(s: &Stmt, n: i128, out: &mut Vec<Nest>) {
    let Stmt::ForRange { body, .. } = s else { return };
    for st in body {
        if let Some(c) = bump(st) {
            out.push((c, n, s as *const Stmt as usize));
        } else if let Some(k) = trips(st) {
            bumps(st, n.saturating_mul(k), out);
        }
    }
}

/// The local `s` adds 1 to (`c = c + 1`), if that is what it does.
fn bump(s: &Stmt) -> Option<LocalIdx> {
    let Stmt::Assign(p, e) = s else { return None };
    let e = match &e.kind {
        ExprKind::Proven(x) => x,
        _ => e,
    };
    let ExprKind::Binary(BinOp::Add | BinOp::AddWrap, a, b) = &e.kind else { return None };
    let one = |x: &Expr| matches!(x.kind, ExprKind::Int(1));
    let reads = |x: &Expr| matches!(&x.kind, ExprKind::Load(q) if q.path.is_empty() && q.root == p.root);
    (p.path.is_empty() && ((reads(a) && one(b)) || (one(a) && reads(b)))).then_some(p.root)
}

/// How many statements and places in `s` write local `c`.
fn writes_in(s: &Stmt, c: LocalIdx) -> usize {
    let mut n = 0;
    walk(std::slice::from_ref(s), &mut |node| match node {
        Node::Stmt(Stmt::Let(l, _) | Stmt::Bind(l, _) | Stmt::BindGlobal(l, _) | Stmt::BindEnv { local: l, .. } | Stmt::ForRange { var: l, .. }) if *l == c => n += 1,
        Node::Place(p, true) if p.root == c => n += 1,
        _ => {}
    });
    n
}

/// Whether `s` holds a `break`, `continue` or `return`.
fn leaves(s: &Stmt) -> bool {
    let mut found = false;
    walk(std::slice::from_ref(s), &mut |n| found |= matches!(n, Node::Stmt(Stmt::Break | Stmt::Continue | Stmt::Return(_))));
    found
}
