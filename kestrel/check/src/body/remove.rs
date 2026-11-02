//! `remove b`: removing the current item of a `for b in xs` loop over a list. The loop compacts
//! the list as it goes (see KIR's `lower::remove`), so its items move while it runs: the body may
//! use the list only through the item. After `remove b`, `b` is gone, like a moved variable.

use std::collections::{HashMap, HashSet};

use kestrel_syntax::ast::{Arg, Block, Expr, ExprKind, InterpPart, Stmt};
use kestrel_syntax::Span;

use super::lists::{covers, Step};
use super::moves::MoveAt;
use super::place::Immutable;
use super::regions::{Ctx, PathKey};
use super::BodyChecker;
use crate::tables::LocalId;

/// What a `for ... in` loop runs over, as far as `remove` is concerned.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Over {
    List,
    Array,
    Map,
    Str,
    /// Something that can't be looped over (already reported).
    Other,
}

/// A `for ... in` loop being checked.
struct ItemLoop {
    item: LocalId,
    over: Over,
    /// The list's variable (aliases followed) and its place in it, when the loop runs over a place.
    list: Option<(LocalId, Vec<Step>)>,
    /// The list as written, for `match` guards.
    key: Option<PathKey>,
    /// The list as the loop's errors name it.
    name: String,
    /// Whether the body removes the item.
    removing: bool,
    /// The item and the views into it declared in the body: using them doesn't use the list.
    item_views: HashSet<LocalId>,
    /// Places in the list's variable that the body uses, with where.
    uses: Vec<(Span, Vec<Step>)>,
}

#[derive(Default)]
pub(crate) struct Removing {
    loops: Vec<ItemLoop>,
    /// The place each view (a loop item, `match` binding or `let ... else` view) reads.
    pub(crate) views: HashMap<LocalId, (LocalId, Vec<Step>)>,
}

impl Removing {
    /// How many `for ... in` loops are open.
    pub(crate) fn loops_len(&self) -> usize {
        self.loops.len()
    }
}

impl<'a> BodyChecker<'a> {
    /// Starts the body of a `for ... in` loop over `iter` whose item is `item`. A loop over a list
    /// whose body says `remove <item>` anywhere is a removing loop (`remove` in a loop over anything
    /// else is refused when it is checked).
    pub(crate) fn enter_item_loop(&mut self, item: LocalId, over: Over, iter: &Expr, body: &Block) {
        let removing = over == Over::List && block_removes(body, &self.t.local(item).name);
        let list = self.list_place(iter);
        if removing && list.is_some() {
            self.t.removing_loops.insert(iter.id);
        }
        let name = match self.for_in_lists.iter().rposition(|x| x.item == item) {
            Some(i) => self.loop_list_name(i),
            None => String::new(),
        };
        let key = self.path_key(iter);
        let item_views = HashSet::from([item]);
        self.removing.loops.push(ItemLoop { item, over, list, key, name, removing, item_views, uses: Vec::new() });
    }

    /// Ends the loop `enter_item_loop` started. A removing loop's body may not use its list.
    pub(crate) fn leave_item_loop(&mut self) {
        let lp = self.removing.loops.pop().expect("a loop is open");
        let (true, Some((_, list))) = (lp.removing, &lp.list) else { return };
        let mut uses = lp.uses;
        uses.sort_by_key(|(span, _)| (span.start, span.end));
        uses.dedup_by_key(|(span, _)| *span);
        let item = self.t.local(lp.item).name.clone();
        for (span, path) in &uses {
            // Part of a longer use (`w` in `w.score`): the longer one decides.
            let inner = uses.iter().any(|(s, p)| s != span && s.start <= span.start && span.end <= s.end && p.starts_with(path));
            let overlaps = path.iter().zip(list).all(|(a, b)| a.is_none() || b.is_none() || a == b);
            if !inner && overlaps {
                let name = &lp.name;
                let msg = format!(
                    "this loop removes items from `{name}`, so its items move while it runs; use the loop's item (`{item}`), or use `{name}` after the loop"
                );
                self.error(*span, msg);
            }
        }
    }

    /// A place (a variable, field or element) the body uses.
    pub(crate) fn note_remove_read(&mut self, e: &Expr) {
        if !self.removing.loops.iter().any(|l| l.removing) {
            return;
        }
        let (Some(key), Some((root, path))) = (self.path_key(e), self.list_place(e)) else { return };
        for lp in self.removing.loops.iter_mut().filter(|l| l.removing) {
            if !lp.item_views.contains(&key.root) && lp.list.as_ref().is_some_and(|(r, _)| *r == root) {
                lp.uses.push((e.span, path.clone()));
            }
        }
    }

