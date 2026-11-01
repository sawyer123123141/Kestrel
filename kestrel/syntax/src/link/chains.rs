//! Module names inside code: `packet.make()` and `net.packet.make()` (a module's last name, or the
//! dotted name a `use` wrote, before one of its items), and full names in types, interface lists
//! and rules (`net.packet.Packet`, `geom.Stack[int]`, `struct Game: rollback.Rollback`).

use super::{Rewriter, Scoped};
use crate::ast::{Expr, ExprKind};
use crate::span::Span;

/// What an expression or a name's prefix names as a module.
pub(super) enum ModRef {
    One(usize),
    /// A last name two modules share, as written (see `Scoped::Ambiguous`).
    Two(String, String),
}

/// `a.b.c` for the field chain `a.b.c` over a bare name.
pub(super) fn dotted(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field { base, name, .. } => Some(format!("{}.{name}", dotted(base)?)),
        _ => None,
    }
}

impl Rewriter<'_> {
    /// The module `e` names: a module's last name, or a field chain that is a dotted name the file
    /// imported whole (`net.packet`).
    pub(super) fn module(&self, e: &Expr) -> Option<ModRef> {
        match &e.kind {
            ExprKind::Ident(n) => self.module_named(n),
            ExprKind::Field { .. } if !self.chains.is_empty() => self.chains.get(&dotted(e)?).map(|&m| ModRef::One(m)),
            _ => None,
        }
    }

    /// The module `name` (a last name, or a dotted name as written) names in this file.
    fn module_named(&self, name: &str) -> Option<ModRef> {
        if name.contains('.') {
            return self.chains.get(name).map(|&m| ModRef::One(m));
        }
        match self.scope.get(name) {
            Some(Scoped::Module(m)) => Some(ModRef::One(*m)),
            Some(Scoped::Ambiguous(a, b)) => Some(ModRef::Two(a.clone(), b.clone())),
            _ => None,
        }
    }

    /// The qualified name of `pub` item `item` (at `span`) of the module `r` names, written at
    /// `at`: an error when there's no such item, or when `r` could be two modules.
    pub(super) fn item_of(&mut self, r: ModRef, item: &str, span: Span, at: Span) -> Option<String> {
        match r {
            ModRef::One(m) => self.exported(m, item, span),
            ModRef::Two(a, b) => {
                self.ambiguous(&a, &b, item, at);
                None
            }
        }
    }

    /// The error for a last name two modules share, used before `item`.
    pub(super) fn ambiguous(&mut self, a: &str, b: &str, item: &str, span: Span) {
        let short = a.rsplit('.').next().unwrap_or(a);
        self.error(span, format!("`{short}` could be `{a}` or `{b}`; write the full name, like `{a}.{item}`"));
    }

    /// A type, interface or rule written with its module (`net.packet.Packet`): its qualified name,
    /// or `None` after saying what is wrong. Besides the names a `use` wrote, a type may name a
    /// module this file uses by its full name (`net.packet.Packet` inside `net/` after `use packet`):
    /// no local can hide a type.
    pub(super) fn full_type(&mut self, name: &str, span: Span) -> Option<String> {
        let (prefix, item) = name.rsplit_once('.')?;
        let full = || self.used.iter().find(|&&m| self.names[m] == prefix).map(|&m| ModRef::One(m));
        match self.module_named(prefix).or_else(full) {
            Some(r) => self.item_of(r, item, span, span),
            None => {
                let msg = format!("`{prefix}` isn't a module this file uses; add `use {prefix}`, or import the type: `use {{ {item} from {prefix} }}`");
                self.error(span, msg);
                None
            }
        }
    }

    /// The error for a module's name used as a value: `geom` alone, `net.packet` alone, or the
    /// first name `net` of `use net.packet`.
    pub(super) fn not_a_value(&mut self, name: &str, span: Span) {
        let msg = match self.scope.get(name) {
            Some(Scoped::Ambiguous(a, b)) => {
                let (a, b) = (a.clone(), b.clone());
                return self.ambiguous(&a, &b, "something", span);
            }
            Some(Scoped::Prefix(chain)) => {
                format!("`{name}` starts the module name `{chain}`; use one of that module's items, like `{chain}.something`")
            }
            _ => format!("`{name}` is a module; use one of its items, like `{name}.something`"),
        };
        self.error(span, msg);
    }
}
