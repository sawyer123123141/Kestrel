//! The modules that ship inside the compiler: `window`, `draw`, `draw3d`, `input`, `random`,
//! `file`, `time`, `args` and `env`. They are ordinary Kestrel files, embedded here; all but
//! `random` have bodyless `native` functions that Rust implements (the window ones in
//! `kestrel-window`, the other four modules' in `kestrel-runtime`). A program loads one with
//! `use window`, exactly as it would a file next to it.

use std::path::Path;

/// The directory name of an embedded module's virtual path.
const DIR: &str = "<builtin>";

/// Each built-in module: its name, and its source.
const MODULES: [(&str, &str); 9] = [
    ("window", include_str!("../std/window.kes")),
    ("draw", include_str!("../std/draw.kes")),
    ("draw3d", include_str!("../std/draw3d.kes")),
    ("input", include_str!("../std/input.kes")),
    ("random", include_str!("../std/random.kes")),
    ("file", include_str!("../std/file.kes")),
    ("time", include_str!("../std/time.kes")),
    ("args", include_str!("../std/args.kes")),
    ("env", include_str!("../std/env.kes")),
];

/// Whether a program that uses built-in module `name` needs the window library (the window,
/// drawing and input natives). `random` is plain Kestrel, and the natives of `file`, `time`, `args`
/// and `env` are the runtime's, so those need only the runtime.
pub fn needs_window(name: &str) -> bool {
    matches!(name, "window" | "draw" | "draw3d" | "input")
}

/// The built-in module called `name`, as its name and source.
pub(crate) fn find(name: &str) -> Option<(&'static str, &'static str)> {
    MODULES.iter().copied().find(|(n, _)| *n == name)
}

/// The path shown for built-in module `name` in diagnostics: `<builtin>/draw.kes`.
pub(crate) fn path(name: &str) -> String {
    format!("{DIR}/{name}.kes")
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
        for name in ["window", "draw", "draw3d", "input", "random", "file", "time", "args", "env"] {
            let (found, src) = find(name).expect(name);
            assert_eq!(found, name);
            assert!(src.starts_with("//"), "{name} starts with a comment");
        }
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
        assert!(is_path(Path::new(&path("draw"))));
        assert!(!is_path(Path::new("examples/draw.kes")));
        assert!(!is_path(Path::new("builtin/draw.kes")));
    }
}
