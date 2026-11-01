//! `kestrel build` finds the runtime library (and, for window programs, the
//! window library) next to the executable, so a copied binary keeps working.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LIB_FILE: &str = if cfg!(windows) { "kestrel_runtime.lib" } else { "libkestrel_runtime.a" };
const WINDOW_LIB_FILE: &str = if cfg!(windows) { "kestrel_window.lib" } else { "libkestrel_window.a" };

/// Where the build left the window library. Only Windows builds have one; elsewhere the window
/// tests are ignored.
#[cfg(windows)]
const BUILT_WINDOW_LIB: &str = env!("KESTREL_WINDOW_LIB");
#[cfg(not(windows))]
const BUILT_WINDOW_LIB: &str = "";

/// A fresh directory holding only a copy of the `kestrel` binary.
fn relocated_binary(name: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let exe = dir.join(format!("kestrel{}", std::env::consts::EXE_SUFFIX));
    fs::copy(env!("CARGO_BIN_EXE_kestrel"), &exe).unwrap();
    (dir, exe)
}

/// Builds `tests/programs/<program>.kes` with the copied `kestrel`; the exe lands in `dir`.
fn build_program(kestrel: &Path, dir: &Path, program: &str) -> Output {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../tests/programs/{program}.kes"));
    let out = dir.join(format!("{program}{}", std::env::consts::EXE_SUFFIX));
    Command::new(kestrel).arg("build").arg(src).arg("-o").arg(out).output().unwrap()
}

fn build_hello(kestrel: &Path, dir: &Path) -> Output {
    build_program(kestrel, dir, "hello")
}

/// A program that uses `draw` and `input`, so it needs the window library.
fn build_window_program(kestrel: &Path, dir: &Path) -> Output {
    build_program(kestrel, dir, "builtin_modules")
}

#[test]
fn copied_binary_falls_back_to_the_library_it_was_built_with() {
    let (dir, kestrel) = relocated_binary("relocated_alone");
    let out = build_hello(&kestrel, &dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn copied_binary_uses_the_library_next_to_it() {
    let (dir, kestrel) = relocated_binary("relocated_with_lib");
    fs::copy(env!("KESTREL_RUNTIME_LIB"), dir.join(LIB_FILE)).unwrap();
    let out = build_hello(&kestrel, &dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn library_next_to_the_binary_wins_over_the_built_in_path() {
    let (dir, kestrel) = relocated_binary("relocated_bad_lib");
    fs::write(dir.join(LIB_FILE), b"not a library").unwrap();
    let out = build_hello(&kestrel, &dir);
    assert!(!out.status.success(), "the bogus library next to the binary should have been linked");
    assert!(String::from_utf8_lossy(&out.stderr).contains("linking failed"));
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn copied_binary_falls_back_to_the_window_library_it_was_built_with() {
    let (dir, kestrel) = relocated_binary("relocated_window_alone");
    let out = build_window_program(&kestrel, &dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn copied_binary_uses_the_window_library_next_to_it() {
    let (dir, kestrel) = relocated_binary("relocated_with_window_lib");
    fs::copy(BUILT_WINDOW_LIB, dir.join(WINDOW_LIB_FILE)).unwrap();
    let out = build_window_program(&kestrel, &dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let exe = dir.join(format!("builtin_modules{}", std::env::consts::EXE_SUFFIX));
    let run = Command::new(exe).env("KESTREL_POISON", "1").output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn window_library_next_to_the_binary_wins_over_the_built_in_path() {
    let (dir, kestrel) = relocated_binary("relocated_bad_window_lib");
    fs::write(dir.join(WINDOW_LIB_FILE), b"not a library").unwrap();
    let out = build_window_program(&kestrel, &dir);
    assert!(!out.status.success(), "the bogus window library next to the binary should have been linked");
    assert!(String::from_utf8_lossy(&out.stderr).contains("linking failed"));
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn each_program_links_only_the_library_it_needs() {
    // A plain program never touches the window library, and a window program never touches
    // the runtime library on its own: it carries the runtime inside the window library.
    let (dir, kestrel) = relocated_binary("relocated_other_lib_bad");
    fs::write(dir.join(WINDOW_LIB_FILE), b"not a library").unwrap();
    let out = build_hello(&kestrel, &dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let (dir, kestrel) = relocated_binary("relocated_runtime_lib_bad");
    fs::write(dir.join(LIB_FILE), b"not a library").unwrap();
    let out = build_window_program(&kestrel, &dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}
