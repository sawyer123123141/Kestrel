//! Binds `parts[i]` once for a run of statements, so `promote` keeps its fields in registers
//! (spec decision 4 of the performance round, `docs/superpowers/specs/2026-11-01-perf-round-design.md`).
//! After inlining, `parts[i].step(dt)` is a run of places `parts[i].pos`, `parts[i].vel`, each
//! indexed (and checked) again. In a run of statements of one list where every place of a list
//! `R` is `R[i]...` with one index local `i`, this pass puts `Bind(e, R[i])` before the run and
//! rewrites the places to `e...`; `promote` then treats `e` as it treats a `for ... in` element.
//!
//! Why the run means one element all along, as indexing each place did:
//! - `R`'s header can't change in it: every place of `R` there is an element (no `add`, `clear`,
//!   assignment or `var` argument of `R` itself), `R` isn't bound or set again, and nothing there
//!   can hold `R` another way: no closure, interface value, indirect or interface call, `par for`
//!   or function value copy (decision 3's rule).
//! - Nothing else reaches `R`'s elements there: no place of another local pointing into the same
//!   memory (a loop element of `R`, or of the list `R` itself belongs to).
//! - `i` is a register local nothing in the run writes; a `let j = i` in the run (as `+=` makes
//!   for its index) makes `r[j]` the same element.
//!
//! So `R[i]` is in bounds everywhere in the run when it is at the first use. The `Bind` carries
//! that use's check and line, and stands right before the statement holding it, which uses `R[i]`
//! unconditionally and does nothing else that can fail or that anything can see: a bad index
//! fails on the same line, with the same index, after the same output.

use std::collections::{HashMap, HashSet};

use crate::ir::*;
use crate::range::show::place_text;
use crate::range::sites::{checks_of, Arith};
use crate::visit::{walk, Node};
use crate::visit_mut::{walk_place, walk_stmt, VisitMut};

/// The fewest uses of `R[i]` a run needs to be bound: one use gains nothing.
const MIN_USES: usize = 2;

/// The home of every parameter of a `par for` body, which may point into one another.
const PAR_HOME: LocalIdx = LocalIdx(u32::MAX);

/// Binds each run of `R[i]` places in `m` (see the module comment). `optimize` runs it after
/// inlining and before `scalarize`.
pub fn bind_indexes(m: &mut Module) {
    for fi in 0..m.funcs.len() {
        if m.funcs[fi].native.is_some() {
            continue;
        }
        let mut body = std::mem::take(&mut m.funcs[fi].body);
        let par: HashSet<LocalIdx> = if m.par_bodies.contains(&FuncIdx(fi as u32)) { m.funcs[fi].params.iter().copied().collect() } else { HashSet::new() };
        let homes = bind_roots(&body);
        Binder { m, f: fi, homes, par }.visit_stmts(&mut body);
        m.funcs[fi].body = body;
    }
}

/// Each bound local: the root of the place it points into.
fn bind_roots(body: &[Stmt]) -> HashMap<LocalIdx, LocalIdx> {
    let mut out = HashMap::new();
    walk(body, &mut |n| match n {
        Node::Stmt(Stmt::Bind(l, p)) | Node::Stmt(Stmt::Let(l, Expr { kind: ExprKind::MakeIface { data: IfaceData::Borrowed(p), .. }, .. })) => {
            out.insert(*l, p.root);
        }
        _ => {}
    });
    out
}

/// The local an index reads, when it is a plain read of one.
fn index_local(at: &Expr) -> Option<LocalIdx> {
    match &at.kind {
        ExprKind::Load(q) if q.path.is_empty() => Some(q.root),
        _ => None,
    }
}

/// Whether `p` is `r[i]...` for an `i` in `is` (locals holding one value).
fn is_use(p: &Place, r: LocalIdx, is: &[LocalIdx]) -> bool {
    p.root == r && matches!(p.path.first(), Some(Proj::Index { at, .. }) if index_local(at).is_some_and(|l| is.contains(&l)))
}

struct Binder<'a> {
    m: &'a mut Module,
    f: usize,
    homes: HashMap<LocalIdx, LocalIdx>,
    /// The parameters of a `par for` body (empty for any other function).
    par: HashSet<LocalIdx>,
}

