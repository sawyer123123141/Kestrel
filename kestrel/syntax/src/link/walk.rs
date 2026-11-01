//! The walk over one module's items that rewrites its names (see `link`), keeping track of
//! the locals in scope.

use super::chains::dotted;
use super::{Rewriter, Scoped};
use crate::settle::names_generic_item;
use crate::ast::*;

impl Rewriter<'_> {
    pub(super) fn item(&mut self, item: &mut Item) {
        match item {
            Item::Fn(f) => {
                f.name = self.qualified(&f.name).unwrap_or_else(|| f.name.clone());
                self.type_params(&mut f.type_params);
                self.func(f);
            }
            Item::Struct(s) => {
                s.name = self.qualified(&s.name).unwrap_or_else(|| s.name.clone());
                self.type_params(&mut s.type_params);
                for f in &mut s.fields {
                    self.ty(&mut f.ty);
                    if let Some(d) = &mut f.default {
                        self.expr(d);
                    }
                }
                self.iface_names(&mut s.ifaces);
                s.methods.iter_mut().for_each(|m| self.func(m));
            }
            Item::Interface(i) => {
                i.name = self.qualified(&i.name).unwrap_or_else(|| i.name.clone());
                i.methods.iter_mut().for_each(|m| self.func(m));
            }
            Item::Enum(e) => {
                e.name = self.qualified(&e.name).unwrap_or_else(|| e.name.clone());
                self.type_params(&mut e.type_params);
                self.iface_names(&mut e.ifaces);
                for v in &mut e.variants {
                    v.fields.iter_mut().for_each(|f| self.ty(&mut f.ty));
                }
                e.methods.iter_mut().for_each(|m| self.func(m));
            }
            Item::Const(c) => {
                c.bind.name = self.qualified(&c.bind.name).unwrap_or_else(|| c.bind.name.clone());
                if let Some(t) = &mut c.ty {
                    self.ty(t);
                }
                self.expr(&mut c.value);
            }
            Item::Use(_) => {}
        }
        self.placeholders.clear();
    }

    /// The placeholders of an item come into scope for every type in it (its bodies included),
    /// until the item ends. A rule is named like an interface, except the built-in rules.
    fn type_params(&mut self, params: &mut [TypeParam]) {
        self.placeholders = params.iter().map(|p| p.name.clone()).collect();
        for p in params {
            for (rule, span) in &mut p.rules {
                if matches!(rule.as_str(), "equatable" | "ordered" | "key") {
                    continue;
                }
                if rule.contains('.') {
                    if let Some(q) = self.full_type(&rule.clone(), *span) {
                        *rule = q;
                    }
                    continue;
                }
                match self.qualified(rule) {
                    Some(q) => *rule = q,
                    None => self.unseen(&rule.clone(), *span, "type", false),
                }
            }
        }
    }

    /// The interfaces in `struct S: A, B` are named like types.
    fn iface_names(&mut self, ifaces: &mut [(String, crate::span::Span)]) {
        for (name, span) in ifaces {
            if name.contains('.') {
                if let Some(q) = self.full_type(&name.clone(), *span) {
                    *name = q;
                }
                continue;
            }
            match self.qualified(name) {
                Some(q) => *name = q,
                None => self.unseen(&name.clone(), *span, "type", false),
            }
        }
    }

    fn func(&mut self, f: &mut FnDecl) {
        let mark = self.locals.len();
        for p in &mut f.params {
            // A native has no body, so its parameter names are only labels and hide nothing:
            // `draw.text(text: string, ...)` may name its parameter `text`.
            if !p.is_self && !f.is_native {
                self.binding(&p.bind);
            }
            self.ty(&mut p.ty);
        }
        if let Some(r) = &mut f.ret {
            self.ty(r);
        }
        self.block(&mut f.body);
        self.locals.truncate(mark);
    }

    fn ty(&mut self, t: &mut TypeExpr) {
        match &mut t.kind {
            TypeKind::Named { name, args } => {
                // A placeholder is left alone, before anything else: a constant of the module
                // with its name doesn't turn it into `geom.N`, and an entry-file item named
                // like it is no "unknown type".
                if name.contains('.') {
                    // A full name, `net.packet.Packet`: the module, then its item.
                    if let Some(q) = self.full_type(&name.clone(), t.span) {
                        *name = q;
                    }
                } else if !self.placeholders.contains(name) {
                    match self.qualified(name) {
                        Some(q) => *name = q,
                        None => self.unseen(&name.clone(), t.span, "type", false),
                    }
                }
                args.iter_mut().for_each(|a| self.ty(a));
            }
            TypeKind::Array(inner) | TypeKind::Optional(inner) => self.ty(inner),
            TypeKind::Fn { params, ret, .. } => {
                params.iter_mut().for_each(|p| self.ty(&mut p.ty));
                if let Some(r) = ret {
                    self.ty(r);
                }
            }
        }
    }

    /// A block's own locals go out of scope at its end.
    fn block(&mut self, b: &mut Block) {
        let mark = self.locals.len();
        b.stmts.iter_mut().for_each(|s| self.stmt(s));
        self.locals.truncate(mark);
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match s {
            Stmt::Let { bind, ty, value, else_block, .. } => {
                self.expr(value);
                if let Some(t) = ty {
                    self.ty(t);
                }
                if let Some(b) = else_block {
                    self.block(b);
                }
                self.binding(bind);
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
                let mark = self.locals.len();
                self.binding(var);
                self.block(body);
                self.locals.truncate(mark);
            }
            Stmt::ForIn { index, var, iter, body, .. } => {
                self.expr(iter);
                let mark = self.locals.len();
                if let Some(i) = index {
                    self.binding(i);
                }
                self.binding(var);
                self.block(body);
                self.locals.truncate(mark);
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

    fn expr(&mut self, e: &mut Expr) {
        // `net.packet` alone: a module, not a value.
        if matches!(e.kind, ExprKind::Field { .. }) && self.module(e).is_some() {
            let name = dotted(e).unwrap_or_default();
            return self.not_a_value(&name, e.span);
        }
        match &mut e.kind {
            ExprKind::Ident(name) => match self.scope.get(name.as_str()) {
                Some(Scoped::Own(q, _) | Scoped::Item(q, _)) => *name = q.clone(),
                Some(Scoped::Module(_) | Scoped::Ambiguous(..) | Scoped::Prefix(_)) => self.not_a_value(&name.clone(), e.span),
                None => self.unseen(&name.clone(), e.span, "name", true),
            },
            ExprKind::Call { name, name_span, args } => {
                match self.qualified(name) {
                    Some(q) => *name = q,
                    None => self.unseen(&name.clone(), *name_span, "function", false),
                }
                self.args(args);
            }
            ExprKind::MethodCall { receiver, name, name_span, args } => match self.module(receiver) {
                // `m.f(...)` (or `net.m.f(...)`) calls `f` from module `m`.
                Some(m) => {
                    let (name, name_span, mut args) = (name.clone(), *name_span, std::mem::take(args));
                    self.args(&mut args);
                    let name = self.item_of(m, &name, name_span, receiver.span).unwrap_or(name);
                    e.kind = ExprKind::Call { name, name_span, args };
                }
                None => {
                    self.expr(receiver);
                    self.args(args);
                }
            },
            ExprKind::Field { base, name, name_span } => match self.module(base) {
                // `m.LIMIT`, or `m.Shape` in `m.Shape.Circle(...)`.
                Some(m) => {
                    let (name, name_span) = (name.clone(), *name_span);
                    let name = self.item_of(m, &name, name_span, base.span).unwrap_or(name);
                    e.kind = ExprKind::Ident(name);
                }
                None => self.expr(base),
            },
            ExprKind::CallValue { callee, args } => {
                self.expr(callee);
                self.args(args);
            }
            ExprKind::Closure(c) => {
                let mark = self.locals.len();
                for p in &mut c.params {
                    self.binding(&p.bind);
                    if let Some(t) = &mut p.ty {
                        self.ty(t);
                    }
                }
                if let Some(r) = &mut c.ret {
                    self.ty(r);
                }
                self.block(&mut c.body);
                self.locals.truncate(mark);
            }
            ExprKind::Unary { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::If { cond, then_block, else_block } => {
                self.expr(cond);
                self.block(then_block);
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for a in arms {
                    let mark = self.locals.len();
                    if let Pattern::Variant { binding: Some(b), .. } = &a.pattern {
                        self.binding(b);
                    }
                    self.block(&mut a.body);
                    self.locals.truncate(mark);
                }
            }
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
            }
            // The base first (`geom.best` becomes `geom.best` the name); then what it is decides
            // which half is rewritten: the types after a generic, the index after anything else.
            // `settle` (at the end of `link`) makes the same decision.
            ExprKind::Brackets { base, index, types } => {
                self.expr(base);
                let is_local = |n: &str| self.locals.iter().any(|l| l == n);
                if names_generic_item(base, is_local, |n| self.visible.get(n).copied()).is_some() {
                    types.iter_mut().for_each(|t| self.ty(t));
                } else if let Some(i) = index {
                    self.expr(i);
                }
            }
            ExprKind::TypeApply { name, name_span, type_args } => {
                match self.qualified(name) {
                    Some(q) => *name = q,
                    None => self.unseen(&name.clone(), *name_span, "type", false),
                }
                type_args.iter_mut().for_each(|t| self.ty(t));
            }
            ExprKind::ArrayLit(items) => items.iter_mut().for_each(|i| self.expr(i)),
            ExprKind::MapLit(entries) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            ExprKind::TypeCall { type_args, args, .. } => {
                type_args.iter_mut().for_each(|t| self.ty(t));
                self.args(args);
            }
            ExprKind::Interp(parts) => {
                for p in parts {
                    if let InterpPart::Expr(x) = p {
                        self.expr(x);
                    }
                }
            }
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) => {}
        }
    }
}
