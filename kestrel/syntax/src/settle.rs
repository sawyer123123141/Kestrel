//! Settling: what `base[...]` means once the names are known. The parser keeps both readings of
//! brackets after a name (`ExprKind::Brackets`: the types, and the index when it parses as one);
//! this pass, run at the end of `parse` and of `link`, picks one by what the name is:
//!
//! - the name of a generic struct, enum, interface or function (and not hidden by a local):
//!   types. `best[Enemy](foes)` is a `TypeCall`, `Maybe[int]` a `TypeApply`;
//! - anything else: an index (`xs[i]`), or, when only types parse, an error.
//!
//! `link` makes the same decision (`names_generic_item`) while it rewrites names, so it knows
//! which half of the brackets to rewrite.

use std::collections::HashMap;

use crate::ast::*;
use crate::diag::Diagnostic;

/// What kind of item a name is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemKind {
    Fn,
    Struct,
    Enum,
    Interface,
    Const,
}

/// The name `base` writes when it names a struct, enum, interface or function item that is not
/// hidden by a local: the brackets after it hold types. (A constant, a local or a field is
/// indexed.) `is_local` says whether a local of that name is in scope, `item_kind` what item
/// the name is.
pub(crate) fn names_generic_item(
    base: &Expr,
    is_local: impl Fn(&str) -> bool,
    item_kind: impl Fn(&str) -> Option<ItemKind>,
) -> Option<String> {
    let ExprKind::Ident(name) = &base.kind else { return None };
    if is_local(name) {
        return None;
    }
    match item_kind(name) {
        Some(ItemKind::Fn | ItemKind::Struct | ItemKind::Enum | ItemKind::Interface) => Some(name.clone()),
        Some(ItemKind::Const) | None => None,
    }
}

/// Settles every `Brackets` in `program` (see the module docs). Returns the errors: types in
/// brackets after something that isn't a generic.
pub(crate) fn settle(program: &mut Program) -> Vec<Diagnostic> {
    let mut kinds = HashMap::new();
    for item in &program.items {
        let (name, kind) = match item {
            Item::Fn(f) => (&f.name, ItemKind::Fn),
            Item::Struct(s) => (&s.name, ItemKind::Struct),
            Item::Enum(e) => (&e.name, ItemKind::Enum),
            Item::Interface(i) => (&i.name, ItemKind::Interface),
            Item::Const(c) => (&c.bind.name, ItemKind::Const),
            Item::Use(_) => continue,
        };
        kinds.insert(name.clone(), kind);
    }
    let mut s = Settler { kinds, locals: Vec::new(), diags: Vec::new() };
    for item in &mut program.items {
        s.item(item);
    }
    s.diags
}

struct Settler {
    kinds: HashMap<String, ItemKind>,
    /// The locals and parameters in scope, as `link` tracks them.
    locals: Vec<String>,
    diags: Vec<Diagnostic>,
}

impl Settler {
    fn item(&mut self, item: &mut Item) {
        match item {
            Item::Fn(f) => self.func(f),
            Item::Struct(s) => {
                s.fields.iter_mut().filter_map(|f| f.default.as_mut()).for_each(|d| self.expr(d));
                s.methods.iter_mut().for_each(|m| self.func(m));
            }
            Item::Enum(e) => e.methods.iter_mut().for_each(|m| self.func(m)),
            Item::Interface(i) => i.methods.iter_mut().for_each(|m| self.func(m)),
            Item::Const(c) => self.expr(&mut c.value),
            Item::Use(_) => {}
        }
    }

    fn func(&mut self, f: &mut FnDecl) {
        let mark = self.locals.len();
        self.locals.extend(f.params.iter().map(|p| p.bind.name.clone()));
        self.block(&mut f.body);
        self.locals.truncate(mark);
    }

    fn block(&mut self, b: &mut Block) {
        let mark = self.locals.len();
        b.stmts.iter_mut().for_each(|s| self.stmt(s));
        self.locals.truncate(mark);
    }

    fn scoped(&mut self, names: &[&Binding], body: &mut Block) {
        let mark = self.locals.len();
        self.locals.extend(names.iter().map(|b| b.name.clone()));
        self.block(body);
        self.locals.truncate(mark);
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match s {
            Stmt::Let { bind, value, else_name, else_block, .. } => {
                self.expr(value);
                if let Some(b) = else_block {
                    let mark = self.locals.len();
                    self.locals.extend(else_name.as_ref().map(|n| n.name.clone()));
                    self.block(b);
                    self.locals.truncate(mark);
                }
                self.locals.push(bind.name.clone());
            }
            Stmt::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::ForRange { var, start, end, body, .. } => {
                self.expr(start);
                self.expr(end);
                self.scoped(&[var], body);
            }
            Stmt::ForIn { index, var, iter, body, .. } => {
                self.expr(iter);
                let names: Vec<&Binding> = index.iter().chain(std::iter::once(&*var)).collect();
                self.scoped(&names, body);
            }
            Stmt::Arena { body, .. } => self.block(body),
            Stmt::In { arena, body, .. } => {
                self.expr(arena);
                self.block(body);
            }
            Stmt::Return { value: Some(v), .. } | Stmt::Expr(v) => self.expr(v),
            Stmt::Assume { cond, .. } => self.expr(cond),
            Stmt::Return { value: None, .. } | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Remove { .. } => {}
        }
    }