impl Binder<'_> {
    /// The local whose memory `l` points into, following bound locals to what they belong to.
    fn home(&self, mut l: LocalIdx) -> LocalIdx {
        for _ in 0..=self.homes.len() {
            match self.homes.get(&l) {
                Some(&up) => l = up,
                None => break,
            }
        }
        if self.par.contains(&l) {
            PAR_HOME
        } else {
            l
        }
    }

    /// The `(R, i)` pairs `s` uses: list or array locals indexed by a register local.
    fn candidates(&self, s: &Stmt) -> Vec<(LocalIdx, LocalIdx)> {
        let locals = &self.m.funcs[self.f].locals;
        let mut out = Vec::new();
        walk(std::slice::from_ref(s), &mut |n| {
            let Node::Place(p, _) = n else { return };
            let Some(Proj::Index { at, .. }) = p.path.first() else { return };
            let Some(i) = index_local(at) else { return };
            let r_ok = matches!(locals[p.root.0 as usize].ty, KTy::List(_) | KTy::Array(_));
            let i_ok = locals[i.0 as usize].kind == LocalKind::Value && at.ty.is_int();
            if r_ok && i_ok && !out.contains(&(p.root, i)) {
                out.push((p.root, i));
            }
        });
        out
    }

    /// Whether `s` can belong to a run of `r[i]`, `i` any of `is`: see the module comment.
    fn compatible(&self, s: &Stmt, r: LocalIdx, is: &[LocalIdx]) -> bool {
        let home = self.home(r);
        let mut ok = true;
        walk(std::slice::from_ref(s), &mut |n| match n {
            Node::Place(p, written) => {
                if p.root == r {
                    ok &= is_use(p, r, is);
                } else if is.contains(&p.root) {
                    ok &= !written;
                } else if self.home(p.root) == home {
                    ok = false;
                }
            }
            Node::Stmt(Stmt::Let(l, _) | Stmt::Bind(l, _) | Stmt::BindGlobal(l, _) | Stmt::BindEnv { local: l, .. } | Stmt::ForRange { var: l, .. })
                if *l == r || is.contains(l) =>
            {
                ok = false
            }
            // A closure, an interface value or a `par for` could hold `R`; a `@prove` or `@assume`
            // block keeps its checks inside it.
            Node::Stmt(
                Stmt::ParFor { .. }
                | Stmt::CallIndirect { .. }
                | Stmt::CallVirtual { .. }
                | Stmt::CopyFnEnv(_)
                | Stmt::CopyIface(_)
                | Stmt::Prove { .. }
                | Stmt::Assume { .. },
            ) => ok = false,
            Node::Expr(Expr { kind: ExprKind::CallIndirect { .. } | ExprKind::CallVirtual { .. } | ExprKind::MakeFn { .. } | ExprKind::MakeIface { .. }, .. }) => {
                ok = false
            }
            // A whole element moved out is a removing loop's item, which that loop keeps indexed.
            Node::Expr(Expr { kind: ExprKind::Move(p), .. }) if p.root == r && p.path.len() == 1 => ok = false,
            _ => {}
        });
        ok
    }

    /// The local `s` sets, when it is `let j = i` with `i` one of `is` and `j` a new register local.
    fn copy_of(&self, s: &Stmt, is: &[LocalIdx]) -> Option<LocalIdx> {
        let Stmt::Let(j, e) = s else { return None };
        let fresh = !is.contains(j) && self.m.funcs[self.f].locals[j.0 as usize].kind == LocalKind::Value;
        (fresh && index_local(e).is_some_and(|l| is.contains(&l))).then_some(*j)
    }

    /// Binds the run of `list` starting at `s`, if one does, and returns where scanning goes on.
    fn try_run(&mut self, list: &mut Vec<Stmt>, s: usize) -> Option<usize> {
        for (r, i) in self.candidates(&list[s]) {
            let mut is = vec![i];
            // `let i = k` right before the run: `k` is the same index in it.
            if let Some(Stmt::Let(_, e)) = s.checked_sub(1).map(|b| &list[b]).filter(|b| matches!(b, Stmt::Let(l, _) if *l == i)) {
                if let Some(k) = index_local(e).filter(|&k| k != i && self.m.funcs[self.f].locals[k.0 as usize].kind == LocalKind::Value) {
                    is.push(k);
                }
            }
            if !self.compatible(&list[s], r, &is) {
                continue;
            }
            let Some((at, check)) = first_use(&list[s], r, &is) else { continue };
            let mut end = s + 1;
            while end < list.len() {
                // `let j = i` (as `+=` makes for its index) gives `i` another name in the run.
                if let Some(j) = self.copy_of(&list[end], &is) {
                    is.push(j);
                } else if !self.compatible(&list[end], r, &is) {
                    break;
                }
                end += 1;
            }
            let counts: Vec<usize> = list[s..end].iter().map(|st| count_uses(st, r, &is)).collect();
            let last = s + counts.iter().rposition(|&n| n > 0).expect("the first statement uses it");
            // `promote` writes the element back at the end of the list: never after memory is freed.
            if counts.iter().sum::<usize>() < MIN_USES || frees(&list[s..]) {
                continue;
            }
            let bound = Place { root: r, path: vec![Proj::Index { at, check }] };
            let f = &self.m.funcs[self.f];
            let name = place_text(self.m, f, &bound, 1);
            let (KTy::List(el) | KTy::Array(el)) = f.locals[r.0 as usize].ty else { unreachable!("a candidate is a list or an array") };
            let ty = self.m.elems[el.0 as usize];
            let locals = &mut self.m.funcs[self.f].locals;
            locals.push(Local { name, ty, kind: LocalKind::RefParam });
            let e = LocalIdx(locals.len() as u32 - 1);
            let mut rw = Rebind { r, is, e };
            for st in &mut list[s..=last] {
                rw.visit_stmt(st);
            }
            list.insert(s, Stmt::Bind(e, bound));
            self.homes.insert(e, r);
            return Some(last + 2);
        }
        None
    }
}

