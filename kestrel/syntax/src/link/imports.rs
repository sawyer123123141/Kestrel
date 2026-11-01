//! A module's `use` clauses: the names each one brings into the module's scope (see `link`).
//!
//! `use { a from m }` brings item `a`. `use m` and `use net.packet` bring the module whole: by its
//! last name (`packet`), unless two such modules share it, and by the dotted name as written,
//! whose first name (`net`) is then taken in the file. When one of two modules sharing a last name
//! was written as a single name, it keeps the short name.

use super::{Rewriter, Scoped};
use crate::ast::UseClause;
use crate::span::Span;

impl Rewriter<'_> {
    /// Brings the names of clause `c` into scope; `resolved` is the module the loader found for it
    /// (`None` when it found none and said so).
    pub(super) fn import(&mut self, c: &UseClause, resolved: Option<usize>) {
        let Some(m) = resolved else { return };
        if c.names.is_empty() {
            self.import_module(&c.module, c.module_span, m);
            return;
        }
        for (name, span) in &c.names {
            let Some(q) = self.exported(m, name, *span) else { continue };
            if self.scope.contains_key(name) {
                self.error(*span, format!("`{name}` is already defined here"));
            } else {
                let kind = self.exports[m][name.as_str()].kind;
                self.scope.insert(name.clone(), Scoped::Item(q, kind));
            }
        }
    }

    /// `use m` or `use a.b.m` (as `written`): module `m` imported whole.
    fn import_module(&mut self, written: &str, span: Span, m: usize) {
        if let Some(msg) = self.module_and_item(written, m) {
            self.error(span, msg);
            return;
        }
        let single = !written.contains('.');
        if !single {
            let first = written.split('.').next().unwrap_or(written);
            match self.scope.get(first) {
                Some(Scoped::Own(..) | Scoped::Item(..)) => {
                    self.error(span, format!("`{first}` is already defined here"));
                    return;
                }
                None => {
                    self.scope.insert(first.to_string(), Scoped::Prefix(written.to_string()));
                }
                Some(_) => {}
            }
            self.chains.insert(written.to_string(), m);
        }
        self.used.push(m);
        let short = written.rsplit('.').next().unwrap_or(written);
        let held = match self.scope.get(short) {
            None | Some(Scoped::Prefix(_)) => None,
            Some(Scoped::Module(old)) => Some((*old, self.shorts.get(short).copied().unwrap_or(true))),
            Some(Scoped::Ambiguous(..)) => {
                // Two dotted modules share it already; a single name takes it.
                if single {
                    self.hold(short, m, single);
                }
                return;
            }
            Some(Scoped::Own(..) | Scoped::Item(..)) => {
                self.error(span, format!("`{short}` is already defined here"));
                return;
            }
        };
        match held {
            None => self.hold(short, m, single),
            // The same name twice (`use geom` twice), as today.
            Some((_, true)) if single => self.error(span, format!("`{short}` is already defined here")),
            Some((old, _)) if old == m => {}
            // A single name keeps the short name; the dotted one is reached by its whole name.
            Some((_, true)) => {}
            Some(_) if single => self.hold(short, m, single),
            Some((old, false)) => {
                let tail = format!(".{short}");
                let first = self.chains.iter().find(|(c, &i)| i == old && c.ends_with(&tail)).map_or_else(|| self.names[old].clone(), |(c, _)| c.clone());
                self.scope.insert(short.to_string(), Scoped::Ambiguous(first, written.to_string()));
            }
        }
    }

    /// `short` names module `m` in this file.
    fn hold(&mut self, short: &str, m: usize, single: bool) {
        self.scope.insert(short.to_string(), Scoped::Module(m));
        self.shorts.insert(short.to_string(), single);
    }

    /// The module this file imported whole as `written` (a name or a dotted name), if any.
    pub(super) fn module_written(&self, written: &str) -> Option<usize> {
        if written.contains('.') {
            return self.chains.get(written).copied();
        }
        match self.scope.get(written) {
            Some(Scoped::Module(m)) => Some(*m),
            _ => None,
        }
    }

    /// The error for importing module `m` as `written` when one full name would mean both a module
    /// and an item: `use net` whose `net.kes` has a `pub` item `packet`, and `use net.packet`, in
    /// either order (the error is at the second).
    fn module_and_item(&self, written: &str, m: usize) -> Option<String> {
        let clash = |dotted: &str, module: usize, item: &str, holder: usize| {
            self.exports[holder].get(item).filter(|e| e.is_pub).map(|_| {
                format!("`{dotted}` is both the module {} and `{item}` in {}; rename one of them", self.files[module], self.files[holder])
            })
        };
        if let Some((prefix, item)) = written.rsplit_once('.') {
            if let Some(msg) = self.module_written(prefix).and_then(|holder| clash(written, m, item, holder)) {
                return Some(msg);
            }
        }
        let below = format!("{written}.");
        let mut chains: Vec<(&String, &usize)> = self.chains.iter().collect();
        chains.sort();
        chains.into_iter().find_map(|(chain, &module)| {
            let item = chain.strip_prefix(&below).filter(|rest| !rest.contains('.'))?;
            clash(chain, module, item, m)
        })
    }
}
