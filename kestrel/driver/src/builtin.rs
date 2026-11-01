//! The modules that ship inside the compiler: `window`, `draw`, `draw3d`, `input`, `random`,
//! `file`, `time`, `args`, `env` and `rollback`. They are ordinary Kestrel files, embedded here;
//! all but `random` and `rollback` have bodyless `native` functions that Rust implements (the
//! window ones in `kestrel-window`, the other four modules' in `kestrel-runtime`). A program loads
//! one with `use window`, exactly as it would a file next to it. A module may be several files
//! (its parts, like `rollback`'s): each is parsed on its own, so messages name the part and its
//! line, and the loader merges them into one module.

use std::path::Path;

/// The directory name of an embedded module's virtual path.
const DIR: &str = "<builtin>";

/// One file of a built-in module: its path under `<builtin>/`, and its source.
pub(crate) type Part = (&'static str, &'static str);

/// Each built-in module: its name, and its parts, the first being the module's own file.
const MODULES: [(&str, &[Part]); 10] = [
    ("window", &[("window.kes", include_str!("../std/window.kes"))]),
    ("draw", &[("draw.kes", include_str!("../std/draw.kes"))]),
    ("draw3d", &[("draw3d.kes", include_str!("../std/draw3d.kes"))]),
    ("input", &[("input.kes", include_str!("../std/input.kes"))]),
    ("random", &[("random.kes", include_str!("../std/random.kes"))]),
    ("file", &[("file.kes", include_str!("../std/file.kes"))]),
    ("time", &[("time.kes", include_str!("../std/time.kes"))]),
    ("args", &[("args.kes", include_str!("../std/args.kes"))]),
    ("env", &[("env.kes", include_str!("../std/env.kes"))]),
    (
        "rollback",
        &[
            ("rollback.kes", include_str!("../std/rollback.kes")),
            ("rollback/session.kes", include_str!("../std/rollback/session.kes")),
            ("rollback/replay.kes", include_str!("../std/rollback/replay.kes")),
            ("rollback/fakenet.kes", include_str!("../std/rollback/fakenet.kes")),
        ],
    ),
];

/// Whether a program that uses built-in module `name` needs the window library (the window,
/// drawing and input natives). `random` is plain Kestrel, and the natives of `file`, `time`, `args`
/// and `env` are the runtime's, so those need only the runtime.
pub fn needs_window(name: &str) -> bool {
    matches!(name, "window" | "draw" | "draw3d" | "input")
}

/// The built-in module called `name`, as its name and parts.
pub(crate) fn find(name: &str) -> Option<(&'static str, &'static [Part])> {
    MODULES.iter().copied().find(|(n, _)| *n == name)
}

/// The path shown in diagnostics for a built-in module's part, from its path under `<builtin>/`:
/// `<builtin>/draw.kes`, `<builtin>/rollback/session.kes`.
pub(crate) fn path(part: &str) -> String {
    format!("{DIR}/{part}")
}

/// Whether `path` is the virtual path of a built-in module (see `path`).
pub(crate) fn is_path(path: &Path) -> bool {
    path.starts_with(DIR)
}

/// The error for a file `name.kes` that sits where a `use name` would find it. When that file
/// is the program itself (`is_program`), it is the program that has to be renamed.
pub(crate) fn shadowed(name: &str, is_program: bool) -> String {
    if is_program {
        format!("`{name}` is a built-in module, so a program can't be called {name}.kes; rename the program")
    } else {
        format!("`{name}` is a built-in module, so {name}.kes can't be used as a module; rename the file")
    }
}

/// The error for a dotted `use` whose first name is built-in module `name` (`use draw.shapes`):
/// a dotted name is never a built-in module, and a folder named like one would hide it.
pub(crate) fn folder_named(name: &str) -> String {
    format!("`{name}` is a built-in module, so a folder can't be called {name}; rename the folder")
}

/// The error for a `native` declaration in a file the user wrote.
pub(crate) const USER_NATIVE: &str = "only Kestrel's built-in modules can declare native functions";

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{find, folder_named, is_path, path, shadowed};

    #[test]
    fn every_module_is_found_by_name() {
        for name in ["window", "draw", "draw3d", "input", "random", "file", "time", "args", "env", "rollback"] {
            let (found, parts) = find(name).expect(name);
            assert_eq!(found, name);
            // The first part is the module's own file; the others are in its folder.
            assert_eq!(parts[0].0, format!("{name}.kes"));
            for (part, src) in parts {
                assert!(src.starts_with("//"), "{part} starts with a comment");
                assert!(*part == parts[0].0 || part.starts_with(&format!("{name}/")), "{part} is in the folder {name}/");
            }
        }
        assert_eq!(find("rollback").expect("rollback").1.len(), 4);
        assert!(find("geom").is_none());
    }

    #[test]
    fn the_shadowing_error_says_which_file_to_rename() {
        assert!(shadowed("draw", false).ends_with("draw.kes can't be used as a module; rename the file"));
        assert!(shadowed("draw", true).ends_with("a program can't be called draw.kes; rename the program"));
        assert_eq!(folder_named("draw"), "`draw` is a built-in module, so a folder can't be called draw; rename the folder");
    }

    #[test]
    fn virtual_paths_are_recognised() {
        assert!(is_path(Path::new(&path("draw.kes"))));
        assert_eq!(path("rollback/session.kes"), "<builtin>/rollback/session.kes");
        assert!(!is_path(Path::new("examples/draw.kes")));
        assert!(!is_path(Path::new("builtin/draw.kes")));
    }
}
