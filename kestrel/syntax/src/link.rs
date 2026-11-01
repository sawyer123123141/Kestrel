//! Links several parsed files (modules) into one program. Items of every
//! module except the entry are renamed `module.name` (with the module's full name, like
//! `net.packet.Packet`), and each module's references are rewritten to what they mean there: its
//! own items, names it imported with `use { a from m }`, and `m.a` for a module it imported
//! whole with `use m` (by its last name `m`, or by the whole dotted name it was written with).
//!
//! Each module sees only those names, so this is also where a local is checked against them:
//! a local can't take the name of one of its module's items or imports. (The checker sees the
//! linked program, where another module's items, the entry file's included, are all global.)

mod chains;
mod imports;
mod walk;

use std::collections::HashMap;

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::settle::{settle, ItemKind};
use crate::span::Span;

pub struct Module {
    /// The module's full name: its file's path from the entry file's folder, `.` for `/` and
    /// without `.kes` (`net/packet.kes` is `net.packet`, and `net/net.kes` is `net`). The entry
    /// file's is its file name without `.kes`; its items keep their own names.
    pub name: String,
    /// Its file's path from the entry file's folder, with `/` (`net/packet.kes`), for messages.
    pub file: String,
    pub program: Program,
    /// The module each `use` clause of `program` names, by its index in the modules, in the order
    /// the clauses are written: `None` for one that wasn't found (the loader said why).
    pub resolved: Vec<Option<usize>>,
}

struct Export {
    qualified: String,
    is_pub: bool,
    kind: ItemKind,
}

/// What a name means inside one module.
enum Scoped {
    /// One of the module's own items, by its qualified name and kind.
    Own(String, ItemKind),
    /// An item imported with `use { a from m }`, by its qualified name and kind.
    Item(String, ItemKind),
    /// A module imported whole with `use m` or `use a.m`, by its last name.
    Module(usize),
    /// The last name of two modules imported whole (`use net.packet`, `use ui.packet`), as they
    /// were written: neither has the short name, and each is reached by its whole name.
    Ambiguous(String, String),
    /// The first name of a dotted `use` (`net` of `use net.packet`, as written), which only
    /// starts a module's name.
    Prefix(String),
}

/// Links `modules`, the entry file first. Their ids must already be
/// distinct (see `parse_at`). `reserved` says which names no item may take (the checker
/// refuses such an item, so `link` says nothing more about it).
pub fn link(modules: Vec<Module>, reserved: fn(&str) -> bool) -> (Program, Vec<Diagnostic>) {
    let exports: Vec<HashMap<String, Export>> = modules
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let qualify = |name: &str| if i == 0 { name.to_string() } else { format!("{}.{name}", m.name) };
            m.program
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Fn(f) => Some((f.name.clone(), f.is_pub, ItemKind::Fn)),
                    Item::Struct(s) => Some((s.name.clone(), s.is_pub, ItemKind::Struct)),
                    Item::Enum(e) => Some((e.name.clone(), e.is_pub, ItemKind::Enum)),
                    Item::Interface(i) => Some((i.name.clone(), i.is_pub, ItemKind::Interface)),
                    Item::Const(c) => Some((c.bind.name.clone(), c.is_pub, ItemKind::Const)),
                    Item::Use(_) => None,
                })
                .map(|(name, is_pub, kind)| (name.clone(), Export { qualified: qualify(&name), is_pub, kind }))
                .collect()
        })
        .collect();
    let names: Vec<String> = modules.iter().map(|m| m.name.clone()).collect();
    let files: Vec<String> = modules.iter().map(|m| m.file.clone()).collect();
    let (mut expr_count, mut bind_count) = (0, 0);
    let mut items = Vec::new();
    let mut proves = Vec::new();
    let mut hex_literals = Vec::new();
    let mut diags = Vec::new();
    for (i, m) in modules.into_iter().enumerate() {
        expr_count = expr_count.max(m.program.expr_count);
        bind_count = bind_count.max(m.program.bind_count);
        proves.extend(m.program.proves);
        hex_literals.extend(m.program.hex_literals);
        let mut rw = Rewriter {
            scope: HashMap::new(),
            chains: HashMap::new(),
            shorts: HashMap::new(),
            used: Vec::new(),
            exports: &exports,
            names: &names,
            files: &files,
            module: i,
            locals: Vec::new(),
            placeholders: Vec::new(),
            visible: HashMap::new(),
            reserved,
            diags: Vec::new(),
        };
        for (name, e) in &exports[i] {
            rw.scope.insert(name.clone(), Scoped::Own(e.qualified.clone(), e.kind));
        }
        let mut resolved = m.resolved.iter().copied();
        for item in &m.program.items {
            if let Item::Use(u) = item {
                for c in &u.clauses {
                    rw.import(c, resolved.next().flatten());
                }
            }
        }
        rw.visible = rw.visible_kinds();
        for mut item in m.program.items {
            if !matches!(item, Item::Use(_)) {
                rw.item(&mut item);
                items.push(item);
            }
        }
        diags.append(&mut rw.diags);
    }
    let mut program = Program { items, expr_count, bind_count, builtin_ranges: Vec::new(), proves, hex_literals };
    diags.extend(settle(&mut program));
    (program, diags)
}