    fn args(&mut self, args: &mut [Arg]) {
        args.iter_mut().for_each(|a| self.expr(&mut a.value));
    }

    /// Whether the brackets after `base` hold types: its name (see `names_generic_item`).
    fn generic(&self, base: &Expr) -> Option<String> {
        names_generic_item(base, |n| self.locals.iter().any(|l| l == n), |n| self.kinds.get(n).copied())
    }

    /// Bottom-up: the parts first, then the node itself.
    fn expr(&mut self, e: &mut Expr) {
        if matches!(e.kind, ExprKind::Brackets { .. }) {
            return self.brackets(e);
        }
        self.parts(e);
        // `best[Enemy](foes)`: a call of a generic with its types written.
        if matches!(&e.kind, ExprKind::CallValue { callee, .. } if matches!(callee.kind, ExprKind::TypeApply { .. })) {
            let ExprKind::CallValue { callee, args } = std::mem::replace(&mut e.kind, ExprKind::None) else { return };
            if let ExprKind::TypeApply { name, name_span, type_args } = callee.kind {
                e.kind = ExprKind::TypeCall { name, name_span, type_args, args };
            }
        }
    }

    /// Settles what is inside `e`.
    fn parts(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) | ExprKind::Ident(_) => {}
            ExprKind::TypeApply { .. } => {}
            ExprKind::Interp(parts) => {
                for p in parts {
                    if let InterpPart::Expr(x) = p {
                        self.expr(x);
                    }
                }
            }
            ExprKind::Unary { expr, .. } | ExprKind::Try(expr) => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Call { args, .. } | ExprKind::TypeCall { args, .. } => self.args(args),
            ExprKind::If { cond, then_block, else_block } => {
                self.expr(cond);
                self.block(then_block);
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expr(receiver);
                self.args(args);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for a in arms {
                    let bound = match &a.pattern {
                        Pattern::Variant { binding: Some(b), .. } => Some(b.name.clone()),
                        _ => None,
                    };
                    let mark = self.locals.len();
                    self.locals.extend(bound);
                    self.block(&mut a.body);
                    self.locals.truncate(mark);
                }
            }
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
            }
            ExprKind::Brackets { .. } => unreachable!("settled by `expr`"),
            ExprKind::ArrayLit(items) => items.iter_mut().for_each(|i| self.expr(i)),
            ExprKind::MapLit(entries) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            ExprKind::CallValue { callee, args } => {
                self.expr(callee);
                self.args(args);
            }
            ExprKind::Closure(c) => {
                let mark = self.locals.len();
                self.locals.extend(c.params.iter().map(|p| p.bind.name.clone()));
                self.block(&mut c.body);
                self.locals.truncate(mark);
            }
        }
    }

    /// A `Brackets` node becomes a `TypeApply` (a generic's name), an `Index`, or an error.
    fn brackets(&mut self, e: &mut Expr) {
        let ExprKind::Brackets { mut base, index, types } = std::mem::replace(&mut e.kind, ExprKind::None) else { return };
        self.expr(&mut base);
        if let Some(name) = self.generic(&base) {
            e.kind = ExprKind::TypeApply { name, name_span: base.span, type_args: types };
            return;
        }
        match index {
            Some(mut index) => {
                self.expr(&mut index);
                e.kind = ExprKind::Index { base, index };
            }
            None => {
                let tys: Vec<String> = types.iter().map(TypeExpr::to_sexpr).collect();
                let after = match written(&base) {
                    Some(path) => format!(" after `{path}`"),
                    // `mk().items[...]`: the field alone, as the base isn't a path.
                    None => match &base.kind {
                        ExprKind::Field { name, .. } => format!(" after `{name}`"),
                        _ => String::new(),
                    },
                };
                let msg = format!("`[...]`{after} indexes it, but `{}` isn't a value; types go after a generic type or function", tys.join(", "));
                // The node stays empty: the error stops the program before anything reads it.
                self.diags.push(Diagnostic::error(e.span, msg));
            }
        }
    }
}

/// The base as the user wrote it, for a message, when it is a path (`xs`, `game.items`).
fn written(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field { base, name, .. } => Some(format!("{}.{name}", written(base)?)),
        _ => None,
    }
}
