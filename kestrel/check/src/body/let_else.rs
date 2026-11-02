//! `let x = opt else { ... }`: unwrapping an optional, or a `Result` (`let x = r else why { ... }`,
//! the language round's decision 10). When `opt` is a place that can't change, `x` reads its
//! payload in place (a view, like a loop's item) instead of moving or copying it.

use std::collections::HashMap;

use kestrel_syntax::ast::{Binding, Block, Expr, ExprKind, TypeExpr};

use super::ok_wrap::article;
use super::{BodyChecker, Expect};
use crate::tables::{LocalId, Res};
use crate::ty::Ty;

/// A `let ... else` view in scope: `view` reads from inside `root` until the end of the block that
/// declares it, which is open while there are at least `depth` scopes.
#[derive(Clone)]
pub(crate) struct LetGuard {
    pub(crate) view: LocalId,
    pub(crate) root: LocalId,
    pub(crate) depth: usize,
}

impl<'a> BodyChecker<'a> {
    /// `let x = opt else { ... }`: `x` is the value inside the optional; the `else` block runs when
    /// it's `none`, and must leave. On a `Result`, `x` is the `Ok` value and the block runs on an
    /// error, which `name` (`else why { ... }`) names in the block.
    pub(crate) fn check_let_else(&mut self, is_var: bool, bind: &Binding, ty: Option<&TypeExpr>, value: &Expr, name: Option<&Binding>, otherwise: &Block) {
        let ann = ty.map(|t| self.resolve(t));
        let want = ann.map(|t| if t == Ty::Error { t } else { Ty::Optional(self.t.intern_elem(t)) });
        let found = self.check_expr(value, self.ok_hint(value, want));
        let result = self.t.result_parts(found);
        let found = self.finish_value(value, found, if result.is_some() { None } else { want });
        let (mut inner, error) = match (result, found) {
            (Some((v, e)), _) => (v, Some(e)),
            (None, Ty::Optional(e)) => (self.t.elem(e), None),
            (None, Ty::Error) => (Ty::Error, None),
            (None, other) => {
                let msg = format!("`let ... else` unwraps an optional (`T?`), but this is {}", self.tn(other));
                self.error(value.span, msg);
                (Ty::Error, None)
            }
        };
        if let (Some(a), Some(_)) = (ann, error) {
            if a != inner && a != Ty::Error && inner != Ty::Error {
                let (held, declared) = (self.tn(inner), self.tn(a));
                let msg = format!(
                    "this `{}` holds {} `{held}`, but `{}` is declared as `{declared}`; change the type or leave it out",
                    self.tn(found),
                    article(&held),
                    bind.name
                );
                self.error(value.span, msg);
                inner = Ty::Error;
            }
        }
        // A `Result` is read in place when it holds memory worth not copying, whatever `T` is.
        let root = if inner == Ty::Error || is_var { None } else { self.view_root(value, if error.is_some() { found } else { inner }) };
        // The `else` block runs when `value` is `none` (or an error): nothing has been unwrapped or
        // moved yet. It must leave, so what it moves doesn't flow past it.
        let before = self.moved.clone();
        self.scopes.push(HashMap::new());
        if let Some(n) = name {
            self.declare_why(n, error, found, root, value);
        }
        let leaves = self.check_block(otherwise, Expect::Discard) == Ty::Never;
        self.scopes.pop();
        if !leaves {
            self.error_help(otherwise.span, "the `else` block has to leave", "end it with `return`, `break` or `continue`");
        }
        self.moved = before;
        if root.is_none() {
            self.consume(value);
        }
        let l = self.declare(bind, inner, is_var, false, false);
        if let Some(root) = root {
            self.t.locals[l.0 as usize].alias = Some(root);
            self.views.insert(l);
            self.note_view(l, value);
            self.t.let_else_views.insert(l);
            self.let_guards.push(LetGuard { view: l, root, depth: self.scopes.len() });
        }
    }

    /// `else why { ... }`: `why` is the error of the `Result` `value` (of type `found`), a view of
    /// it where `x` is one of the `Ok` value (`root` is then the variable both read), or of the
    /// temporary the `Result` went into. An optional has no error to name.
    fn declare_why(&mut self, name: &Binding, error: Option<Ty>, found: Ty, root: Option<LocalId>, value: &Expr) {
        let Some(error) = error else {
            if found != Ty::Error {
                self.error(name.span, "an optional has no error to name; write `else { ... }`");
            }
            self.declare(name, Ty::Error, false, false, false);
            return;
        };
        let why = self.declare(name, error, false, false, false);
        if let Some(root) = root {
            self.t.locals[why.0 as usize].alias = Some(root);
            self.views.insert(why);
            self.note_view(why, value);
        }
    }

    /// The variable `x` reads from when `let x = value else` can be a view: `value` is a place
    /// rooted at a variable that can't change, it isn't a `T` made into a `T?`, and its payload
    /// (`inner`) holds memory worth not copying. A whole owning variable moves instead, as with `let`.
    fn view_root(&self, value: &Expr, inner: Ty) -> Option<LocalId> {
        let worth = self.t.has_heap(inner) || self.t.is_owning(inner);
        if !worth || self.t.coerce.contains_key(&value.id) || !self.is_place(value) {
            return None;
        }
        let root = self.place_root(value)?;
        self.immutable(root)?;
        if let Some(&Res::Local(l)) = self.t.res.get(&value.id) {
            if self.t.is_owning(inner) && !self.t.local(l).is_param && !self.views.contains(&l) {
                return None;
            }
        }
        Some(root)
    }

    /// Whether `e` is a variable, or fields and array or list elements of one: memory a view can read.
    fn is_place(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Ident(_) => matches!(self.t.res.get(&e.id), Some(Res::Local(_))),
            ExprKind::Field { base, .. } => self.is_field_or_lane(e) && self.is_place(base),
            ExprKind::Index { base, .. } => matches!(self.t.ty(base.id), Ty::Array(_) | Ty::List(_)) && self.is_place(base),
            _ => false,
        }
    }
}