struct Rewriter<'a> {
    scope: HashMap<String, Scoped>,
    /// The modules imported whole by a dotted name, by that name as written (`net.packet`).
    chains: HashMap<String, usize>,
    /// Whether the module holding each short name (`Scoped::Module`) was written as a single name.
    shorts: HashMap<String, bool>,
    /// Every module imported whole, however it is reached.
    used: Vec<usize>,
    exports: &'a [HashMap<String, Export>],
    /// Each module's full name, by index; 0 is the entry file.
    names: &'a [String],
    /// Each module's file from the entry file's folder, by index.
    files: &'a [String],
    /// The module being rewritten, an index into `names`; 0 is the entry file.
    module: usize,
    /// The locals and parameters in scope at this point of the module's code.
    locals: Vec<String>,
    /// The placeholders of the item being rewritten: a name among them in a type is the
    /// placeholder, whatever else the module has by that name.
    placeholders: Vec<String>,
    /// The items this module can name once rewritten (its own, its imports, and the `pub` items
    /// of the modules it imported whole), by qualified name: what the brackets decision asks.
    visible: HashMap<String, ItemKind>,
    /// Names no item may take (see `link`).
    reserved: fn(&str) -> bool,
    diags: Vec<Diagnostic>,
}

impl Rewriter<'_> {
    fn error(&mut self, span: Span, msg: String) {
        self.diags.push(Diagnostic::error(span, msg));
    }

    /// The kind of every item the module's scope reaches, by qualified name.
    fn visible_kinds(&self) -> HashMap<String, ItemKind> {
        let mut kinds = HashMap::new();
        for s in self.scope.values() {
            if let Scoped::Own(q, k) | Scoped::Item(q, k) = s {
                kinds.insert(q.clone(), *k);
            }
        }
        for &m in &self.used {
            for e in self.exports[m].values().filter(|e| e.is_pub) {
                kinds.insert(e.qualified.clone(), e.kind);
            }
        }
        kinds
    }

    /// The qualified name of `pub` item `name` of module `m`.
    fn exported(&mut self, m: usize, name: &str, span: Span) -> Option<String> {
        let module = &self.names[m];
        match self.exports[m].get(name) {
            Some(e) if e.is_pub => Some(e.qualified.clone()),
            Some(_) => {
                let msg = format!("`{name}` isn't `pub` in `{module}`");
                self.error(span, msg);
                None
            }
            None => {
                let msg = format!("`{module}` has no `{name}`");
                self.error(span, msg);
                None
            }
        }
    }

    fn qualified(&self, name: &str) -> Option<String> {
        match self.scope.get(name) {
            Some(Scoped::Own(q, _) | Scoped::Item(q, _)) => Some(q.clone()),
            _ => None,
        }
    }

    /// A local or parameter comes into scope. It can't take the name of one of the module's
    /// items or imports (the checker only sees the qualified names, so it couldn't tell).
    fn binding(&mut self, b: &Binding) {
        match self.scope.get(&b.name) {
            Some(Scoped::Module(_) | Scoped::Item(..) | Scoped::Ambiguous(..) | Scoped::Prefix(_)) => {
                self.error(b.span, format!("`{}` is already defined: it's imported; pick another name", b.name));
            }
            Some(Scoped::Own(..)) => {
                let msg = format!("`{}` is already defined in this file; Kestrel doesn't allow shadowing", b.name);
                self.diags.push(Diagnostic::error(b.span, msg).with_help("pick a different name"));
            }
            None => {}
        }
        self.locals.push(b.name.clone());
    }

    /// A bare name that means nothing in this module. The entry file's items keep their own
    /// names when linked, so without this another module could reach one it never imported.
    /// `local` is whether a local of that name would be what it means (not for a call or a type).
    fn unseen(&mut self, name: &str, span: Span, what: &str, local: bool) {
        let hidden = self.module != 0 && self.exports[0].contains_key(name) && !(self.reserved)(name);
        if hidden && !(local && self.locals.iter().any(|l| l == name)) {
            let (entry, here) = (&self.names[0], &self.names[self.module]);
            let msg = format!("unknown {what} `{name}`: it's in {entry}.kes, which `{here}` can't see");
            let help = "a module sees only its own items and the ones it imports with `use`";
            self.diags.push(Diagnostic::error(span, msg).with_help(help));
        }
    }
}
