//! Finds and parses every file of a program: the entry file, then each module a file uses, once
//! each (the rollback library design, decisions 12 and 13).
//!
//! In a file whose folder is `D`, `use a.b` is `D/a/b.kes`, else `D/a/b/b.kes` (a folder's own
//! file), and `use net` is `D/net.kes`, else `D/net/net.kes`; a single name may be a built-in
//! module instead. Names never go up, so every file is in the entry file's folder or below it,
//! and a module's full name is its path from there (`net/packet.kes` is `net.packet`, and
//! `net/net.kes` is `net`). A file is loaded once, keyed by its canonical path, or by its path
//! made lexically normal when it isn't on disk (an editor's unsaved buffer, a test's text).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use kestrel_syntax::ast::{Item, UseClause};
use kestrel_syntax::link::Module;
use kestrel_syntax::{parse_at, Diagnostic};

use crate::{base_name, builtin, push_file, stem, user_natives, Read, Source};

/// What `load` read: every file, and the modules parsed from them.
pub(crate) struct Loaded {
    pub files: Vec<Source>,
    pub modules: Vec<Module>,
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
        diags: Vec::new(),
        missing: Vec::new(),
        builtins: Vec::new(),
    };
    let mut modules = Vec::new();
    let (mut next_expr, mut next_bind) = (0, 0);
    let mut i = 0;
    while i < l.files.len() {
        let (program, file_diags) = parse_at(&l.files[i].src, l.files[i].base, next_expr, next_bind);
        (next_expr, next_bind) = (program.expr_count, program.bind_count);
        l.diags.extend(file_diags);
        if !l.files[i].builtin {
            l.diags.extend(user_natives(&program.items));
        }
        let mut resolved = Vec::new();
        for item in &program.items {
            let Item::Use(u) = item else { continue };
            for c in &u.clauses {
                resolved.push(l.resolve(i, c));
            }
        }
        let f = &l.files[i];
        modules.push(Module { name: f.module.clone(), file: f.file.clone(), program, resolved });
        i += 1;
    }
    Loaded { files: l.files, modules, diags: l.diags, missing: l.missing, builtins: l.builtins }
}

struct Loader<'a> {
    read: Read<'a>,
    /// Every path read so far, and what it held: each is read once.
    texts: HashMap<PathBuf, Option<String>>,
    files: Vec<Source>,
    /// Each file's folder from the entry file's folder, by name.
    folders: Vec<Vec<String>>,
    /// The user files loaded, by key (see `key`).
    by_key: HashMap<PathBuf, usize>,
    by_builtin: HashMap<&'static str, usize>,
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
            if let Some((name, text)) = builtin::find(first) {
                return Some(self.builtin(i, &dir, c, name, text));
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
        self.by_key.insert(k, self.files.len());
        self.folders.push(place);
        push_file(&mut self.files, path.to_string_lossy().into_owned(), text, false, full, file);
        Some(self.files.len() - 1)
    }

    /// Built-in module `name` (with source `text`) used by file `i`, in folder `dir`. A built-in
    /// module wins over a file of the same name; say so rather than quietly ignoring the file.
    /// (The file may be the program itself.)
    fn builtin(&mut self, i: usize, dir: &Path, c: &UseClause, name: &'static str, text: &'static str) -> usize {
        let clash = dir.join(format!("{name}.kes"));
        if !self.files[i].builtin && clash.exists() {
            let is_program = same_file(&clash, Path::new(&self.files[0].path));
            self.diags.push(Diagnostic::error(c.module_span, builtin::shadowed(name, is_program)));
        }
        if let Some(&found) = self.by_builtin.get(name) {
            return found;
        }
        self.builtins.push(name);
        self.by_builtin.insert(name, self.files.len());
        self.folders.push(Vec::new());
        push_file(&mut self.files, builtin::path(name), text.to_string(), true, name.to_string(), format!("{name}.kes"));
        self.files.len() - 1
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

    use super::{full_name, lexical, load};

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
}
