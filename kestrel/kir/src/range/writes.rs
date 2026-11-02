//! What a statement or expression may write. Every place KIR's walker marks as written
//! (`visit::places`) is a whole write, unless it is one of the known list operations below; so a
//! write added to KIR later is covered as long as the walker marks it.
//!
//! One addition: a virtual call writes its receiver's place as a whole (the walker reads it, since
//! the interface value itself doesn't change). A borrowed interface value
//! (`MakeIface { data: Borrowed(g) }`) lends `g` to the call it is passed to, so the walker marks
//! `g` written where the value is made, and while it is a call's argument that is enough. But
//! `inline_small` may move such an argument into a local of the inlined body
//! (`let d = MakeIface Borrowed(g)`, later `d.heal()`), and the call's other arguments run between
//! the two: `cure_now(var g, g.items[1])` checks `g.items[1]` after the `let` and before `heal`
//! empties `g.items`. So `Homes` points `d` at `g` (as a loop element points into its list), and
//! the virtual call's write of `d` forgets every length inside `g`.

use std::collections::HashMap;

use super::state::State;
use super::terms::{Full, Term};
use crate::ir::*;
use crate::visit::{walk, walk_expr, Node};

/// How a list's length changes.
#[derive(Clone, Debug, PartialEq)]
pub enum LenChange {
    /// By this many items (`add` and `insert` 1, `remove_at` -1).
    Add(i64),
    /// To this value (`clear` 0, `SetLen` its operand).
    Set(Expr),
    /// To something not known (`Compact`, the end of a `remove b` loop).
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WriteKind {
    /// Anything in the place may change.
    Whole,
    /// The items change, the length doesn't (`sort`, `Dup`).
    Items,
    /// The length changes (and the items with it).
    Length(LenChange),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Write {
    /// A local gets a new value: `let`, a `for` range's variable, or a reference local pointed at
    /// another place (`Bind`, `BindEnv`, `BindGlobal`).
    Local(LocalIdx),
    /// A place is written (an assignment, a move, a `var` argument, a lent interface value, ...).
    Place(Place, WriteKind),
}

impl Write {
    /// The local whose own value this write may change: a local's new value, or a place rooted at
    /// it (an `Addressed` integer local passed as `var`, for one).
    pub fn root(&self) -> LocalIdx {
        match self {
            Write::Local(l) => *l,
            Write::Place(p, _) => p.root,
        }
    }
}

/// Everything `s` may write, its own effect included.
pub fn of_stmt(s: &Stmt) -> Vec<Write> {
    let mut c = Collector::default();
    walk(std::slice::from_ref(s), &mut |n| c.node(n));
    c.out
}

/// Everything running `stmts` may write.
pub fn of_stmts(stmts: &[Stmt]) -> Vec<Write> {
    let mut c = Collector::default();
    walk(stmts, &mut |n| c.node(n));
    c.out
}

/// The statement's own effect, when the analysis applies it as more than forgetting: a `let` or
/// an assignment (the value it stores), or a list operation whose change to the length is known.
/// It is one of `of_stmt`'s writes.
pub fn effect(s: &Stmt) -> Option<Write> {
    match s {
        Stmt::Let(l, _) => Some(Write::Local(*l)),
        Stmt::Assign(p, _) => Some(Write::Place(p.clone(), WriteKind::Whole)),
        _ => list_change(s).map(|(p, kind)| Write::Place(p.clone(), kind)),
    }
}

/// Everything evaluating `e` may write.
pub fn of_expr(e: &Expr) -> Vec<Write> {
    let mut c = Collector::default();
    c.expr(e);
    c.out
}

/// What evaluating the operands of a simple statement may write: the writes of its expressions,
/// including the index expressions of its places and the values of its arguments, but not the
/// statement's own effect (the place it assigns, the list it changes, its `var` arguments).
pub fn of_operands(s: &Stmt) -> Vec<Write> {
    let mut c = Collector::default();
    for e in operands(s) {
        c.expr(e);
    }
    c.out
}

/// The expressions a simple statement evaluates before its effect, in no particular order: its
/// values, the values of its arguments, and the index expressions of its places. A compound
/// statement (`If`, `While`, `ForRange`) has none here: its parts are analysed by their own rules.
pub fn operands(s: &Stmt) -> Vec<&Expr> {
    let mut out = Vec::new();
    match s {
        Stmt::Let(_, e) | Stmt::Return(Some(e)) | Stmt::RandSeed(e) | Stmt::Fail { message: e, .. } | Stmt::Exit { code: e, .. } => out.push(e),
        Stmt::Assign(p, e) | Stmt::ListAdd(p, e) | Stmt::SetLen(p, e) => {
            out.push(e);
            index_exprs(p, &mut out);
        }
        Stmt::List(_, p, xs) => {
            out.extend(xs);
            index_exprs(p, &mut out);
        }
        Stmt::MapSet(p, k, v, _) => {
            out.extend([k, v]);
            index_exprs(p, &mut out);
        }
        Stmt::MapUnlock(p, saved) => {
            out.push(saved);
            index_exprs(p, &mut out);
        }
        Stmt::Call(_, args, _) => args_of(args, &mut out),
        Stmt::CallIndirect { callee: p, args, .. } | Stmt::CallVirtual { recv: p, args, .. } => {
            index_exprs(p, &mut out);
            args_of(args, &mut out);
        }
        Stmt::ParFor { args, count, .. } => {
            out.push(count);
            args_of(args, &mut out);
        }
        Stmt::Print(parts) | Stmt::Write(parts) => print_values(parts, &mut out),
        Stmt::BindEnv { env, .. } => out.push(env),
        Stmt::CopyItems(dst, src) => {
            index_exprs(src, &mut out);
            index_exprs(dst, &mut out);
        }
        Stmt::Bind(_, p)
        | Stmt::CopyFnEnv(p)
        | Stmt::CopyIface(p)
        | Stmt::Dup(p)
        | Stmt::FreeArena(p)
        | Stmt::DropGpu(p)
        | Stmt::PushArena(Some(p), _) => index_exprs(p, &mut out),
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
    out
}

fn index_exprs<'a>(p: &'a Place, out: &mut Vec<&'a Expr>) {
    out.extend(p.path.iter().filter_map(Proj::at));
}

/// The value arguments, and the index expressions of the `var` arguments' places.
fn args_of<'a>(args: &'a [Arg], out: &mut Vec<&'a Expr>) {
    for a in args {
        match a {
            Arg::Value(e) => out.push(e),
            Arg::Ref(p) => index_exprs(p, out),
        }
    }
}

fn print_values<'a>(parts: &'a [PrintArg], out: &mut Vec<&'a Expr>) {
    for p in parts {
        match p {
            PrintArg::Value(e) => out.push(e),
            PrintArg::Interp(inner) => print_values(inner, out),
            PrintArg::Str(_) => {}
        }
    }
}