    /// A view declared over `of`: it reads the place `of` names, and when that is inside a loop's
    /// item, using it uses the item.
    pub(crate) fn note_remove_view(&mut self, view: LocalId, of: &Expr) {
        if let Some(p) = self.list_place(of) {
            self.removing.views.insert(view, p);
        }
        let Some(key) = self.path_key(of) else { return };
        for lp in &mut self.removing.loops {
            if lp.item_views.contains(&key.root) {
                lp.item_views.insert(view);
            }
        }
    }

    /// A call of a function that doesn't exist. `remove(b)`, with `b` a loop's item, is a slip
    /// for the statement `remove b`.
    pub(crate) fn unknown_function(&mut self, span: Span, name: &str, args: &[Arg]) {
        let msg = format!("unknown function `{name}`");
        let item = match args {
            [a] if name == "remove" => match &a.value.kind {
                ExprKind::Ident(b) => self.lookup_local(b).filter(|l| self.removing.loops.iter().any(|lp| lp.item == *l)).map(|_| b),
                _ => None,
            },
            _ => None,
        };
        match item {
            Some(b) => self.error_help(span, msg, format!("to remove the loop's item, write `remove {b}` without parentheses")),
            None => self.error(span, msg),
        }
    }

    /// `remove name`.
    pub(crate) fn check_remove(&mut self, name: &str, name_span: Span, span: Span) {
        let found = self.lookup_local(name).and_then(|l| self.removing.loops.iter().rposition(|lp| lp.item == l));
        let Some(i) = found else {
            let msg = format!("`remove` takes the item of a `for ... in` loop over a list; `{name}` isn't one");
            return self.error(name_span, msg);
        };
        // The item of a loop outside the closure this is in: the closure is a function of its own.
        if i < self.removing_floor() {
            return self.error(span, "`remove` can't leave this closure; it is a function of its own");
        }
        if self.par_inside_frame() {
            return self.error(span, "`remove` can't be used in a `par for`: its iterations run at the same time");
        }
        let lp = &self.removing.loops[i];
        let (item, key) = (lp.item, lp.key.clone());
        let msg = match lp.over {
            Over::List => None,
            Over::Array => Some(format!("arrays have a fixed length; use a `list[{}]` if items come and go", self.tn(self.t.local(item).ty))),
            Over::Map => Some("`remove` works in loops over a list; for a map, use `m.remove(k)`".to_string()),
            Over::Str => Some("`remove` works in loops over a list; strings can't change, so build a new one".to_string()),
            Over::Other => return,
        };
        if let Some(msg) = msg {
            return self.error(span, msg);
        }
        let Some((root, path)) = lp.list.clone() else {
            return self.error(span, "`remove` needs a loop over a list in a variable; store the list in a `var` first");
        };
        if !self.removable_from(root, span) {
            return;
        }
        if let Some(at) = self.for_in_lists.iter().rposition(|x| x.item == item) {
            if self.for_in_lists[..at].iter().any(|x| x.root == root && covers(&path, &x.path)) {
                let msg = format!("can't remove from `{}` here: an outer loop is also looping over it", self.loop_list_name(at));
                return self.error(span, msg);
            }
        }
        if let Some(binding) = key.and_then(|k| self.matched_by(root, &k)) {
            return self.matched_error(span, name, binding);
        }
        let views = &self.removing.loops[i].item_views;
        if self.ctxs.iter().any(|c| matches!(c, Ctx::Owned(p) if views.contains(&p.root))) {
            let msg = format!("can't remove `{name}` here: this `in` block is putting memory in `{name}`'s arena");
            return self.error_help(span, msg, "remove it after the block");
        }
        // A removing loop between here and the item's loop, over a list inside the item, would
        // compact that list on its way out, after the item is gone.
        let inner = self.removing.loops[i + 1..].iter().find(|lp| {
            lp.removing
                && lp.key.is_some()
                && (lp.key.as_ref().is_some_and(|k| views.contains(&k.root))
                    || lp.list.as_ref().is_some_and(|(r, p)| *r == root && p.len() > path.len() && p.starts_with(&path)))
        });
        if let Some(key) = inner.and_then(|lp| lp.key.clone()) {
            let msg = format!("can't remove `{name}` here: the loop over `{}` is removing items from it; leave that loop first", self.path_name(&key));
            return self.error(span, msg);
        }
        match self.moved.get(&item) {
            Some(at) if at.every => self.error(span, format!("`{name}` was removed earlier, so it can't be removed again")),
            Some(_) => self.error_help(
                span,
                format!("`{name}` may have been removed earlier, so it can't be removed again"),
                "it's removed on some paths to this line",
            ),
            None => {
                self.moved.insert(item, MoveAt { span, every: true, removed: true });
                self.t.removes.insert(span, item);
            }
        }
    }

