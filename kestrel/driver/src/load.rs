//! Finds and parses every file of a program: the entry file, then each module a file uses, once
//! each (the rollback library design, decisions 12 and 13).
//!
//! In a file whose folder is `D`, `use a.b` is `D/a/b.kes`, else `D/a/b/b.kes` (a folder's own
//! file), and `use net` is `D/net.kes`, else `D/net/net.kes`; a single name may be a built-in
//! module instead. Names never go up, so every file is in the entry file's folder or below it,
//! and a module's full name is its path from there (`net/packet.kes` is `net.packet`, and
//! `net/net.kes` is `net`). A file is loaded once, keyed by its canonical path, or by its path
//! made lexically normal when it isn't on disk (an editor's unsaved buffer, a test's text).
//!
//! A built-in module of several parts (`rollback`) is a `Source` per part, so messages name the
//! part, and one `Module`: the parts' items are merged, so they see each other's as one file's.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use kestrel_syntax::ast::{Item, Program, UseClause};
use kestrel_syntax::link::Module;
use kestrel_syntax::{parse_at, Diagnostic};

use crate::{base_name, builtin, push_file, stem, user_natives, Read, Source};

/// What `load` read: every file, and the modules parsed from them.
pub(crate) struct Loaded {
    pub files: Vec<Source>,
    /// The modules, the entry file first. A module is one file, except a built-in module of
    /// several parts, which is one module of several files; `Module::resolved` is by module.
    pub modules: Vec<Module>,
    /// The index in `files` of each module's first file (its own file, for a built-in of parts).
    pub first_file: Vec<usize>,
    /// Errors that stop the compile before linking (a file that doesn't parse, ...).
    pub diags: Vec<Diagnostic>,
    /// The modules that couldn't be found, as errors. Linking goes on after them, so that it can
    /// say what else is wrong.
    pub missing: Vec<Diagnostic>,
    pub builtins: Vec<&'static str>,
}

const ABOVE: &str = "a module finds modules next to it or in folders below it, never above";

/// Parses the entry file `path` (with text `src`) and, transitively, every module it uses,
/// reading each module file with `read`.
pub(crate) fn load(path: &str, src: &str, read: Read) -> Loaded {
    let entry = Source { path: path.to_string(), src: src.to_string(), base: 0, builtin: false, module: stem(path), file: base_name(path) };
    let mut l = Loader {
        read,
        texts: HashMap::new(),
        files: vec![entry],
        folders: vec![Vec::new()],
        by_key: HashMap::from([(key(Path::new(path)), 0)]),
        by_builtin: HashMap::new(),
        file_module: vec![0],
        first_file: vec![0],
        diags: Vec::new(),
        missing: Vec::new(),
        builtins: Vec::new(),
    };
    let mut modules: Vec<Module> = Vec::new();
    let (mut next_expr, mut next_bind) = (0, 0);
    let mut i = 0;
    while i < l.files.len() {
        let (program, file_diags) = parse_at(&l.files[i].src, l.files[i].base, next_expr, next_bind);
        (next_expr, next_bind) = (program.expr_count, program.bind_count);
        l.diags.extend(file_diags);
        if !l.files[i].builtin {
            l.diags.extend(user_natives(&program.items));
        }
        let mi = l.file_module[i];
        // A later part of a built-in module: its items join the module's, and a `use` clause the
        // module already has is kept once.
        let program = match modules.get(mi) {
            Some(m) => without_repeated_uses(program, &m.program),
            None => program,
        };
        let mut resolved = Vec::new();
        for item in &program.items {
            let Item::Use(u) = item else { continue };
            for c in &u.clauses {
                resolved.push(l.resolve(i, c));
            }
        }
        match modules.get_mut(mi) {
            Some(m) => merge(m, program, resolved),
            None => {
                let f = &l.files[i];
                modules.push(Module { name: f.module.clone(), file: f.file.clone(), program, resolved });
            }
        }
        i += 1;
    }
    Loaded { files: l.files, modules, first_file: l.first_file, diags: l.diags, missing: l.missing, builtins: l.builtins }
}

