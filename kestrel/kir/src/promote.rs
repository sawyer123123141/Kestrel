//! Keeps the fields of an exclusive place in registers for a region of code, so a field written
//! there isn't read back from memory. The places are pointer parameters of plain type (`var self`,
//! a `var` number, a read-only struct), for the whole body, and loop elements of plain type
//! (`for p in ps`, a `par for` item), from their `Bind` to the end of the loop body. Each field the
//! region uses is loaded once at its start, and each field it stores is written back at every
//! way out: the end, `return`, and a `break` or `continue` of the element's loop.
//!
//! Only one route reaches such a place while the region runs: the checker refuses a `var`
//! argument whose place overlaps another argument's in the same call, and there are no
//! global variables that change. A region that reaches the place another way (passing it on as
//! `var`, a `par for`, or for a loop element its collection or another element of it) is left
//! alone, and so is a loop element's region that returns after freeing memory (see
//! `returns_after_freeing`). A runtime panic loses the fields not yet written back, which
//! nothing can observe.

use std::collections::{HashMap, HashSet};

use crate::ir::*;
use crate::layout::StructLayout;
use crate::leaves::{field_path, leaf_name, leaf_place, leaves, load, under, Leaf};
use crate::split::{Rewriter, Split};
use crate::visit::{places, walk, walk_expr, Node};
use crate::visit_mut::{walk_stmt, VisitMut};

pub fn promote(m: &mut Module) {
    let Module { funcs, structs, .. } = m;
    for f in funcs.iter_mut().filter(|f| f.native.is_none()) {
        let homes = bind_roots(&f.body);
        let mut body = std::mem::take(&mut f.body);
        Elements { structs, locals: &mut f.locals, homes: &homes }.visit_stmts(&mut body);
        for &p in &f.params {
            if f.locals[p.0 as usize].kind == LocalKind::RefParam {
                body = region(structs, &mut f.locals, &homes, p, body, false);
            }
        }
        f.body = body;
    }
}

/// Each loop element's `Bind`: the root of the place it points at.
fn bind_roots(body: &[Stmt]) -> HashMap<LocalIdx, LocalIdx> {
    let mut out = HashMap::new();
    walk(body, &mut |n| {
        if let Node::Stmt(Stmt::Bind(l, p)) = n {
            out.insert(*l, p.root);
        }
    });
    out
}

/// The local whose memory `l` points into, following loop elements to what they belong to.
fn home(mut l: LocalIdx, homes: &HashMap<LocalIdx, LocalIdx>) -> LocalIdx {
    for _ in 0..homes.len() {
        match homes.get(&l) {
            Some(&up) => l = up,
            None => break,
        }
    }
    l
}

/// Promotes every loop element in the rest of the statement list after its `Bind`.
struct Elements<'a> {
    structs: &'a [StructLayout],
    locals: &'a mut Vec<Local>,
    homes: &'a HashMap<LocalIdx, LocalIdx>,
}

impl VisitMut for Elements<'_> {
    fn visit_stmts(&mut self, list: &mut Vec<Stmt>) {
        // Inner loops first.
        for s in list.iter_mut() {
            self.visit_stmt(s);
        }
        let mut i = 0;
        while i < list.len() {
            if let Stmt::Bind(l, _) = list[i] {
                let end = list[i + 1..].iter().position(|s| matches!(s, Stmt::Bind(m, _) if *m == l)).map_or(list.len(), |k| i + 1 + k);
                let rest: Vec<Stmt> = list.drain(i + 1..end).collect();
                let done = region(self.structs, self.locals, self.homes, l, rest, true);
                let n = done.len();
                list.splice(i + 1..i + 1, done);
                i += n;
            }
            i += 1;
        }
    }
}

