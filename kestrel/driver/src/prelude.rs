//! How files join a module: a later part of a built-in module of several parts joins the module of
//! its first part (see `load`).

use kestrel_syntax::ast::{Item, Program, UseClause};
use kestrel_syntax::link::Module;

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
    use kestrel_syntax::ast::Item;
    use kestrel_syntax::parse_at;

    use super::without_repeated_uses;

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