impl VisitMut for Binder<'_> {
    /// Binds this list's runs first, then looks inside each statement.
    fn visit_stmts(&mut self, list: &mut Vec<Stmt>) {
        let mut s = 0;
        while s < list.len() {
            s = self.try_run(list, s).unwrap_or(s + 1);
        }
        for st in list.iter_mut() {
            walk_stmt(self, st);
        }
    }
}

/// The index and check the `Bind` of a run of `r[i]` starting at `s` takes, from `s`'s uses: when
/// `s` uses `r[i]` unconditionally, all on one line, and does nothing else that can fail or be
/// seen. `None` otherwise.
fn first_use(s: &Stmt, r: LocalIdx, is: &[LocalIdx]) -> Option<(Expr, IndexCheck)> {
    if !quiet(s, r, is) || !sure_stmt(s, &|p| is_use(p, r, is)) {
        return None;
    }
    let mut uses: Vec<(Expr, IndexCheck)> = Vec::new();
    walk(std::slice::from_ref(s), &mut |n| {
        if let Node::Place(p, _) = n {
            if let (true, Some(Proj::Index { at, check })) = (is_use(p, r, is), p.path.first()) {
                uses.push((at.clone(), *check));
            }
        }
    });
    let (at, first) = uses.first()?.clone();
    if uses.iter().any(|(a, _)| a.line != at.line) {
        return None;
    }
    let check = if uses.iter().any(|(_, c)| *c == IndexCheck::Checked) { IndexCheck::Checked } else { first };
    Some((at, check))
}

/// How many places of `s` are `r[i]...`, `i` any of `is`.
fn count_uses(s: &Stmt, r: LocalIdx, is: &[LocalIdx]) -> usize {
    let mut n = 0;
    walk(std::slice::from_ref(s), &mut |node| {
        if let Node::Place(p, _) = node {
            n += usize::from(is_use(p, r, is));
        }
    });
    n
}

/// Whether `stmts` free memory: the end of an `arena`, a freed arena or a dropped GPU object.
fn frees(stmts: &[Stmt]) -> bool {
    let mut out = false;
    walk(stmts, &mut |n| {
        if let Node::Stmt(Stmt::PopArena { free: true } | Stmt::FreeArena(_) | Stmt::DropGpu(_)) = n {
            out = true;
        }
    });
    out
}