/// `part`'s program without the `use` clauses that `module` already has (the same module and
/// names), and without a `use` left with no clause.
fn without_repeated_uses(mut part: Program, module: &Program) -> Program {
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
fn merge(m: &mut Module, part: Program, resolved: Vec<Option<usize>>) {
    m.program.items.extend(part.items);
    m.program.proves.extend(part.proves);
    m.program.hex_literals.extend(part.hex_literals);
    m.program.expr_count = part.expr_count;
    m.program.bind_count = part.bind_count;
    m.resolved.extend(resolved);
}

struct Loader<'a> {
    read: Read<'a>,
    /// Every path read so far, and what it held: each is read once.
    texts: HashMap<PathBuf, Option<String>>,
    files: Vec<Source>,
    /// Each file's folder from the entry file's folder, by name.
    folders: Vec<Vec<String>>,
    /// The user modules loaded, by their file's key (see `key`).
    by_key: HashMap<PathBuf, usize>,
    /// The built-in modules loaded, by name.
    by_builtin: HashMap<&'static str, usize>,
    /// Each file's module, by index: files and modules differ only for a built-in of parts.
    file_module: Vec<usize>,
    /// Each module's first file.
    first_file: Vec<usize>,
    diags: Vec<Diagnostic>,
    missing: Vec<Diagnostic>,
    builtins: Vec<&'static str>,
}

impl Loader<'_> {
    fn read(&mut self, path: &Path) -> Option<String> {
        let read = self.read;
        self.texts.entry(path.to_path_buf()).or_insert_with(|| read(path)).clone()
    }

    /// The module clause `c` of file `i` names, loading it if it's new; `None` (after an error)
    /// when there's none.
    fn resolve(&mut self, i: usize, c: &UseClause) -> Option<usize> {
        let dir = Path::new(&self.files[i].path).parent().map(Path::to_path_buf).unwrap_or_default();
        let parts: Vec<&str> = c.module.split('.').collect();
        let (first, last) = (parts[0], parts[parts.len() - 1]);
        if parts.len() == 1 {
            if let Some((name, parts)) = builtin::find(first) {
                return Some(self.builtin(i, &dir, c, name, parts));
            }
        } else if builtin::find(first).is_some() {
            self.missing.push(Diagnostic::error(c.module_span, builtin::folder_named(first)));
            return None;
        }
        let folder = parts[..parts.len() - 1].iter().fold(dir.clone(), |d, p| d.join(p));
        let own_folder = folder.join(last);
        let (flat, own) = (folder.join(format!("{last}.kes")), own_folder.join(format!("{last}.kes")));
        let here = self.folders[i].clone();
        let mut place: Vec<String> = here.iter().cloned().chain(parts[..parts.len() - 1].iter().map(|p| p.to_string())).collect();
        let (path, text) = match self.read(&flat) {
            // An editor's reader may give the same text for any file of that name, so the folder's
            // own file is looked for on disk here; it is read too, so that hot reload watches it
            // (deleting it fixes the error).
            Some(_) if own.is_file() => {
                self.read(&own);
                let msg = format!("`{0}` could be {1}.kes or {1}/{2}.kes; remove or rename one of them", c.module, parts.join("/"), last);
                self.missing.push(Diagnostic::error(c.module_span, msg));
                return None;
            }
            Some(text) => (flat, text),
            None => match self.read(&own) {
                Some(text) => {
                    place.push(last.to_string());
                    (own, text)
                }
                None => {
                    let d = Diagnostic::error(c.module_span, not_found(&c.module, &parts, &own_folder));
                    let above = !self.files[i].builtin && is_above(&dir, here.len(), &parts);
                    self.missing.push(if above { d.with_help(ABOVE) } else { d });
                    return None;
                }
            },
        };
        let k = key(&path);
        if let Some(&found) = self.by_key.get(&k) {
            return Some(found);
        }
        let full = full_name(&place, last);
        let file = place.iter().map(String::as_str).chain([format!("{last}.kes").as_str()]).collect::<Vec<_>>().join("/");
        let module = self.first_file.len();
        self.by_key.insert(k, module);
        self.folders.push(place);
        self.file_module.push(module);
        self.first_file.push(self.files.len());
        push_file(&mut self.files, path.to_string_lossy().into_owned(), text, false, full, file);
        Some(module)
    }

    /// Built-in module `name` (with its `parts`) used by file `i`, in folder `dir`. A built-in
    /// module wins over a file of the same name; say so rather than quietly ignoring the file.
    /// (The file may be the program itself.)
    fn builtin(&mut self, i: usize, dir: &Path, c: &UseClause, name: &'static str, parts: &[builtin::Part]) -> usize {
        let clash = dir.join(format!("{name}.kes"));
        if !self.files[i].builtin && clash.exists() {
            let is_program = same_file(&clash, Path::new(&self.files[0].path));
            self.diags.push(Diagnostic::error(c.module_span, builtin::shadowed(name, is_program)));
        }
        if let Some(&found) = self.by_builtin.get(name) {
            return found;
        }
        let module = self.first_file.len();
        self.builtins.push(name);
        self.by_builtin.insert(name, module);
        self.first_file.push(self.files.len());
        for (part, text) in parts {
            self.folders.push(Vec::new());
            self.file_module.push(module);
            push_file(&mut self.files, builtin::path(part), text.to_string(), true, name.to_string(), part.to_string());
        }
        module
    }
}

