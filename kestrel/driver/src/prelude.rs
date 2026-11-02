//! How files join a module: a later part of a built-in module of several parts joins the module of
//! its first part (see `load`), and the prelude joins the entry file's.
//!
//! The prelude (`std/prelude.kes`, shown as `<builtin>/prelude.kes`) is what every file sees
//! without a `use`: the built-in `Result[T, E]`. It is not a module, so no `use` names it. Its items
//! join the entry file's module, so they keep their own names when linked (`Result`, never
//! `prelude.Result`) and are one definition for the whole program; every other module gets a
//! `use` of them it never wrote, so they are in its scope as an import is (its own types inside
//! `Result[...]` are its own). `syntax::link` itself knows nothing of it.

use kestrel_syntax::ast::{Item, Program, UseClause, UseDecl};
use kestrel_syntax::link::Module;
use kestrel_syntax::{parse_at, Diagnostic};

use crate::{builtin, push_file, Source};

/// The prelude's source.
const PRELUDE: &str = include_str!("../std/prelude.kes");

/// Adds the prelude to what `load` read: its file after the others, parsed with ids after
/// `next_expr` and `next_bind`; its items joined to the entry file's module, `modules[0]`; and in
/// every other module a `use` of each of its `pub` items, unless the module has an item of that
/// name (the checker refuses that item: the name is reserved).
pub(crate) fn add(files: &mut Vec<Source>, modules: &mut [Module], next_expr: u32, next_bind: u32, diags: &mut Vec<Diagnostic>) {
    let entry = modules[0].name.clone();
    push_file(files, builtin::path("prelude.kes"), PRELUDE.to_string(), true, entry.clone(), "prelude.kes".to_string());
    let base = files.last().expect("the prelude").base;
    let (program, parse_diags) = parse_at(PRELUDE, base, next_expr, next_bind);
    diags.extend(parse_diags);
    let names: Vec<(String, kestrel_syntax::Span)> = program.items.iter().filter_map(pub_name).collect();
    for m in modules.iter_mut().skip(1) {
        let own: Vec<String> = m.program.items.iter().filter_map(item_name).collect();
        let names: Vec<(String, kestrel_syntax::Span)> = names.iter().filter(|(n, _)| !own.contains(n)).cloned().collect();
        let Some(&(_, span)) = names.first() else { continue };
        let clause = UseClause { module: entry.clone(), module_span: span, names };
        m.program.items.push(Item::Use(UseDecl { clauses: vec![clause], span }));
        // The clause names the entry file's module, which holds the prelude's items.
        m.resolved.push(Some(0));
    }
    merge(&mut modules[0], program, Vec::new());
}

/// The name of a `pub` item, and where it is declared.
fn pub_name(item: &Item) -> Option<(String, kestrel_syntax::Span)> {
    match item {
        Item::Fn(f) if f.is_pub => Some((f.name.clone(), f.name_span)),
        Item::Struct(s) if s.is_pub => Some((s.name.clone(), s.name_span)),
        Item::Enum(e) if e.is_pub => Some((e.name.clone(), e.name_span)),
        Item::Interface(i) if i.is_pub => Some((i.name.clone(), i.name_span)),
        Item::Const(c) if c.is_pub => Some((c.bind.name.clone(), c.bind.span)),
        _ => None,
    }
}

/// An item's name (none for a `use`).
fn item_name(item: &Item) -> Option<String> {
    match item {
        Item::Fn(f) => Some(f.name.clone()),
        Item::Struct(s) => Some(s.name.clone()),
        Item::Enum(e) => Some(e.name.clone()),
        Item::Interface(i) => Some(i.name.clone()),
        Item::Const(c) => Some(c.bind.name.clone()),
        Item::Use(_) => None,
    }
}

/// `part`'s program without the `use` clauses that `module` already has (the same module and
/// names), and without a `use` left with no clause.
pub(crate) fn without_repeated_uses(mut part: Program, module: &Program) -> Program {
    let same = |a: &UseClause, b: &UseClause| a.module == b.module && a.names.iter().map(|n| &n.0).eq(b.names.iter().map(|n| &n.0));
    let had: Vec<&UseClause> = module
        .items
        .iter()
        .filter_map(|item| if let Item::Use(u) = item { Some(&u.clauses) } else { None })
        .flatten()
        .collect();
    part.items.retain_mut(|item| {
        let Item::Use(u) = item else { return true };
        u.clauses.retain(|c| !had.iter().any(|h| same(h, c)));
        !u.clauses.is_empty()
    });
    part
}

/// Adds the program of a later part of module `m` to it (see `without_repeated_uses`), with the
/// module each of its `use` clauses resolved to. Ids count up across files, so the part's counts
/// are the largest so far.
pub(crate) fn merge(m: &mut Module, part: Program, resolved: Vec<Option<usize>>) {
    m.program.items.extend(part.items);
    m.program.proves.extend(part.proves);
    m.program.hex_literals.extend(part.hex_literals);
    m.program.expr_count = part.expr_count;
    m.program.bind_count = part.bind_count;
    m.resolved.extend(resolved);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use kestrel_syntax::ast::Item;
    use kestrel_syntax::parse_at;

    use super::without_repeated_uses;
    use crate::load::load;

    #[test]
    fn the_prelude_joins_the_entry_files_module_and_no_use_finds_it() {
        let read = |p: &Path| (p == Path::new("geom.kes")).then(|| "pub f() {}\n".to_string());
        let l = load("main.kes", "use prelude\nuse geom\nmain() {}\n", &read);
        // The prelude is not a module: `use prelude` looks for a file, as any `use` does.
        assert!(l.diags.is_empty(), "{:?}", l.diags);
        let missing: Vec<&str> = l.missing.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(missing, ["can't find module `prelude`: there's no `prelude.kes` next to this file"]);
        let last = l.files.last().expect("files");
        assert_eq!((last.path.as_str(), last.builtin, last.module.as_str()), ("<builtin>/prelude.kes", true, "main"));
        // Its `Result` is the entry file's module's item, so it keeps its name when linked.
        let enums = |p: &kestrel_syntax::ast::Program| p.items.iter().filter_map(|i| if let Item::Enum(e) = i { Some(e.name.clone()) } else { None }).collect::<Vec<_>>();
        assert_eq!(enums(&l.modules[0].program), ["Result"]);
        // `geom` sees it through a `use` it never wrote, of the entry file's module.
        let Some(Item::Use(u)) = l.modules[1].program.items.last() else { panic!("no `use` added") };
        assert_eq!(u.clauses[0].names.iter().map(|n| n.0.as_str()).collect::<Vec<_>>(), ["Result"]);
        assert_eq!(l.modules[1].resolved, [Some(0)]);
    }

    #[test]
    fn a_use_clause_a_part_repeats_is_kept_once() {
        let (module, _) = parse_at("use random\nuse { Rng from random }\n", 0, 0, 0);
        let (part, _) = parse_at("use { Rng from random }\nuse { Rng, randf from random }\nuse window\n", 100, module.expr_count, module.bind_count);
        let kept: Vec<String> = without_repeated_uses(part, &module)
            .items
            .iter()
            .filter_map(|i| if let Item::Use(u) = i { Some(u.clauses.iter().map(|c| format!("{} {}", c.module, c.names.len())).collect::<Vec<_>>().join(", ")) } else { None })
            .collect();
        assert_eq!(kept, ["random 2", "window 0"]);
    }
}
