//! Statements. A simple statement goes through the spec's four steps ("One statement at a time"):
//! forget what its expressions may write; analyse each operand from that state; decide its checks;
//! then add the facts from checks that passed on every path, forget the writes again, and apply
//! the statement's own effect (see `effect`). While the operands are analysed, what they write is
//! volatile (`Cx::within`): two reads of a written term may be two values, so no linear form or
//! learned fact names it. So a fact never outlives the value it is about, no two reads around a
//! write are taken for one value, and the analysis never depends on the order codegen evaluates
//! operands in.

use super::bounds::{Lin, Val};
use super::facts::ZERO;
use super::interval::Interval;
use super::state::State;
use super::tracked::Tracked;
use super::writes::{LenChange, Write, WriteKind};
use super::{terms, writes, Cx, Frame, Learn, MAX_ITEMS};
use crate::ir::*;

impl Cx<'_> {
    /// The state after `body`, run from `st`. Statements after the point where no state reaches
    /// are unreachable: their checks stay.
    pub(super) fn stmts(&mut self, mut st: State, body: &[Stmt]) -> State {
        for (i, s) in body.iter().enumerate() {
            if st.is_dead() {
                self.unreachable(&body[i..]);
                return st;
            }
            st = self.stmt(st, s);
        }
        st
    }

    /// Statements no state reaches: in a deciding pass their checks are listed as kept, and
    /// nothing flows out of them (a `break` there joins no loop's exit).
    pub(super) fn unreachable(&mut self, rest: &[Stmt]) {
        if self.deciding && !self.gave_up && !rest.is_empty() {
            self.keep += 1;
            let top = self.top.clone();
            let _ = self.stmts(top, rest);
            self.keep -= 1;
        }
    }

    fn stmt(&mut self, st: State, s: &Stmt) -> State {
        if !self.enter(true) {
            return State::dead();
        }
        let out = match s {
            Stmt::If { cond, then_body, else_body } => {
                let (t, f) = self.condition(st, cond);
                let a = self.stmts(t, then_body);
                let b = self.stmts(f, else_body);
                a.join(&self.t, &b)
            }
            Stmt::Assume { conds, body, .. } => self.assume(st, s, conds, body),
            Stmt::Prove { id, at, body } => self.region(st, s, (*id, *at), body),
            Stmt::While { cond, body } => self.while_loop(st, cond, body),
            Stmt::ForRange { var, start, end, body } => self.for_range(st, s, *var, (start, end), body),
            Stmt::Break | Stmt::Continue => {
                if self.keep == 0 {
                    if let Some(frame) = self.frames.last_mut() {
                        let slot = if matches!(s, Stmt::Break) { &mut frame.breaks } else { &mut frame.continues };
                        join_into(&self.t, slot, st);
                    }
                }
                State::dead()
            }
            // `fail` and `exit` end the block as `return` does, after their operand is analysed.
            Stmt::Return(_) | Stmt::Fail { .. } | Stmt::Exit { .. } => {
                let _ = self.simple(st, s);
                State::dead()
            }
            Stmt::Trap => State::dead(),
            _ => self.simple(st, s),
        };
        self.leave();
        out
    }

    /// A simple statement, through the four steps.
    fn simple(&mut self, st: State, s: &Stmt) -> State {
        let mut st = st;
        // 1. Forget what the expressions may write.
        let w = writes::of_operands(s);
        self.forget(&mut st, &w);
        // 2 and 3. Each operand from that state, deciding its checks; what they write is volatile.
        let mark = self.learned.len();
        let value = self.within(&w, |cx| cx.operands(&st, s));
        // 4. Facts from checks that passed, then the writes again, then the statement's effect.
        self.apply_learned(&mut st, mark);
        let effect = writes::effect(s);
        let mut rest = writes::of_stmt(s);
        if let Some(e) = &effect {
            if let Some(i) = rest.iter().position(|w| w == e) {
                rest.remove(i);
            }
        }
        self.forget(&mut st, &rest);
        if let Some(e) = effect {
            self.apply(&mut st, &e, value, &rest);
        }
        st
    }

    /// Adds the facts learned since `mark` to `st` (and takes them off the list).
    pub(super) fn apply_learned(&mut self, st: &mut State, mark: usize) {
        let learned: Vec<_> = self.learned.drain(mark..).collect();
        for l in learned {
            match l {
                Learn::Range(local, r) => st.narrow(&self.t, local, r),
                Learn::Diff(a, b, c) => st.fact(&self.t, a, b, c),
            }
        }
    }

    /// Analyses a simple statement's operands from `st`; the value a `let`, an assignment or a
    /// `SetLen` stores.
    fn operands(&mut self, st: &State, s: &Stmt) -> Option<Val> {
        match s {
            Stmt::Let(_, e) | Stmt::Return(Some(e)) | Stmt::RandSeed(e) | Stmt::Fail { message: e, .. } | Stmt::Exit { code: e, .. } => {
                return Some(self.eval_v(st, e, true))
            }
            Stmt::Assign(p, e) | Stmt::SetLen(p, e) => {
                let v = self.eval_v(st, e, true);
                self.place(st, p, true);
                return Some(v);
            }
            Stmt::ListAdd(p, e) => {
                self.eval(st, e, true);
                self.place(st, p, true);
            }
            Stmt::List(_, p, xs) => {
                self.all(st, xs, true);
                self.place(st, p, true);
            }
            Stmt::MapSet(p, k, v, _) => {
                self.eval(st, k, true);
                self.eval(st, v, true);
                self.place(st, p, true);
            }
            Stmt::MapUnlock(p, saved) => {
                self.eval(st, saved, true);
                self.place(st, p, true);
            }
            Stmt::Call(_, args, _) => self.args(st, args, true),
            Stmt::CallIndirect { callee: p, args, .. } | Stmt::CallVirtual { recv: p, args, .. } => {
                self.place(st, p, true);
                self.args(st, args, true);
            }
            Stmt::ParFor { args, count, .. } => {
                self.eval(st, count, true);
                self.args(st, args, true);
            }
            Stmt::Print(parts) | Stmt::Write(parts) => self.print_args(st, parts, true),
            Stmt::BindEnv { env, .. } => {
                self.eval(st, env, true);
            }
            Stmt::Bind(_, p)
            | Stmt::CopyFnEnv(p)
            | Stmt::CopyIface(p)
            | Stmt::Dup(p)
            | Stmt::FreeArena(p)
            | Stmt::DropGpu(p)
            | Stmt::PushArena(Some(p), _) => self.place(st, p, true),
            Stmt::CopyItems(dst, src) => {
                self.place(st, src, true);
                self.place(st, dst, true);
            }
            Stmt::If { .. }
            | Stmt::Assume { .. }
            | Stmt::Prove { .. }
            | Stmt::While { .. }
            | Stmt::ForRange { .. }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Return(None)
            | Stmt::BindGlobal(..)
            | Stmt::PushArena(None, _)
            | Stmt::PopArena { .. }
            | Stmt::SetLine(_)
            | Stmt::Trap => {}
        }
        None
    }

    /// The states where `cond` is true and where it is false, through the spec's condition rule:
    /// forget everything the condition may write, refine with its parts in their order, add the
    /// facts from its checks that passed, then forget its writes again, so no arm, loop body or
    /// loop exit sees a fact about a value the condition changed after the fact was made.
    pub(super) fn condition(&mut self, st: State, cond: &Expr) -> (State, State) {
        let mut st = st;
        let w = writes::of_expr(cond);
        self.forget(&mut st, &w);
        let mark = self.learned.len();
        let (mut t, mut f) = self.cond_parts(st, cond, true);
        let learned: Vec<_> = self.learned.drain(mark..).collect();
        for l in learned {
            for side in [&mut t, &mut f] {
                match l {
                    Learn::Range(local, r) => side.narrow(&self.t, local, r),
                    Learn::Diff(a, b, c) => side.fact(&self.t, a, b, c),
                }
            }
        }
        self.forget(&mut t, &w);
        self.forget(&mut f, &w);
        (t, f)
    }

    /// Runs a loop body from `st` with a fresh frame: the state at its end, and the states its
    /// `break`s and `continue`s carried out.
    pub(super) fn body(&mut self, st: State, body: &[Stmt]) -> (State, Frame) {
        self.frames.push(Frame::default());
        let end = self.stmts(st, body);
        let frame = self.frames.pop().expect("pushed above");
        (end, frame)
    }
}