/// The full name of the file `last.kes` in folder `place` (from the entry file's folder): its
/// path with `.` for `/` and without `.kes`, where a folder's own file `x/x.kes` is `x`. It comes
/// from the path alone, so it is the same however the file was reached.
fn full_name(place: &[String], last: &str) -> String {
    let mut names: Vec<&str> = place.iter().map(String::as_str).collect();
    if names.last() != Some(&last) {
        names.push(last);
    }
    names.join(".")
}

/// Whether the module named `parts`, missing next to a file in folder `dir` (`depth` folders
/// below the entry file's), is in one of the folders above it, up to the entry file's.
fn is_above(dir: &Path, depth: usize, parts: &[&str]) -> bool {
    let last = parts[parts.len() - 1];
    dir.ancestors().skip(1).take(depth).any(|up| {
        let folder = parts.iter().fold(up.to_path_buf(), |d, p| d.join(p));
        folder.with_extension("kes").is_file() || folder.join(format!("{last}.kes")).is_file()
    })
}

/// Why module `module` (its names `parts`) isn't there: `folder` is the folder named like it,
/// which may hold other modules but not its own file.
fn not_found(module: &str, parts: &[&str], folder: &Path) -> String {
    let last = parts[parts.len() - 1];
    if folder.is_dir() {
        let path = parts.join("/");
        let mut inside: Vec<String> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".kes").map(str::to_string))
            .collect();
        inside.sort();
        return match inside.first() {
            Some(m) => format!("can't find module `{module}`: the folder {path}/ has no {last}.kes; add one, or use a module inside it, like `use {module}.{m}`"),
            None => format!("can't find module `{module}`: the folder {path}/ has no {last}.kes; add one"),
        };
    }
    if parts.len() == 1 {
        format!("can't find module `{module}`: there's no `{module}.kes` next to this file")
    } else {
        format!("can't find module `{module}`: there's no {}.kes next to this file", parts.join("/"))
    }
}

/// The key a file is loaded once by: its canonical path, or its path made lexically normal when
/// it isn't on disk.
fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| lexical(path))
}

