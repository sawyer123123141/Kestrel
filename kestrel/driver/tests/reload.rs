//! `compile_for_reload`: what `kestrel run` does from reading the entry file to `prove`, for the
//! reloading host's compile thread.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use kestrel_driver::{compile_for_reload, DiagStyle};

/// A fresh folder for one test, with `files` written into it.
fn folder(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kestrel-reload-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    dir
}

fn time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[test]
fn the_files_carry_the_times_read_before_the_compile() {
    let dir = folder(
        "times",
        &[
            ("game.kes", "\u{feff}use geom\nuse draw\nmain() { print(geom.twice(21)) }\n"),
            ("geom.kes", "pub twice(n: int) -> int { n * 2 }\n"),
        ],
    );
    let (game, geom) = (dir.join("game.kes"), dir.join("geom.kes"));
    let before = (time(&game), time(&geom));
    let (c, warnings) = compile_for_reload(&game, DiagStyle::Full, false).unwrap();
    assert_eq!(warnings, "", "a clean program has no warnings to print");
    assert_eq!(c.files[0].path, game, "the entry file comes first");
    assert_eq!(c.files[0].modified, before.0);
    let g = c.files.iter().find(|f| f.path == geom).expect("the used module is listed");
    assert_eq!(g.modified, before.1);
    let draw = c.files.iter().find(|f| f.is_builtin()).expect("the built-in module is listed");
    assert_eq!(draw.modified, None);
    assert_eq!(c.builtins, ["draw"]);
    assert!(c.files.iter().all(|f| !f.changed()));

    // A save after the times were read shows as a change on the next poll.
    let later = before.1.unwrap() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&geom).unwrap().set_modified(later).unwrap();
    assert!(c.files.iter().find(|f| f.path == geom).unwrap().changed());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reloadable_program_comes_without_the_generated_main() {
    let src = "struct Game {
    n: int
}
pub setup() -> Game { Game(n: 0) }
pub frame(var s: Game) -> bool {
    s.n += 1
    s.n < 3
}
";
    let dir = folder("host", &[("game.kes", src)]);
    let (c, _) = compile_for_reload(&dir.join("game.kes"), DiagStyle::Full, false).unwrap();
    assert!(c.module.reload.is_some());
    assert!(c.module.main.is_none());
    assert!(!c.module.funcs.iter().any(|f| f.name == "main"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_compile_error_returns_the_diagnostics_with_the_line() {
    let dir = folder("error", &[("game.kes", "main() {\n    print(missing)\n}\n")]);
    let Err(failure) = compile_for_reload(&dir.join("game.kes"), DiagStyle::Full, false) else {
        panic!("compiled a program with an error")
    };
    let text = failure.text;
    assert!(text.contains("missing"), "{text}");
    assert!(text.contains("game.kes:2:"), "{text}");
    assert!(text.ends_with("1 error\n"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_contract_is_proved_for_the_build_asked_for() {
    // The overflow check of `x + 1` is in a checked build, not in a `--release` one.
    let src = "@prove(no_checks)\npub f(x: int) -> int { x + 1 }\nmain() { print(f(1)) }\n";
    let dir = folder("prove", &[("game.kes", src)]);
    let path = dir.join("game.kes");
    let Err(failure) = compile_for_reload(&path, DiagStyle::Short, false) else { panic!("the contract held in a checked build") };
    let text = failure.text;
    assert!(text.contains("can't be proven `no_checks`"), "{text}");
    assert!(compile_for_reload(&path, DiagStyle::Short, true).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_file_is_an_error_that_names_it() {
    let path = std::env::temp_dir().join("kestrel-reload-no-such-file.kes");
    let Err(failure) = compile_for_reload(&path, DiagStyle::Full, false) else { panic!("compiled a missing file") };
    let text = failure.text;
    // The host watches for it to appear.
    assert_eq!(failure.files.len(), 1);
    assert_eq!((&failure.files[0].path, failure.files[0].modified), (&path, None));
    assert!(text.starts_with(&format!("kestrel: can't read {}: ", path.display())), "{text}");
}

#[test]
fn it_compiles_on_another_thread() {
    let dir = folder("thread", &[("game.kes", "main() { print(1) }\n")]);
    let path = dir.join("game.kes");
    let compiled = std::thread::spawn(move || compile_for_reload(&path, DiagStyle::Full, false)).join().unwrap();
    assert!(compiled.is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failure_lists_the_files_it_read_with_their_times() {
    // A module only the broken program uses, and one it names that doesn't exist yet: the host
    // watches both, so fixing the first or creating the second is seen.
    let dir = folder(
        "failure_files",
        &[("game.kes", "use broken\nuse absent\nmain() { print(broken.k()) }\n"), ("broken.kes", "pub k() -> int { \"three\" }\n")],
    );
    let (game, broken, absent) = (dir.join("game.kes"), dir.join("broken.kes"), dir.join("absent.kes"));
    let Err(failure) = compile_for_reload(&game, DiagStyle::Full, false) else { panic!("compiled a broken program") };
    assert_eq!(failure.files[0].path, game, "the entry file comes first");
    assert_eq!(failure.files[0].modified, time(&game));
    let find = |path: &Path| failure.files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path:?} in {:?}", failure.files));
    assert_eq!(find(&broken).modified, time(&broken));
    assert_eq!(find(&absent).modified, None);
    assert!(failure.files.iter().all(|f| !f.changed()));
    std::fs::write(&absent, "pub x() -> int { 1 }\n").unwrap();
    assert!(find(&absent).changed(), "a file that appears is a change");
    let _ = std::fs::remove_dir_all(&dir);
}
