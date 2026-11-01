//! What an editor needs from a program: every diagnostic `kestrel check` gives before lowering,
//! with its span, and the linked program and the checker's tables (even when the check found
//! errors), to answer "what is this" and "where is it defined".

use kestrel_check::Tables;
use kestrel_syntax::ast::{Item, Program, UseDecl};
use kestrel_syntax::link::link;
use kestrel_syntax::Diagnostic;

use crate::{builtin_ranges, load, Loaded, Read, Source};

pub struct Analysis {
    /// The entry file first, then each module it uses (built-in ones included).
    pub sources: Vec<Source>,
    /// Each module's `use` declarations, by module, the entry file's first (linking drops them
    /// from `program`). A module is one source, except a built-in module of several parts (like
    /// `rollback`), which is one module of several sources.
    pub uses: Vec<Vec<UseDecl>>,
    /// The module each `use` clause names, by module in the order the clauses are written (see
    /// `kestrel_syntax::link::Module::resolved`), as the index of its source (for a built-in
    /// module of several parts, its first part's).
    pub resolved: Vec<Vec<Option<usize>>>,
    /// The linked program, when every file parsed.
    pub program: Option<Program>,
    /// The checker's tables, when the program linked. They may come with errors (see
    /// `kestrel_check::check_keeping_tables`).
    pub tables: Option<Tables>,
    /// Every diagnostic, with its span in the shared span space of `sources`, in the order
    /// `kestrel check` gives them.
    pub diags: Vec<Diagnostic>,
}

impl Analysis {
    /// The index of the source the shared offset `offset` falls in.
    pub fn source_at(&self, offset: u32) -> usize {
        self.sources.iter().rposition(|s| s.base <= offset).unwrap_or(0)
    }
}

/// Parses, links and checks the program whose entry file is `path` (with text `src`), reading
/// the modules it uses with `read`. Each stage runs only when the one before it found no error,
/// as for `kestrel check`. With `needs_main` false the entry file is checked as a module.
pub fn analyze(path: &str, src: &str, needs_main: bool, read: Read) -> Analysis {
    let Loaded { files, modules, first_file, mut diags, missing, .. } = load(path, src, read);
    let uses = modules.iter().map(|m| use_decls(&m.program)).collect();
    let resolved = modules.iter().map(|m| m.resolved.iter().map(|r| r.map(|m| first_file[m])).collect()).collect();
    if diags.iter().any(Diagnostic::is_error) {
        return Analysis { sources: files, uses, resolved, program: None, tables: None, diags };
    }
    diags.extend(missing);
    let (mut program, link_diags) = link(modules, kestrel_check::is_reserved_name);
    program.builtin_ranges = builtin_ranges(&files);
    diags.extend(link_diags);
    let tables = if diags.iter().any(Diagnostic::is_error) {
        None
    } else {
        let (tables, check_diags) = kestrel_check::check_keeping_tables(&program, needs_main);
        diags.extend(check_diags);
        Some(tables)
    };
    Analysis { sources: files, uses, resolved, program: Some(program), tables, diags }
}

fn use_decls(p: &Program) -> Vec<UseDecl> {
    p.items
        .iter()
        .filter_map(|i| match i {
            Item::Use(u) => Some(u.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::analyze;

    fn none(_: &Path) -> Option<String> {
        None
    }

    #[test]
    fn a_clean_program_has_tables_and_no_diagnostics() {
        let a = analyze("main.kes", "main() {\n    let x = 1\n}\n", true, &none);
        assert!(a.diags.is_empty(), "{:?}", a.diags);
        assert!(a.tables.is_some());
    }

    #[test]
    fn tables_come_with_check_errors() {
        let a = analyze("main.kes", "main() {\n    let x: int = true\n    let y = 2.5\n}\n", true, &none);
        assert_eq!(a.diags.len(), 1, "{:?}", a.diags);
        let t = a.tables.expect("tables");
        assert!(t.locals.iter().any(|l| l.name == "y" && l.ty == kestrel_check::Ty::Float));
    }

    #[test]
    fn a_parse_error_stops_before_linking() {
        let a = analyze("main.kes", "main() {\n    let = 1\n}\n", true, &none);
        assert!(a.diags.iter().any(|d| d.is_error()));
        assert!(a.program.is_none() && a.tables.is_none());
    }

    #[test]
    fn modules_are_read_with_the_reader() {
        let read = |p: &Path| (p.file_name()? == "geom.kes").then(|| "pub area(w: int) -> int {\n    w * w\n}\n".to_string());
        let a = analyze("dir/main.kes", "use geom\nmain() {\n    print(geom.area(2))\n}\n", true, &read);
        assert!(a.diags.is_empty(), "{:?}", a.diags);
        assert_eq!(a.sources.len(), 2);
        assert!(a.sources[1].path.ends_with("geom.kes"));
        assert_eq!(a.source_at(a.sources[1].base + 3), 1);
        assert_eq!(a.uses[0].len(), 1);
    }
}
