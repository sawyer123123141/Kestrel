//! What is at an offset of the linked program: the smallest node whose span holds it. Spans are
//! in the program's shared span space, so `offset` is the entry file's offset plus its base.

use kestrel_syntax::ast::*;
use kestrel_syntax::Span;

/// A node under the cursor.
#[derive(Clone, Copy, Debug)]
pub enum Hit<'p> {
    Expr(&'p Expr),
    /// The name in a call, method call, field access or type call (`f` in `f(x)`, `y` in `p.y`).
    Name(&'p Expr),
    /// A local, parameter, loop variable or constant being introduced.
    Binding(&'p Binding),
    /// A named type: `Point` in `p: Point`.
    Type(&'p TypeExpr),
    /// The name of a declaration (a function, struct, enum, interface, field or variant).
    Decl(Span),
    /// An interface named in `struct S: A` or a rule, with any types (`Handler[Click]`).
    Iface(&'p IfaceRef),
    /// The variant name in a `match` arm: arm `arm` of the `match` expression `matched`.
    Pattern { matched: &'p Expr, arm: usize, span: Span },
    /// The name in `remove b`, keyed by the statement's span.
    Remove { stmt: Span, name: Span },
}

/// The smallest node of `p` whose span holds `offset` (an offset at a span's end counts, so a
/// cursor just after a name finds it). Of nodes with the same span, the innermost wins.
pub fn find(p: &Program, offset: u32) -> Option<Hit<'_>> {
    let mut f = Finder { offset, best: None };
    for item in &p.items {
        f.item(item);
    }
    f.best.map(|(_, h)| h)
}

struct Finder<'p> {
    offset: u32,
    best: Option<(u32, Hit<'p>)>,
}

impl<'p> Finder<'p> {
    fn hit(&mut self, span: Span, hit: Hit<'p>) {
        // An empty default span is a node the parser made up, written nowhere.
        if span != Span::default() && span.start <= self.offset && self.offset <= span.end {
            let len = span.end - span.start;
            if self.best.as_ref().is_none_or(|(b, _)| len <= *b) {
                self.best = Some((len, hit));
            }
        }
    }

    fn item(&mut self, item: &'p Item) {
        match item {
            Item::Fn(f) => self.func(f),
            Item::Const(c) => {
                self.hit(c.bind.span, Hit::Binding(&c.bind));
                if let Some(t) = &c.ty {
                    self.ty(t);
                }
                self.expr(&c.value);
            }
            Item::Struct(s) => {
                self.hit(s.name_span, Hit::Decl(s.name_span));
                self.type_params(&s.type_params);
                self.ifaces(&s.ifaces);
                self.fields(&s.fields);
                s.methods.iter().for_each(|m| self.func(m));
            }
            Item::Enum(e) => {
                self.hit(e.name_span, Hit::Decl(e.name_span));
                self.type_params(&e.type_params);
                self.ifaces(&e.ifaces);
                for v in &e.variants {
                    self.hit(v.name_span, Hit::Decl(v.name_span));
                    self.fields(&v.fields);
                }
                e.methods.iter().for_each(|m| self.func(m));
            }
            Item::Interface(i) => {
                self.hit(i.name_span, Hit::Decl(i.name_span));
                self.type_params(&i.type_params);
                i.methods.iter().for_each(|m| self.func(m));
            }
            Item::Use(_) => {}
        }
    }

    /// Each placeholder is a declaration, and each rule it lists names an interface (or a built-in rule).
    fn type_params(&mut self, params: &'p [TypeParam]) {
        for p in params {
            self.hit(p.span, Hit::Decl(p.span));
            self.ifaces(&p.rules);
        }
    }

    fn ifaces(&mut self, ifaces: &'p [IfaceRef]) {
        for r in ifaces {
            self.hit(r.span, Hit::Iface(r));
            r.args.iter().for_each(|a| self.ty(a));
        }
    }

    fn fields(&mut self, fields: &'p [FieldDecl]) {
        for f in fields {
            self.hit(f.name_span, Hit::Decl(f.name_span));
            self.ty(&f.ty);
            if let Some(d) = &f.default {
                self.expr(d);
            }
        }
    }

    fn func(&mut self, f: &'p FnDecl) {
        self.hit(f.name_span, Hit::Decl(f.name_span));
        self.type_params(&f.type_params);
        for p in &f.params {
            self.hit(p.bind.span, Hit::Binding(&p.bind));
            // A `self` parameter's type is filled in by the parser, at the `self`.
            if !p.is_self {
                self.ty(&p.ty);
            }
        }
        if let Some(r) = &f.ret {
            self.ty(r);
        }
        self.block(&f.body);
    }

    fn ty(&mut self, t: &'p TypeExpr) {
        match &t.kind {
            TypeKind::Named { args, .. } => {
                self.hit(t.span, Hit::Type(t));
                args.iter().for_each(|a| self.ty(a));
            }
            TypeKind::Array(inner) | TypeKind::Optional(inner) => self.ty(inner),
            TypeKind::Fn { params, ret, .. } => {
                params.iter().for_each(|p| self.ty(&p.ty));
                if let Some(r) = ret {
                    self.ty(r);
                }
            }
        }
    }

    fn block(&mut self, b: &'p Block) {
        b.stmts.iter().for_each(|s| self.stmt(s));
    }

    fn stmt(&mut self, s: &'p Stmt) {
        match s {
            Stmt::Let { bind, ty, value, else_name, else_block, .. } => {
                self.hit(bind.span, Hit::Binding(bind));
                if let Some(t) = ty {
                    self.ty(t);
                }
                self.expr(value);
                if let Some(n) = else_name {
                    self.hit(n.span, Hit::Binding(n));
                }
                if let Some(b) = else_block {
                    self.block(b);
                }
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
                self.hit(var.span, Hit::Binding(var));
                self.expr(start);
                self.expr(end);
                self.block(body);
            }
            Stmt::ForIn { index, var, iter, body, .. } => {
                if let Some(i) = index {
                    self.hit(i.span, Hit::Binding(i));
                }
                self.hit(var.span, Hit::Binding(var));
                self.expr(iter);
                self.block(body);
            }
            Stmt::Arena { body, .. } => self.block(body),
            Stmt::In { arena, body, .. } => {
                self.expr(arena);
                self.block(body);
            }
            Stmt::Return { value: Some(v), .. } | Stmt::Expr(v) => self.expr(v),
            Stmt::Assume { cond, .. } => self.expr(cond),
            Stmt::Remove { name_span, span, .. } => self.hit(*name_span, Hit::Remove { stmt: *span, name: *name_span }),
            Stmt::Return { value: None, .. } | Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    fn args(&mut self, args: &'p [Arg]) {
        args.iter().for_each(|a| self.expr(&a.value));
    }

    fn expr(&mut self, e: &'p Expr) {
        self.hit(e.span, Hit::Expr(e));
        match &e.kind {
            ExprKind::Call { name, name_span, args } => {
                // `m.f(x)` is linked to a call of `m.f` named at `f`; `m.` names it too.
                let name_span = if name.contains('.') { Span { start: e.span.start, end: name_span.end } } else { *name_span };
                self.hit(name_span, Hit::Name(e));
                self.args(args);
            }
            ExprKind::MethodCall { receiver, name_span, args, .. } => {
                self.expr(receiver);
                self.hit(*name_span, Hit::Name(e));
                self.args(args);
            }
            ExprKind::Field { base, name_span, .. } => {
                self.expr(base);
                self.hit(*name_span, Hit::Name(e));
            }
            ExprKind::TypeCall { name_span, type_args, args, .. } => {
                self.hit(*name_span, Hit::Name(e));
                type_args.iter().for_each(|t| self.ty(t));
                self.args(args);
            }
            ExprKind::TypeApply { name_span, type_args, .. } => {
                self.hit(*name_span, Hit::Name(e));
                type_args.iter().for_each(|t| self.ty(t));
            }
            ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
            ExprKind::CallValue { callee, args } => {
                self.expr(callee);
                self.args(args);
            }
            ExprKind::Closure(c) => {
                for p in &c.params {
                    self.hit(p.bind.span, Hit::Binding(&p.bind));
                    if let Some(t) = &p.ty {
                        self.ty(t);
                    }
                }
                if let Some(r) = &c.ret {
                    self.ty(r);
                }
                self.block(&c.body);
            }
            ExprKind::Unary { expr, .. } | ExprKind::Try(expr) => self.expr(expr),
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
                for (i, a) in arms.iter().enumerate() {
                    if let Pattern::Variant { name_span, binding, .. } = &a.pattern {
                        self.hit(*name_span, Hit::Pattern { matched: e, arm: i, span: *name_span });
                        if let Some(b) = binding {
                            self.hit(b.span, Hit::Binding(b));
                        }
                    }
                    self.block(&a.body);
                }
            }
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
            }
            ExprKind::ArrayLit(items) => items.iter().for_each(|i| self.expr(i)),
            ExprKind::MapLit(entries) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            ExprKind::Interp(parts) => {
                for p in parts {
                    if let InterpPart::Expr(x) = p {
                        self.expr(x);
                    }
                }
            }
            ExprKind::Ident(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) => {}
        }
    }
}