/// Whether `s` does nothing that can fail or that anything can see, but use `r[i]`: only `let`s,
/// assignments and `if`s, of operations with no check, no call, no allocation and no random draw.
fn quiet(s: &Stmt, r: LocalIdx, is: &[LocalIdx]) -> bool {
    let mut ok = true;
    walk(std::slice::from_ref(s), &mut |n| match n {
        Node::Stmt(Stmt::Let(..) | Stmt::Assign(..) | Stmt::If { .. }) => {}
        Node::Stmt(_) => ok = false,
        Node::Expr(e) => {
            ok &= checks_of(e, Arith::Checked).is_empty()
                && matches!(
                    e.kind,
                    ExprKind::Int(_)
                        | ExprKind::Float(_)
                        | ExprKind::Bool(_)
                        | ExprKind::StrLit(_)
                        | ExprKind::NoneLit
                        | ExprKind::Load(_)
                        | ExprKind::Move(_)
                        | ExprKind::Tag(_)
                        | ExprKind::Len(_)
                        | ExprKind::Unary(..)
                        | ExprKind::Binary(..)
                        | ExprKind::And(..)
                        | ExprKind::Or(..)
                        | ExprKind::Cast(_)
                        | ExprKind::Math(..)
                        | ExprKind::StructLit(..)
                        | ExprKind::VariantLit(..)
                        | ExprKind::FieldOf(..)
                        | ExprKind::VecLit(_)
                        | ExprKind::Splat(_)
                        | ExprKind::Shuffle(..)
                        | ExprKind::LaneOf(..)
                        | ExprKind::Seq(..)
                        | ExprKind::If { .. }
                        | ExprKind::Some(_)
                )
        }
        Node::Place(p, _) => {
            for (k, step) in p.path.iter().enumerate() {
                match step {
                    Proj::Index { check: IndexCheck::Checked, .. } if !(k == 0 && is_use(p, r, is)) => ok = false,
                    Proj::MapKey { .. } | Proj::MapValue { .. } => ok = false,
                    _ => {}
                }
            }
        }
    });
    ok
}

/// Whether running `s` (a quiet statement) always reaches a place `u` accepts: one outside the
/// arms of an `if` and the right side of `and` and `or`.
fn sure_stmt(s: &Stmt, u: &impl Fn(&Place) -> bool) -> bool {
    match s {
        Stmt::Let(_, e) => sure_expr(e, u),
        Stmt::Assign(p, e) => sure_expr(e, u) || sure_place(p, u),
        Stmt::If { cond, .. } => sure_expr(cond, u),
        _ => false,
    }
}

fn sure_expr(e: &Expr, u: &impl Fn(&Place) -> bool) -> bool {
    match &e.kind {
        ExprKind::Load(p) | ExprKind::Move(p) | ExprKind::Tag(p) | ExprKind::Len(p) => sure_place(p, u),
        ExprKind::Seq(stmts, v) => stmts.iter().any(|s| sure_stmt(s, u)) || sure_expr(v, u),
        ExprKind::If { cond: x, .. }
        | ExprKind::And(x, _)
        | ExprKind::Or(x, _)
        | ExprKind::Unary(_, x)
        | ExprKind::Cast(x)
        | ExprKind::FieldOf(x, _)
        | ExprKind::Splat(x)
        | ExprKind::Shuffle(x, _)
        | ExprKind::LaneOf(x, _)
        | ExprKind::Some(x) => sure_expr(x, u),
        ExprKind::Binary(_, a, b) => sure_expr(a, u) || sure_expr(b, u),
        ExprKind::StructLit(_, xs) | ExprKind::VariantLit(_, _, xs) | ExprKind::VecLit(xs) | ExprKind::Math(_, xs) => xs.iter().any(|x| sure_expr(x, u)),
        _ => false,
    }
}

fn sure_place(p: &Place, u: &impl Fn(&Place) -> bool) -> bool {
    u(p) || p.path.iter().filter_map(Proj::at).any(|at| sure_expr(at, u))
}

/// Rewrites every `r[i]...` place, `i` any of `is`, to `e...`.
struct Rebind {
    r: LocalIdx,
    is: Vec<LocalIdx>,
    e: LocalIdx,
}

impl VisitMut for Rebind {
    fn visit_place(&mut self, p: &mut Place) {
        if is_use(p, self.r, &self.is) {
            p.root = self.e;
            p.path.remove(0);
        }
        walk_place(self, p);
    }
}
