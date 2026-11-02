//! Scopes: what must happen when control leaves a block (dropping its
//! owning locals, popping an arena it pushed) on every way out of it.

use kestrel_syntax::ast;

use super::assume::Ending;
use super::helpers::{has_heap, owning};
use super::FnLowerer;
use crate::ir::*;

#[derive(Clone, Default)]
pub(crate) struct Scope {
    /// Owning locals declared here, dropped on the way out.
    owning: Vec<LocalIdx>,
    /// An arena pushed for this scope: `Some(true)` for `arena { }` (freed
    /// at the end), `Some(false)` for `in x { }`.
    arena: Option<bool>,
    /// A loop body: `break` and `continue` leave it.
    is_loop: bool,
    /// A loop over a map: leaving it unlocks the map, restoring the lock
    /// count saved in the local.
    unlock: Option<(Place, LocalIdx)>,
    /// Runs last on every way out: the end of a loop over a list that removes its item compacts
    /// it (see `remove`).
    finish: Vec<Stmt>,
}

impl<'a> FnLowerer<'a> {
    /// The statements that leave `s`: drops (newest first), then the arena pop.
    fn cleanup(&mut self, s: &Scope) -> Vec<Stmt> {
        let mut out = Vec::new();
        for &l in s.owning.iter().rev() {
            let ty = self.locals[l.0 as usize].ty;
            out.extend(self.helpers.drop_stmts(Place::local(l), ty, self.t));
        }
        if let Some(free) = s.arena {
            out.push(Stmt::PopArena { free });
        }
        if let Some((map, saved)) = &s.unlock {
            let saved = Expr { kind: ExprKind::Load(Place::local(*saved)), ty: KTy::I64, line: 0 };
            out.push(Stmt::MapUnlock(map.clone(), saved));
        }
        out.extend(s.finish.iter().cloned());
        out
    }

    /// Lowers a loop over `map` with `lower`, so that `return` inside it
    /// unlocks the map. The caller unlocks it after the loop, where the
    /// loop's end and `break` both arrive.
    pub(crate) fn locked(&mut self, map: Place, saved: LocalIdx, lower: impl FnOnce(&mut Self) -> Vec<Stmt>) -> Vec<Stmt> {
        self.scopes.push(Scope { unlock: Some((map, saved)), ..Scope::default() });
        let stmts = lower(self);
        self.scopes.pop().expect("scopes balance");
        stmts
    }

    /// Lowers `lower` inside a scope whose way out runs `finish`. Only a `return` takes it: a
    /// `break` or `continue` stops at a loop inside. The caller runs `finish` where the normal way
    /// out arrives.
    pub(crate) fn guarded<T>(&mut self, finish: Vec<Stmt>, lower: impl FnOnce(&mut Self) -> T) -> T {
        self.scopes.push(Scope { finish, ..Scope::default() });
        let lowered = lower(self);
        self.scopes.pop().expect("scopes balance");
        lowered
    }

    /// Cleanups for jumping out of every scope up to the innermost loop
    /// body (`break`/`continue`), or out of all of them (`return`).
    fn exit_cleanup(&mut self, to_loop: bool) -> Vec<Stmt> {
        let mut out = Vec::new();
        for i in (0..self.scopes.len()).rev() {
            let s = self.scopes[i].clone();
            out.extend(self.cleanup(&s));
            if to_loop && s.is_loop {
                break;
            }
        }
        out
    }

    /// Whether leaving the innermost scope does anything (see `cleanup`).
    pub(super) fn scope_has_cleanup(&self) -> bool {
        let s = self.scopes.last().expect("inside a scope");
        !s.owning.is_empty() || s.arena.is_some() || s.unlock.is_some() || !s.finish.is_empty()
    }

    fn scoped_block(&mut self, block: &ast::Block, scope: Scope, ending: Ending) -> Vec<Stmt> {
        self.scopes.push(scope);
        let mut out = Vec::new();
        self.block_rest(&block.stmts, ending, &mut out);
        let s = self.scopes.pop().expect("scopes balance");
        out.extend(self.cleanup(&s));
        out
    }

    pub(crate) fn block_stmts(&mut self, block: &ast::Block) -> Vec<Stmt> {
        self.scoped_block(block, Scope::default(), Ending::Stmts)
    }

    pub(crate) fn loop_block(&mut self, block: &ast::Block) -> Vec<Stmt> {
        self.scoped_block(block, Scope { is_loop: true, ..Scope::default() }, Ending::Stmts)
    }

    /// A loop body that runs `finish` on every way out of each pass: its end, `continue`, `break`
    /// and `return`.
    pub(crate) fn loop_block_finishing(&mut self, block: &ast::Block, finish: Vec<Stmt>) -> Vec<Stmt> {
        self.scoped_block(block, Scope { is_loop: true, finish, ..Scope::default() }, Ending::Stmts)
    }

    /// A block used as a value: its statements and its tail value, which is
    /// `None` when the block can't finish normally.
    pub(crate) fn block_value(&mut self, block: &ast::Block) -> (Vec<Stmt>, Option<Expr>) {
        self.value_block(block, Ending::Value)
    }