/// Joins `st` into a frame's slot (empty means no path yet).
fn join_into(t: &Tracked, slot: &mut Option<State>, st: State) {
    *slot = Some(match slot.take() {
        Some(old) => old.join(t, &st),
        None => st,
    });
}


impl Cx<'_> {
    /// Applies a statement's own effect `w` (see `writes::effect`), after the rest of its writes
    /// (`rest`) were forgotten. `value` is what a `let`, an assignment or a `SetLen` stores. A
    /// linear form only relates the new value to terms neither `rest` nor `w` itself writes.
    fn apply(&self, st: &mut State, w: &Write, value: Option<Val>, rest: &[Write]) {
        let mut all = rest.to_vec();
        all.push(w.clone());
        match w {
            Write::Local(l) => self.assign(st, *l, value, rest),
            Write::Place(p, WriteKind::Whole) if p.path.is_empty() => self.assign(st, p.root, value, rest),
            Write::Place(p, WriteKind::Whole) if super::vector::is_lane_place(p) && self.t.is_tracked(p.root) => self.lane_store(st, p.root, value),
            Write::Place(p, WriteKind::Whole) => {
                self.forget(st, std::slice::from_ref(w));
                if let Some(n) = self.list_node(p) {
                    self.set_len(st, n, value.and_then(|v| v.len), &all);
                }
            }
            Write::Place(p, WriteKind::Length(change)) => self.change_len(st, p, change, value, (rest, &all)),
            Write::Place(p, WriteKind::Items) => {
                // The length stays; lengths inside the items may change (they move).
                let wf = self.homes.place_full(p);
                for (i, (_, node)) in self.t.lens.iter().enumerate() {
                    if self.homes.inside(&wf, &self.len_full[i]) {
                        st.forget_node(&self.t, *node);
                    }
                }
            }
        }
    }

    /// `l` gets a new value `value`. What was known about it goes, with every length indexed by
    /// it or inside it; a linear form over a term nothing else in the statement wrote (and that
    /// isn't such a length) relates `l` to that term, and `x = x + c` moves `x`'s facts by `c`
    /// instead.
    ///
    /// An untracked `l` (a list, array or struct, or a reference local: a loop element, a match
    /// view, an inlined `var` parameter) gets its new value in the memory it points at, so that is
    /// written as a place: every length in that memory goes, reached through `l` or not
    /// (`homes.meets`: `grid[k].len` and another element `b.len` when `l` is `for k, l in grid`).
    /// A tracked integer local is only its own value.
    pub(super) fn assign(&self, st: &mut State, l: LocalIdx, value: Option<Val>, rest: &[Write]) {
        let own = if self.t.is_tracked(l) { Write::Local(l) } else { Write::Place(Place::local(l), WriteKind::Whole) };
        let placed = self.placed(&own);
        for (i, (_, node)) in self.t.lens.iter().enumerate() {
            if self.hits_len(&placed, i) {
                st.forget_node(&self.t, *node);
            }
        }
        if !self.t.is_tracked(l) {
            // A list or array gets the stored value's length.
            if let Some(n) = self.list_node(&Place::local(l)) {
                let mut all = rest.to_vec();
                all.push(own);
                self.set_len(st, n, value.and_then(|v| v.len), &all);
            }
            return;
        }
        let Some(v) = value else {
            st.forget(&self.t, l);
            return;
        };
        let node = self.t.node(l);
        let lin = v.lin.filter(|x| !self.touched(rest, x.node) && (Some(x.node) == node || !self.touched(std::slice::from_ref(&own), x.node)));
        match (node, lin) {
            (Some(n), Some(x)) if x.node == n => st.shift(&self.t, n, x.c, v.r),
            (Some(n), Some(x)) => {
                st.set(&self.t, l, v.r);
                st.equal(&self.t, n, x.node, x.c);
            }
            _ => st.set(&self.t, l, v.r),
        }
    }

    /// The node of the length of the list or array at `p`, when it is a term.
    fn list_node(&self, p: &Place) -> Option<usize> {
        terms::list_key(self.m, self.f, p, p.path.len()).and_then(|k| self.t.len_node(&k))
    }

    /// Length node `n` is now `len`, when that is over a term none of `writes` (the statement's,
    /// its effect included) changed. Like `change_len`, it relies on codegen finding the place
    /// after the statement's operands (`codegen/src/func/lists.rs` `set_len`).
    fn set_len(&self, st: &mut State, n: usize, len: Option<Lin>, writes: &[Write]) {
        st.forget_node(&self.t, n);
        if let Some(x) = len.filter(|x| x.node != n && !self.touched(writes, x.node)) {
            st.equal(&self.t, n, x.node, x.c);
        }
    }

    /// The length of the list at `p` changes by `change`: it moves only for the term that must be
    /// `p`'s length; every other length that may be inside `p` is forgotten. `rest` are the
    /// statement's other writes, `all` the same with this one.
    ///
    /// The one place the analysis depends on codegen's order: the key is `p` as named after the
    /// operands ran, so `grid[i].add(next(var i))` shifts the length of `grid[i]` for the new `i`
    /// (`rest` forgot it, and the shift keeps "at least 1"). That holds because codegen evaluates
    /// a list statement's operands before it finds the place (`codegen/src/func/collections.rs`
    /// `list_add`, `lists.rs` `list_op` and `set_len`). `range_order_list_place.kes` fails if
    /// that order changes.
    fn change_len(&self, st: &mut State, p: &Place, change: &LenChange, value: Option<Val>, (rest, all): (&[Write], &[Write])) {
        let key = terms::key_of(self.f, p, p.path.len());
        let set = match change {
            LenChange::Set(e) => value.or_else(|| match e.kind {
                ExprKind::Int(c) => Some(Val::exact(Interval::exact(c.into()), Some(Lin { node: ZERO, c: c.into() }))),
                _ => None,
            }),
            _ => None,
        };
        let mut equal_to = None;
        let wf = self.homes.place_full(p);
        for (i, (k, node)) in self.t.lens.iter().enumerate() {
            let n = *node;
            if key.as_ref() != Some(k) {
                if self.homes.meets(&wf, &self.len_full[i]) {
                    st.forget_node(&self.t, n);
                }
                continue;
            }
            match change {
                LenChange::Add(by) => {
                    let (by, r) = (i128::from(*by), st.node_range(&self.t, n));
                    st.shift(&self.t, n, by, Interval::new(r.lo + by, r.hi + by));
                }
                LenChange::Set(_) => match set.and_then(|v| v.lin.map(|x| (v, x))) {
                    // `SetLen(xs, xs.len - 1)`: the length moves.
                    Some((v, x)) if x.node == n && !self.touched(rest, n) => st.shift(&self.t, n, x.c, v.r),
                    _ => {
                        st.forget_node(&self.t, n);
                        if let Some(r) = set.and_then(|v| v.r.meet(Interval::new(0, MAX_ITEMS))) {
                            st.bound(&self.t, n, r);
                        }
                        equal_to = set.and_then(|v| v.lin).filter(|x| x.node != n && !self.touched(all, x.node)).map(|x| (n, x));
                    }
                },
                LenChange::Unknown => st.forget_node(&self.t, n),
            }
        }
        // After every other length the change may touch is forgotten.
        if let Some((n, x)) = equal_to {
            st.equal(&self.t, n, x.node, x.c);
        }
    }
}