/// `stmts` with `root`'s fields kept in locals, when `root` is plain and nothing else reaches
/// its memory there. `in_loop` says `stmts` is the rest of a loop body, which a `break` or
/// `continue` of that loop leaves.
fn region(
    structs: &[StructLayout],
    locals: &mut Vec<Local>,
    homes: &HashMap<LocalIdx, LocalIdx>,
    root: LocalIdx,
    stmts: Vec<Stmt>,
    in_loop: bool,
) -> Vec<Stmt> {
    let (name, ty) = (locals[root.0 as usize].name.clone(), locals[root.0 as usize].ty);
    let Some(all) = leaves(structs, ty) else { return stmts };
    if !exclusive(&stmts, root, homes) || (in_loop && returns_after_freeing(&stmts)) {
        return stmts;
    }
    let (used, dirty) = usage(&stmts, root, &all);
    if used.is_empty() {
        return stmts;
    }
    let leaf_locals: Vec<LocalIdx> = all
        .iter()
        .map(|leaf| {
            locals.push(Local { name: leaf_name(structs, &name, ty, leaf), ty: leaf.ty, kind: LocalKind::Value });
            LocalIdx(locals.len() as u32 - 1)
        })
        .collect();
    let mut body = stmts;
    let roots = HashMap::from([(root, Split { leaves: all.clone(), locals: leaf_locals.clone() })]);
    Rewriter { structs, locals, roots }.visit_stmts(&mut body);
    let at = Place::local(root);
    // The loads and write-backs can't fail, so they need no source line.
    let line = 0;
    let mut out: Vec<Stmt> = used
        .iter()
        .map(|&k| Stmt::Let(leaf_locals[k], Expr { kind: ExprKind::Load(leaf_place(&at, &all[k])), ty: all[k].ty, line }))
        .collect();
    if !dirty.is_empty() {
        let back: Vec<Stmt> = dirty.iter().map(|&k| Stmt::Assign(leaf_place(&at, &all[k]), load(leaf_locals[k], all[k].ty, line))).collect();
        let written: HashSet<LocalIdx> = dirty.iter().map(|&k| leaf_locals[k]).collect();
        Exits { back: &back, written: &written, locals, in_loop, depth: 0 }.visit_stmts(&mut body);
        // Dead after a final `return`, `break` or `continue`; codegen skips it then.
        body.extend(back);
    }
    out.extend(body);
    out
}

/// Whether `stmts` reach `root`'s memory only through plain reads and writes of its fields: it
/// isn't passed as `var`, captured by a `par for` or bound, and no other local into the same
/// memory (for a loop element: its collection, or another element of it) is used. An interface
/// value borrowing it counts as passing it as `var`.
fn exclusive(stmts: &[Stmt], root: LocalIdx, homes: &HashMap<LocalIdx, LocalIdx>) -> bool {
    let mine = home(root, homes);
    let mut ok = true;
    let refs = |args: &[Arg]| args.iter().any(|a| matches!(a, Arg::Ref(p) if p.root == root));
    walk(stmts, &mut |n| match n {
        Node::Place(p, _) if p.root != root && home(p.root, homes) == mine => ok = false,
        Node::Stmt(Stmt::Call(_, a, _) | Stmt::ParFor { args: a, .. }) | Node::Expr(Expr { kind: ExprKind::Call(_, a), .. }) if refs(a) => ok = false,
        Node::Stmt(Stmt::CallIndirect { args: a, .. } | Stmt::CallVirtual { args: a, .. })
        | Node::Expr(Expr { kind: ExprKind::CallIndirect { args: a, .. } | ExprKind::CallVirtual { args: a, .. }, .. })
            if refs(a) =>
        {
            ok = false
        }
        Node::Stmt(Stmt::Bind(_, p)) if p.root == root => ok = false,
        // An interface value borrowing the place (a call's argument, or the `Let` that inlining
        // makes of one) reaches it like a `var` argument does.
        Node::Expr(Expr { kind: ExprKind::MakeIface { data: IfaceData::Borrowed(p), .. }, .. }) if p.root == root => ok = false,
        // (A capture bound by `BindEnv` points into a closure's environment, which no local is.)
        _ => {}
    });
    ok
}