    /// A use of `view`, which may read from inside a loop item (an inner loop's item over one of
    /// its lists, a `match` binding): not after the item was removed, which freed what it held.
    pub(crate) fn check_removed_view(&mut self, view: LocalId, span: Span) {
        let views_of = |lp: &&ItemLoop| lp.item != view && lp.item_views.contains(&view);
        let removed = |lp: &ItemLoop| self.moved.get(&lp.item).filter(|at| at.removed).map(|at| (lp.item, *at));
        let Some((item, at)) = self.removing.loops.iter().rev().filter(views_of).find_map(removed) else { return };
        let (name, item) = (self.t.local(view).name.clone(), self.t.local(item).name.clone());
        let was = if at.every { "was" } else { "may have been" };
        self.error(span, format!("`{name}` reads from `{item}`, which {was} removed earlier, so it can't be used here"));
    }

    /// Whether the list in `root` can change, as `add` needs; reports why not.
    fn removable_from(&mut self, root: LocalId, span: Span) -> bool {
        let name = self.t.local(root).name.clone();
        let (msg, help) = match self.immutable(root) {
            None => {
                self.mark_changed(root);
                return true;
            }
            Some(Immutable::Let) => (format!("can't remove from `{name}`: it was declared with `let`"), "declare it with `var`".to_string()),
            Some(Immutable::Param) => {
                (format!("can't remove from parameter `{name}`: it isn't `var`"), format!("declare it as `var {name}` to change the caller's list"))
            }
            Some(Immutable::SelfParam) => {
                ("can't remove from `self`: the method takes `self`, not `var self`".to_string(), "declare the method with `var self`".to_string())
            }
            Some(Immutable::Captured) => {
                self.error(span, Self::captured_message(&name));
                return false;
            }
        };
        self.error_help(span, msg, help);
        false
    }
}

/// Whether `b` says `remove name` anywhere, nested blocks included.
fn block_removes(b: &Block, name: &str) -> bool {
    b.stmts.iter().any(|s| stmt_removes(s, name))
}

fn stmt_removes(s: &Stmt, name: &str) -> bool {
    let e = |x: &Expr| expr_removes(x, name);
    let b = |x: &Block| block_removes(x, name);
    match s {
        Stmt::Remove { name: n, .. } => n == name,
        Stmt::Let { value, else_block, .. } => e(value) || else_block.as_ref().is_some_and(b),
        Stmt::Assign { target, value, .. } => e(target) || e(value),
        Stmt::While { cond, body, .. } => e(cond) || b(body),
        Stmt::ForRange { start, end, body, .. } => e(start) || e(end) || b(body),
        Stmt::ForIn { iter, body, .. } => e(iter) || b(body),
        Stmt::Arena { body, .. } => b(body),
        Stmt::In { arena, body, .. } => e(arena) || b(body),
        Stmt::Return { value: Some(v), .. } | Stmt::Expr(v) => e(v),
        Stmt::Assume { cond, .. } => e(cond),
        Stmt::Return { value: None, .. } | Stmt::Break(_) | Stmt::Continue(_) => false,
    }
}

fn expr_removes(x: &Expr, name: &str) -> bool {
    let e = |x: &Expr| expr_removes(x, name);
    let args = |a: &[Arg]| a.iter().any(|a| e(&a.value));
    match &x.kind {
        ExprKind::If { cond, then_block, else_block } => {
            e(cond) || block_removes(then_block, name) || else_block.as_ref().is_some_and(|b| block_removes(b, name))
        }
        ExprKind::Match { scrutinee, arms } => e(scrutinee) || arms.iter().any(|a| block_removes(&a.body, name)),
        ExprKind::Unary { expr, .. } | ExprKind::Try(expr) => e(expr),
        ExprKind::Binary { lhs, rhs, .. } => e(lhs) || e(rhs),
        ExprKind::Call { args: a, .. } | ExprKind::TypeCall { args: a, .. } => args(a),
        ExprKind::MethodCall { receiver, args: a, .. } => e(receiver) || args(a),
        ExprKind::CallValue { callee, args: a } => e(callee) || args(a),
        ExprKind::Field { base, .. } => e(base),
        ExprKind::Index { base, index } => e(base) || e(index),
        ExprKind::ArrayLit(items) => items.iter().any(e),
        ExprKind::MapLit(entries) => entries.iter().any(|(k, v)| e(k) || e(v)),
        ExprKind::Interp(parts) => parts.iter().any(|p| matches!(p, InterpPart::Expr(x) if e(x))),
        // A closure's `remove` is its own: it can't remove an outer loop's item (see `check_remove`).
        ExprKind::Closure(_) => false,
        ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) | ExprKind::Ident(_) | ExprKind::TypeApply { .. } => false,
    }
}