/// `path` without `.` parts, and with each `..` taking away the name before it.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `a` and `b` are the same file on disk.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use kestrel_syntax::ast::Item;
    use kestrel_syntax::parse_at;

    use super::{full_name, lexical, load, without_repeated_uses};

    #[test]
    fn a_full_name_comes_from_the_path_alone() {
        let place = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(full_name(&place(&["net"]), "packet"), "net.packet");
        // `net/net.kes`, whether it was reached as the folder's own file or from inside `net/`.
        assert_eq!(full_name(&place(&["net"]), "net"), "net");
        assert_eq!(full_name(&place(&[]), "geom"), "geom");
    }

    #[test]
    fn a_path_not_on_disk_is_keyed_lexically() {
        assert_eq!(lexical(Path::new("dir/./net/../net/packet.kes")), Path::new("dir/net/packet.kes"));
        assert_eq!(lexical(Path::new("../a.kes")), Path::new("../a.kes"));
    }

    #[test]
    fn one_file_reached_by_two_names_is_loaded_once() {
        // `wire` is `net.wire` from the entry and `wire` from `net/packet.kes`, in memory only.
        let read = |p: &Path| {
            let p = p.to_string_lossy().replace('\\', "/");
            match p.as_str() {
                "game/net/packet.kes" => Some("use wire\npub f() {}\n".to_string()),
                "game/net/wire.kes" => Some("pub g() {}\n".to_string()),
                _ => None,
            }
        };
        let l = load("game/main.kes", "use net.packet\nuse net.wire\nmain() {}\n", &read);
        assert!(l.diags.is_empty() && l.missing.is_empty(), "{:?} {:?}", l.diags, l.missing);
        let names: Vec<(&str, &str)> = l.files.iter().map(|f| (f.module.as_str(), f.file.as_str())).collect();
        assert_eq!(names, [("main", "main.kes"), ("net.packet", "net/packet.kes"), ("net.wire", "net/wire.kes")]);
        assert_eq!(l.modules[0].resolved, [Some(1), Some(2)]);
        assert_eq!(l.modules[1].resolved, [Some(2)]);
        // Each file keeps the path it was reached by, for its diagnostics.
        assert_eq!(l.files[1].path.replace('\\', "/"), "game/net/packet.kes");
    }

    #[test]
    fn a_built_in_module_of_parts_is_one_module_of_several_files() {
        let l = load("main.kes", "use rollback\nuse geom\nmain() {}\n", &|p: &Path| (p == Path::new("geom.kes")).then(|| "use rollback\n".to_string()));
        assert!(l.diags.is_empty() && l.missing.is_empty(), "{:?} {:?}", l.diags, l.missing);
        let files: Vec<(&str, &str, &str)> = l.files.iter().map(|f| (f.path.as_str(), f.module.as_str(), f.file.as_str())).collect();
        assert_eq!(&files[1..], [
            ("<builtin>/rollback.kes", "rollback", "rollback.kes"),
            ("<builtin>/rollback/session.kes", "rollback", "rollback/session.kes"),
            ("<builtin>/rollback/replay.kes", "rollback", "rollback/replay.kes"),
            ("<builtin>/rollback/fakenet.kes", "rollback", "rollback/fakenet.kes"),
            ("geom.kes", "geom", "geom.kes"),
            // `FakeNet`'s `Rng`.
            ("<builtin>/random.kes", "random", "random.kes"),
        ]);
        assert!(l.files[1..5].iter().all(|f| f.builtin));
        // One module: the entry's and `geom`'s clauses both name it, by module index, and `geom`
        // is the module after it though it is the sixth file. `random` comes last, used by
        // `rollback/fakenet.kes`.
        assert_eq!(l.modules.len(), 4);
        assert_eq!(l.first_file, [0, 1, 5, 6]);
        assert_eq!(l.modules[0].resolved, [Some(1), Some(2)]);
        assert_eq!(l.modules[1].resolved, [Some(3)]);
        assert_eq!(l.modules[2].resolved, [Some(1)]);
        assert_eq!((l.modules[1].name.as_str(), l.modules[2].name.as_str(), l.modules[3].name.as_str()), ("rollback", "geom", "random"));
        assert_eq!(l.builtins, ["rollback", "random"]);
        // Its items are every part's, and its ids count to the last part's.
        let rollback = &l.modules[1];
        let names: Vec<&str> = rollback.program.items.iter().filter_map(|i| if let Item::Struct(s) = i { Some(s.name.as_str()) } else { None }).collect();
        assert_eq!(names, ["Input", "Packet", "Session", "Replay", "FakeNet"]);
        let counts = l.files[1..5].iter().fold((l.modules[0].program.expr_count, l.modules[0].program.bind_count), |(e, b), f| {
            let (p, _) = parse_at(&f.src, f.base, e, b);
            (p.expr_count, p.bind_count)
        });
        assert_eq!((rollback.program.expr_count, rollback.program.bind_count), counts);
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