#[derive(Default)]
struct Collector {
    out: Vec<Write>,
    /// The kind of the next write of a place, by the place's address: set by the list statement
    /// that owns the place, which the walker reports before the place itself.
    kinds: HashMap<usize, WriteKind>,
}

impl Collector {
    fn expr(&mut self, e: &Expr) {
        walk_expr(e, &mut |n| self.node(n));
    }

    fn node(&mut self, n: Node) {
        match n {
            Node::Stmt(s) => {
                match s {
                    Stmt::Let(l, _) | Stmt::ForRange { var: l, .. } | Stmt::Bind(l, _) | Stmt::BindGlobal(l, _) => {
                        self.out.push(Write::Local(*l))
                    }
                    Stmt::BindEnv { local, .. } => self.out.push(Write::Local(*local)),
                    _ => {}
                }
                if let Some((p, kind)) = list_change(s) {
                    self.kinds.insert(p as *const Place as usize, kind);
                }
                if let Stmt::CallVirtual { recv, .. } = s {
                    self.out.push(Write::Place(recv.clone(), WriteKind::Whole));
                }
            }
            Node::Expr(Expr { kind: ExprKind::CallVirtual { recv, .. }, .. }) => {
                self.out.push(Write::Place(recv.clone(), WriteKind::Whole));
            }
            Node::Place(p, true) => {
                let kind = self.kinds.remove(&(p as *const Place as usize)).unwrap_or(WriteKind::Whole);
                self.out.push(Write::Place(p.clone(), kind));
            }
            Node::Place(_, false) | Node::Expr(_) => {}
        }
    }
}

/// The list a statement changes and how, for the list operations whose change is known.
fn list_change(s: &Stmt) -> Option<(&Place, WriteKind)> {
    Some(match s {
        Stmt::ListAdd(p, _) => (p, WriteKind::Length(LenChange::Add(1))),
        Stmt::List(ListOp::Insert, p, _) => (p, WriteKind::Length(LenChange::Add(1))),
        Stmt::List(ListOp::RemoveAt, p, _) => (p, WriteKind::Length(LenChange::Add(-1))),
        Stmt::List(ListOp::Clear, p, _) => (p, WriteKind::Length(LenChange::Set(Expr { kind: ExprKind::Int(0), ty: KTy::I64, line: 0 }))),
        Stmt::SetLen(p, n) => (p, WriteKind::Length(LenChange::Set(n.clone()))),
        // The target takes the source's length (and its items).
        Stmt::CopyItems(dst, _) => (dst, WriteKind::Length(LenChange::Unknown)),
        Stmt::List(ListOp::Compact, p, _) => (p, WriteKind::Length(LenChange::Unknown)),
        Stmt::List(ListOp::Sort | ListOp::SortBy(_), p, _) | Stmt::Dup(p) => (p, WriteKind::Items),
        // `Reserve` moves the items into new room; the writes change bytes, never the length.
        Stmt::List(ListOp::Reserve | ListOp::WriteBytes | ListOp::WriteCount, p, _) => (p, WriteKind::Items),
        _ => return None,
    })
}

