//! What an argument changes while it's evaluated. Earlier arguments that
//! were passed by pointer would see those changes (or point into memory
//! the change moved), so such calls are rejected; so are an `if` or `match`
//! of places passed that way, and the parts of an expression (`a + f(var a)`)
//! kept until the last has run (`check_kept_parts`). The same walk gives the
//! call exclusivity rule (`exclusive`) the places changed and the values
//! moved inside a call's arguments.

use kestrel_syntax::ast::{Arg, Block, Expr, ExprKind, InterpPart, Stmt};

use super::exclusive::Changes;
use super::regions::{tail, PathKey};
use super::BodyChecker;
use crate::tables::{LocalId, Res};
use crate::ty::Ty;

impl<'a> BodyChecker<'a> {
    /// The variables changed by calls and assignments anywhere inside `e`.
    pub(crate) fn changed_inside(&self, e: &Expr, out: &mut Vec<LocalId>) {
        let mut changes = Changes::default();
        self.changes_in(e, &mut changes);
        out.extend(changes.paths.iter().map(|p| p.root));
    }

    /// The places changed by calls and assignments anywhere inside `e`, and the values moved
    /// there (see `Changes`).
    pub(crate) fn changes_in(&self, e: &Expr, out: &mut Changes) {
        match &e.kind {
            ExprKind::Call { args, .. } => self.changes_by_args(args, out),
            ExprKind::MethodCall { receiver, args, .. } => {
                let res = self.t.res.get(&e.id);
                // A user method, or an interface method called through an interface value.
                let changes_receiver = match (self.t.callee(res), res) {
                    (Some(f), _) => self.t.func(f).params.first().is_some_and(|p| p.is_var),
                    (None, Some(Res::ListAdd | Res::MapRemove)) => true,
                    (None, Some(Res::List(m))) => m.changes_receiver(),
                    _ => false,
                };
                if changes_receiver {
                    self.note_changed(receiver, out);
                }
                self.changes_in(receiver, out);
                self.changes_by_args(args, out);
            }
            ExprKind::TypeCall { args, .. } => self.changes_by_args(args, out),
            ExprKind::CallValue { callee, args } => {
                self.changes_in(callee, out);
                self.changes_by_args(args, out);
            }
            ExprKind::Unary { expr, .. } | ExprKind::Try(expr) => self.changes_in(expr, out),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.changes_in(lhs, out);
                self.changes_in(rhs, out);
            }
            ExprKind::Field { base, .. } => self.changes_in(base, out),
            ExprKind::Index { base, index } => {
                self.changes_in(base, out);
                self.changes_in(index, out);
            }
            ExprKind::ArrayLit(items) => items.iter().for_each(|i| self.changes_in(i, out)),
            ExprKind::MapLit(entries) => {
                for (k, v) in entries {
                    self.changes_in(k, out);
                    self.changes_in(v, out);
                }
            }
            ExprKind::Interp(parts) => {
                for p in parts {
                    if let InterpPart::Expr(x) = p {
                        self.changes_in(x, out);
                    }
                }
            }
            ExprKind::If { cond, then_block, else_block } => {
                self.changes_in(cond, out);
                self.changes_in_block(then_block, out);
                if let Some(b) = else_block {
                    self.changes_in_block(b, out);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.changes_in(scrutinee, out);
                for a in arms {
                    self.changes_in_block(&a.body, out);
                }
            }
            // A move of an owning variable (into a `let`, a construction, a list...).
            ExprKind::Ident(_) if self.t.moves.contains(&e.id) => out.moved_roots.extend(self.place_root(e)),
            // Building a closure runs none of its body, and it changes nothing it captures.
            ExprKind::Closure(_) => {}
            ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) | ExprKind::Ident(_) | ExprKind::TypeApply { .. } => {}
        }
    }

    /// What a call's arguments change: its `var` arguments, and what is changed or moved inside
    /// each argument. An owning value passed to a parameter that isn't `var` counts as moved into
    /// the call (see `exclusive`).
    pub(crate) fn changes_by_args(&self, args: &[Arg], out: &mut Changes) {
        for a in args {
            if a.is_var {
                self.note_changed(&a.value, out);
            } else {
                self.note_passed(&a.value, out);
            }
            self.changes_in(&a.value, out);
        }
    }

    fn changes_in_block(&self, b: &Block, out: &mut Changes) {
        for s in &b.stmts {
            match s {
                Stmt::Let { value, else_block: Some(b), .. } => {
                    self.changes_in(value, out);
                    self.changes_in_block(b, out);
                }
                Stmt::Let { value, .. } | Stmt::Expr(value) | Stmt::Return { value: Some(value), .. } => self.changes_in(value, out),
                Stmt::Assign { target, value, .. } => {
                    self.note_changed(target, out);
                    self.changes_in(value, out);
                }
                Stmt::While { cond, body, .. } => {
                    self.changes_in(cond, out);
                    self.changes_in_block(body, out);
                }
                Stmt::ForRange { start, end, body, .. } => {
                    self.changes_in(start, out);
                    self.changes_in(end, out);
                    self.changes_in_block(body, out);
                }
                Stmt::ForIn { iter, body, .. } => {
                    self.changes_in(iter, out);
                    self.changes_in_block(body, out);
                }
                Stmt::Arena { body, .. } => self.changes_in_block(body, out),
                Stmt::In { arena, body, .. } => {
                    self.changes_in(arena, out);
                    self.changes_in_block(body, out);
                }
                Stmt::Assume { cond, .. } => self.changes_in(cond, out),
                Stmt::Break(_) | Stmt::Continue(_) | Stmt::Return { value: None, .. } => {}
                // Removing the loop's item changes its list: all of the list's variable, since the
                // loop it belongs to has been left.
                Stmt::Remove { span, .. } => {
                    if let Some(root) = self.t.removes.get(span).and_then(|&l| self.t.local(l).alias) {
                        out.paths.push(PathKey { root, steps: Vec::new() });
                    }
                }
            }
        }
    }

    /// `passed` were passed by pointer (`var`, or a read-only aggregate)
    /// before `later` is evaluated: none of them may be changed by it.
    pub(crate) fn check_changed_later(&mut self, passed: &[&Expr], later: &[&Expr]) {
        self.refuse_changed_later(passed, later, Later::Argument);
    }

    /// Each of `parts`, evaluated in order, that holds heap memory is kept as it is (a header that
    /// still points at its place's memory) until the last part has run: no later part may change it.
    /// For `print`'s arguments, the sides of an operator and the parts of an interpolated string.
    pub(crate) fn check_kept_parts(&mut self, parts: &[&Expr], what: Later) {
        for (i, p) in parts.iter().enumerate() {
            if self.t.has_heap(self.t.ty(p.id)) {
                self.refuse_changed_later(&[p], &parts[i + 1..], what);
            }
        }
    }

    fn refuse_changed_later(&mut self, passed: &[&Expr], later: &[&Expr], what: Later) {
        let mut changed = Vec::new();
        for e in later {
            self.changed_inside(e, &mut changed);
        }
        let mut places = Vec::new();
        for p in passed {
            self.read_places(p, &mut places);
        }
        for p in places {
            let Some(root) = self.place_root(p) else { continue };
            if changed.contains(&root) {
                let name = self.t.local(root).name.clone();
                let (msg, help) = match what {
                    Later::Argument => (
                        format!("`{name}` is passed here, but a later argument changes it"),
                        "compute that argument into a variable before the call",
                    ),
                    Later::Operand => (
                        format!("`{name}` is read here, but a later part of this expression changes it"),
                        "compute that part into a variable first",
                    ),
                };
                self.error_help(p.span, msg, help);
            }
        }
    }

    /// The places `e`'s value still points into once it is evaluated: `e` itself when it is a
    /// place; else, when its value holds heap memory, the places an `if`'s or a `match`'s arms give
    /// (that value is kept shallowly, so it points at the arm's place's memory), and the map a
    /// `get` reads from. Other values are made new, or hold no heap memory.
    fn read_places<'e>(&self, e: &'e Expr, out: &mut Vec<&'e Expr>) {
        if self.place_root(e).is_some() {
            out.push(e);
            return;
        }
        if !self.t.has_heap(self.t.ty(e.id)) {
            return;
        }
        match &e.kind {
            ExprKind::If { then_block, else_block, .. } => {
                for b in std::iter::once(then_block).chain(else_block) {
                    if let Some(t) = tail(b) {
                        self.read_places(t, out);
                    }
                }
            }
            ExprKind::Match { arms, .. } => {
                for t in arms.iter().filter_map(|a| tail(&a.body)) {
                    self.read_places(t, out);
                }
            }
            // A field or an item of such a value (`(if c { a } else { b }).items[0]`) points into
            // the same memory. (A string's character is a new string.)
            ExprKind::Field { base, .. } => self.read_places(base, out),
            ExprKind::Index { base, .. } if self.t.ty(base.id) != Ty::Str => self.read_places(base, out),
            // `m.get(k, d)` reads the entry shallowly, so its value points into the map's memory.
            // (The default is copied in its turn. From a borrowed map the value is copied too,
            // which this doesn't tell apart: such a call is refused although it is safe.)
            ExprKind::MethodCall { receiver, name, .. } if name == "get" && self.t.res.get(&e.id) == Some(&Res::MapRead) => {
                self.read_places(receiver, out)
            }
            _ => {}
        }
    }
}

/// What `check_changed_later` and `check_kept_parts` call the values a later one may not change.
#[derive(Clone, Copy)]
pub(crate) enum Later {
    /// A call's argument or receiver.
    Argument,
    /// A side of an operator, or a part of an interpolated string.
    Operand,
}