/// Whether `stmts` contain a `return` and also a statement that can free memory: the end of an
/// `arena`, a freed arena or GPU handle, or a call that takes something by `var` (the generated
/// functions that drop a list's elements do). Lowering puts a `return`'s cleanup right before
/// it, so a loop element's write-back placed there could land after the memory holding the
/// element is freed. (A pointer parameter points into the caller's memory, which no `return` of
/// this function frees, so only loop elements need this.)
fn returns_after_freeing(stmts: &[Stmt]) -> bool {
    let (mut returns, mut frees) = (false, false);
    let by_ref = |args: &[Arg]| args.iter().any(|a| matches!(a, Arg::Ref(_)));
    walk(stmts, &mut |n| match n {
        Node::Stmt(Stmt::Return(_)) => returns = true,
        Node::Stmt(Stmt::PopArena { free: true } | Stmt::FreeArena(_) | Stmt::DropGpu(_)) => frees = true,
        // (Copying a function value's environment, `CopyFnEnv`, or making one only allocates.)
        Node::Stmt(Stmt::Call(_, a, _)) | Node::Expr(Expr { kind: ExprKind::Call(_, a), .. }) if by_ref(a) => frees = true,
        Node::Stmt(Stmt::CallIndirect { args: a, .. }) | Node::Expr(Expr { kind: ExprKind::CallIndirect { args: a, .. }, .. }) if by_ref(a) => frees = true,
        // A method called through an interface value may free memory its value holds, and so may
        // a call given a borrowed interface value (through its `var self` methods).
        Node::Stmt(Stmt::CallVirtual { .. }) | Node::Expr(Expr { kind: ExprKind::CallVirtual { .. }, .. }) => frees = true,
        Node::Expr(Expr { kind: ExprKind::MakeIface { data: IfaceData::Borrowed(_), .. }, .. }) => frees = true,
        _ => {}
    });
    returns && frees
}

/// The leaves `stmts` read or write, and the ones they write, by index into the root's leaves.
fn usage(stmts: &[Stmt], root: LocalIdx, all: &[Leaf]) -> (Vec<usize>, Vec<usize>) {
    let (mut used, mut dirty) = (vec![false; all.len()], vec![false; all.len()]);
    places(stmts, &mut |p, written| {
        if p.root == root {
            for k in under(all, &field_path(&p.path).expect("a plain place's path is fields")) {
                used[k] = true;
                dirty[k] |= written;
            }
        }
    });
    let pick = |v: &[bool]| v.iter().enumerate().filter(|(_, &b)| b).map(|(k, _)| k).collect();
    (pick(&used), pick(&dirty))
}

/// Puts `back` (the write-backs) before every way out of a region.
struct Exits<'a> {
    back: &'a [Stmt],
    /// The locals of the fields written back: a `return` value that stores to one is computed first.
    written: &'a HashSet<LocalIdx>,
    locals: &'a mut Vec<Local>,
    in_loop: bool,
    /// Loops entered inside the region.
    depth: u32,
}

impl Exits<'_> {
    fn stores_written(&self, e: &Expr) -> bool {
        let mut found = false;
        walk_expr(e, &mut |n| {
            if let Node::Stmt(Stmt::Assign(p, _)) = n {
                found |= self.written.contains(&p.root);
            }
        });
        found
    }
}

impl VisitMut for Exits<'_> {
    fn visit_stmts(&mut self, list: &mut Vec<Stmt>) {
        let mut out = Vec::with_capacity(list.len());
        for mut s in std::mem::take(list) {
            self.visit_stmt(&mut s);
            match s {
                Stmt::Return(Some(e)) if self.stores_written(&e) => {
                    let kind = if e.ty.is_aggregate() { LocalKind::Addressed } else { LocalKind::Value };
                    self.locals.push(Local { name: "result".into(), ty: e.ty, kind });
                    let t = LocalIdx(self.locals.len() as u32 - 1);
                    let (ty, line) = (e.ty, e.line);
                    out.push(Stmt::Let(t, e));
                    out.extend(self.back.iter().cloned());
                    out.push(Stmt::Return(Some(load(t, ty, line))));
                }
                Stmt::Return(_) => {
                    out.extend(self.back.iter().cloned());
                    out.push(s);
                }
                Stmt::Break | Stmt::Continue if self.in_loop && self.depth == 0 => {
                    out.extend(self.back.iter().cloned());
                    out.push(s);
                }
                s => out.push(s),
            }
        }
        *list = out;
    }

    fn visit_stmt(&mut self, s: &mut Stmt) {
        // A loop's header (a `while`'s condition, a `for`'s bounds) runs before the loop starts, so a
        // `break` or `continue` there leaves the enclosing loop: only the body is one level deeper.
        match s {
            Stmt::While { cond, body } => {
                self.visit_expr(cond);
                self.depth += 1;
                self.visit_stmts(body);
                self.depth -= 1;
            }
            Stmt::ForRange { start, end, body, .. } => {
                self.visit_expr(start);
                self.visit_expr(end);
                self.depth += 1;
                self.visit_stmts(body);
                self.depth -= 1;
            }
            _ => walk_stmt(self, s),
        }
    }
}