/// The local a write gives a new value as a whole, when it is one: a `let`, a place that is just
/// a local (an integer local passed as `var`, for one), or a lane of a local (`v.x = 5`, `var v.y`:
/// a vector's one range covers every lane, so writing one writes the vector).
fn whole_local(w: &Write) -> Option<LocalIdx> {
    match w {
        Write::Local(l) => Some(*l),
        Write::Place(p, _) if p.path.is_empty() || super::vector::is_lane_place(p) => Some(p.root),
        Write::Place(..) => None,
    }
}

/// A write, with its place from its home (worked out once, for comparing with many terms).
pub(super) struct Placed<'w> {
    pub w: &'w Write,
    pub full: Option<Full>,
}

impl super::Cx<'_> {
    pub(super) fn placed<'w>(&self, w: &'w Write) -> Placed<'w> {
        let full = match w {
            Write::Place(p, _) => Some(self.homes.place_full(p)),
            Write::Local(_) => None,
        };
        Placed { w, full }
    }

    /// Forgets everything `writes` may change: the range and facts of each tracked local they
    /// write, and every length term they may touch.
    pub(super) fn forget(&self, st: &mut State, writes: &[Write]) {
        if writes.is_empty() || st.is_dead() {
            return;
        }
        for w in writes {
            if let Some(l) = whole_local(w) {
                st.forget(&self.t, l);
            }
        }
        if self.t.lens.is_empty() {
            return;
        }
        let placed: Vec<Placed> = writes.iter().map(|w| self.placed(w)).collect();
        for (i, (_, node)) in self.t.lens.iter().enumerate() {
            if placed.iter().any(|p| self.hits_len(p, i)) {
                st.forget_node(&self.t, *node);
            }
        }
    }

    /// Runs `f` (which evaluates the operands of one statement, one condition part, a `for` loop's
    /// start and end, or a `Seq`'s or arm's value) with the nodes `writes` (what those may write)
    /// volatile, then puts the outer ones back. A volatile node was forgotten in the state they are
    /// evaluated from; its reads may come before or after the write, in an order the analysis
    /// doesn't depend on, so no linear form, and no fact from a passed check, names it.
    pub(super) fn within<R>(&mut self, writes: &[Write], f: impl FnOnce(&mut Self) -> R) -> R {
        let set = if writes.is_empty() { Vec::new() } else { self.touched_nodes(writes) };
        let outer = std::mem::replace(&mut self.volatile, set);
        let out = f(self);
        self.volatile = outer;
        out
    }

    /// Whether node `n` is volatile in the expression being evaluated (see `within`).
    pub(super) fn volatile(&self, n: usize) -> bool {
        self.volatile.get(n).copied().unwrap_or(false)
    }

    /// For each node, whether one of `writes` may change its value.
    pub(super) fn touched_nodes(&self, writes: &[Write]) -> Vec<bool> {
        let mut out = vec![false; self.t.nodes()];
        let placed: Vec<Placed> = writes.iter().map(|w| self.placed(w)).collect();
        for w in writes {
            if let Some(n) = whole_local(w).and_then(|l| self.t.node(l)) {
                out[n] = true;
            }
        }
        for (i, (_, node)) in self.t.lens.iter().enumerate() {
            out[*node] = placed.iter().any(|p| self.hits_len(p, i));
        }
        out
    }

    /// Whether one of `writes` may change node `n`'s value.
    pub(super) fn touched(&self, writes: &[Write], n: usize) -> bool {
        match self.t.term(n) {
            Some(Term::Local(l)) => writes.iter().any(|w| whole_local(w) == Some(*l)),
            Some(Term::Len(_)) => {
                let i = self.t.lens.iter().position(|(_, node)| *node == n).expect("a length term is in `lens`");
                writes.iter().any(|w| self.hits_len(&self.placed(w), i))
            }
            Some(Term::Hidden(_)) | None => false,
        }
    }

    /// Whether `w` may change length term number `i` (in `Tracked::lens`): a write of a place it
    /// may be inside, a new value of one of its index locals, or pointing a loop element it is
    /// inside elsewhere.
    pub(super) fn hits_len(&self, w: &Placed, i: usize) -> bool {
        let key = &self.t.lens[i].0;
        match (w.w, &w.full) {
            (Write::Local(l), _) => key.indexed_by(*l) || self.homes.through(key.root, *l),
            (Write::Place(p, _), Some(full)) => (p.path.is_empty() && key.indexed_by(p.root)) || self.homes.meets(full, &self.len_full[i]),
            (Write::Place(..), None) => true,
        }
    }
}