    /// `block_value` for an arm (`Ending::Value`) or a function's body (`Ending::Body`).
    pub(crate) fn value_block(&mut self, block: &ast::Block, ending: Ending) -> (Vec<Stmt>, Option<Expr>) {
        self.scopes.push(Scope::default());
        let mut stmts = Vec::new();
        // A value computed inside an assumption is already a temporary of its own.
        let (value, settled) = self.block_rest(&block.stmts, ending, &mut stmts);
        let s = self.scopes.pop().expect("scopes balance");
        let cleanup = self.cleanup(&s);
        if cleanup.is_empty() || settled {
            stmts.extend(cleanup);
            return (stmts, value);
        }
        // The value may read what's about to be dropped, so compute it first:
        // a read of a place is copied here, into the current arena, and a fresh
        // value (a literal, say) is not. The temporary is read once, so a value
        // with heap memory moves out of it, which the caller sees as fresh too.
        let value = value.map(|v| {
            let v = self.owned(v, false);
            let spilled = self.spill(v, &mut stmts);
            match spilled.kind {
                ExprKind::Load(place) if has_heap(spilled.ty, self.t) => Expr { kind: ExprKind::Move(place), ..spilled },
                _ => spilled,
            }
        });
        stmts.extend(cleanup);
        (stmts, value)
    }

    /// Records a local declared in the current scope; an owning one is
    /// dropped when the scope ends.
    pub(crate) fn declared(&mut self, l: LocalIdx) {
        if owning(self.locals[l.0 as usize].ty, self.t) {
            self.scopes.last_mut().expect("inside a scope").owning.push(l);
        }
    }

    /// `arena { }` (no place: a fresh arena, freed at the end) or `in x { }`.
    pub(crate) fn arena_block(&mut self, arena: Option<&ast::Expr>, body: &ast::Block, out: &mut Vec<Stmt>) {
        let place = arena.map(|a| self.place_or_temp(a, out, true));
        let free = place.is_none();
        out.push(Stmt::PushArena(place, self.line(body.span)));
        let inner = self.scoped_block(body, Scope { arena: Some(free), ..Scope::default() }, Ending::Stmts);
        out.extend(inner);
    }

    /// Lowers `lower` inside a scratch arena, which the caller pushes before the
    /// statements `lower` gives and pops with the statements this gives back. The
    /// arena is a scope like `arena { }`'s while `lower` runs: a `return`, `break` or
    /// `continue` it lowers pops the arena on its way out, so none leaves it pushed.
    pub(crate) fn scratch<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> (T, Vec<Stmt>) {
        self.scopes.push(Scope { arena: Some(true), ..Scope::default() });
        let lowered = lower(self);
        let scope = self.scopes.pop().expect("scopes balance");
        (lowered, self.cleanup(&scope))
    }

    pub(crate) fn return_stmt(&mut self, value: Option<&ast::Expr>, out: &mut Vec<Stmt>) {
        let v = value.map(|v| {
            let v = self.expr(v);
            self.owned(v, true)
        });
        self.return_lowered(v, out);
    }

    /// A `return` of `v`, already lowered and safe to return (`owned`), from inside every scope.
    pub(crate) fn return_lowered(&mut self, mut v: Option<Expr>, out: &mut Vec<Stmt>) {
        // Unlocking only writes the maps' headers, so it goes first: a map
        // returned from inside a loop over it must leave unlocked.
        let (unlocks, cleanup): (Vec<Stmt>, Vec<Stmt>) =
            self.exit_cleanup(false).into_iter().partition(|s| matches!(s, Stmt::MapUnlock(..)));
        out.extend(unlocks);
        if !cleanup.is_empty() {
            v = v.map(|x| self.spill(x, out));
            out.extend(cleanup);
        }
        out.push(Stmt::Return(v));
    }

    pub(crate) fn jump(&mut self, stmt: Stmt, out: &mut Vec<Stmt>) {
        let cleanup = self.exit_cleanup(true);
        out.extend(cleanup);
        out.push(stmt);
    }

    /// `e`, evaluated only sometimes (the right side of `and` / `or`). It is a block of its own,
    /// so what it holds is dropped when it finishes; a block around it can't count on the
    /// temporaries having been made.
    pub(crate) fn conditional(&mut self, e: &ast::Expr) -> Expr {
        self.scopes.push(Scope::default());
        let v = self.expr(e);
        let scope = self.scopes.pop().expect("scopes balance");
        let cleanup = self.cleanup(&scope);
        if cleanup.is_empty() {
            return v;
        }
        let mut stmts = Vec::new();
        let v = self.spill(v, &mut stmts);
        stmts.extend(cleanup);
        let (ty, line) = (v.ty, v.line);
        Expr { kind: ExprKind::Seq(stmts, Box::new(v)), ty, line }
    }

    /// `target = value` where the target holds an `Arena` or a GPU object: the old value is
    /// dropped after the new one is computed (it may read the old one).
    pub(crate) fn assign_owning(&mut self, target: &ast::Expr, value: &ast::Expr, out: &mut Vec<Stmt>) {
        let v = self.value(value);
        let v = self.spill(v, out);
        let place = self.place_or_temp(target, out, true);
        let ty = v.ty;
        out.extend(self.helpers.drop_stmts(place.clone(), ty, self.t));
        out.push(Stmt::Assign(place, v));
    }
}
