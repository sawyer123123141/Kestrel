//! The CLI's unit tests (`main.rs`'s `tests` module).

use std::process::Command;

use super::{check_record, lib_for, output_path, parse_args, proofs_checked, without_bom, Lib};

fn args(list: &[&str]) -> Option<super::Args> {
    parse_args(list.iter().map(|s| s.to_string()).collect())
}

#[test]
fn default_output_is_the_stem_plus_exe_suffix() {
    let a = args(&["build", "dir/my.prog.kes"]).unwrap();
    let expected = format!("my.prog{}", std::env::consts::EXE_SUFFIX);
    assert_eq!(output_path(&a).unwrap().to_str().unwrap(), expected);
    let a = args(&["build", "x.kes", "-o", "out/y"]).unwrap();
    assert_eq!(output_path(&a).unwrap().to_str().unwrap(), "out/y");
}

#[test]
fn bad_command_lines_are_rejected() {
    assert!(args(&["build", "x.kes", "--bogus"]).is_none());
    assert!(args(&["build", "x.kes", "-o"]).is_none());
    assert!(args(&["frob", "x.kes"]).is_none());
    assert!(args(&["build"]).is_none());
    assert!(args(&["build", "x.kes", "extra"]).is_none());
}

#[test]
fn words_after_the_file_go_to_the_program() {
    let a = args(&["run", "g.kes", "a", "--", "--b"]).unwrap();
    assert_eq!(a.program_args, ["a", "--b"]);
    // Its own flags are taken wherever they are, until `--`.
    let a = args(&["run", "g.kes", "a", "--release", "b"]).unwrap();
    assert!(a.release);
    assert_eq!(a.program_args, ["a", "b"]);
    let a = args(&["run", "--release", "g.kes", "--", "--release", "-o", "--"]).unwrap();
    assert!(a.release);
    assert_eq!(a.program_args, ["--release", "-o", "--"]);
    let a = args(&["run", "g.kes"]).unwrap();
    assert!(a.program_args.is_empty());
    assert_eq!(a.path, "g.kes");
    // An unknown flag before `--` is still a mistake.
    assert!(args(&["run", "g.kes", "--bogus"]).is_none());
    assert!(args(&["run", "g.kes", "a", "--bogus"]).is_none());
    // A file is still needed, and only `run` passes words on.
    assert!(args(&["run", "--", "g.kes"]).is_none());
    assert!(args(&["check", "g.kes", "--", "a"]).is_none());
    assert!(args(&["build", "g.kes", "a"]).is_none());
}

#[test]
fn record_and_replay_take_their_files() {
    let a = args(&["run", "g.kes", "--record", "r.replay", "a"]).unwrap();
    assert_eq!((a.record.as_deref(), a.replay.as_deref()), (Some("r.replay"), None));
    assert_eq!(a.program_args, ["a"]);
    let a = args(&["replay", "g.kes", "r.replay", "--release"]).unwrap();
    assert_eq!((a.cmd.as_str(), a.path.as_str(), a.replay.as_deref()), ("replay", "g.kes", Some("r.replay")));
    assert!(a.release && a.record.is_none() && a.program_args.is_empty());
    // `--record` needs its file and goes with `run` only.
    assert!(args(&["run", "g.kes", "--record"]).is_none());
    assert!(args(&["build", "g.kes", "--record", "r.replay"]).is_none());
    // A replay needs exactly the program and the recording, and runs nothing else.
    assert!(args(&["replay", "g.kes"]).is_none());
    assert!(args(&["replay", "g.kes", "r.replay", "extra"]).is_none());
    assert!(args(&["replay", "g.kes", "r.replay", "--", "a"]).is_none());
    assert!(args(&["replay", "g.kes", "r.replay", "--record", "s.replay"]).is_none());
    assert!(args(&["replay", "g.kes", "r.replay", "-o", "x"]).is_none());
}

#[test]
fn no_reload_goes_with_run_only() {
    let a = args(&["run", "g.kes", "--no-reload", "a"]).unwrap();
    assert!(a.no_reload);
    assert_eq!(a.program_args, ["a"]);
    assert!(!args(&["run", "g.kes"]).unwrap().no_reload);
    assert!(args(&["check", "g.kes", "--no-reload"]).is_none());
    assert!(args(&["build", "g.kes", "--no-reload"]).is_none());
    assert!(args(&["replay", "g.kes", "r.replay", "--no-reload"]).is_none());
}

#[test]
fn a_recording_never_overwrites_the_program() {
    let a = args(&["run", "dir/g.kes", "--record", "dir/g.kes"]).unwrap();
    assert_eq!(check_record(&a), Err("`--record` would overwrite the program; give it another file name".to_string()));
    assert_eq!(check_record(&args(&["run", "dir/g.kes", "--record", "dir/g.replay"]).unwrap()), Ok(()));
    assert_eq!(check_record(&args(&["run", "dir/g.kes"]).unwrap()), Ok(()));
    // Another spelling of the same file is refused too.
    let dir = std::env::temp_dir().join(format!("kestrel-record-check-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let program = dir.join("g.kes");
    std::fs::write(&program, "main() {}\n").unwrap();
    let alias = dir.join("sub").join("..").join("g.kes");
    let a = args(&["run", program.to_str().unwrap(), "--record", alias.to_str().unwrap()]).unwrap();
    assert!(check_record(&a).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_proof_check_is_on_only_for_one() {
    assert!(proofs_checked(Some("1".as_ref())));
    assert!(!proofs_checked(Some("0".as_ref())));
    assert!(!proofs_checked(Some("".as_ref())));
    assert!(!proofs_checked(None));
}

#[test]
fn only_a_leading_byte_order_mark_is_stripped() {
    assert_eq!(without_bom("\u{feff}main() {}"), "main() {}");
    assert_eq!(without_bom("main() {}"), "main() {}");
    assert_eq!(without_bom("a\u{feff}"), "a\u{feff}");
}

#[test]
fn only_window_modules_link_the_window_library() {
    assert_eq!(lib_for(&[]), Lib::Runtime);
    assert_eq!(lib_for(&["random"]), Lib::Runtime);
    assert_eq!(lib_for(&["random", "draw"]), Lib::Window);
    assert_eq!(lib_for(&["input"]), Lib::Window);
}

const CHILD_ENV: &str = "KESTREL_PANIC_HOOK_CHILD";

/// Child half of `compiler_panic_exits_3`; does nothing unless the parent set `CHILD_ENV`.
#[test]
fn panic_hook_child() {
    if std::env::var_os(CHILD_ENV).is_some() {
        super::install_panic_hook();
        panic!("boom");
    }
}

#[test]
fn compiler_panic_exits_3() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["tests::panic_hook_child", "--exact", "--nocapture"])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("kestrel: internal compiler error: "), "{stderr}");
    assert!(stderr.contains("boom") && stderr.contains("main_test.rs"), "{stderr}");
}

const COMPILE_CHILD_ENV: &str = "KESTREL_PANIC_HOOK_COMPILE_CHILD";

/// Child half of `a_panic_while_compiling_a_save_is_not_fatal`; does nothing unless the parent set
/// `COMPILE_CHILD_ENV`.
#[test]
fn panic_hook_compile_child() {
    if std::env::var_os(COMPILE_CHILD_ENV).is_some() {
        super::install_panic_hook();
        let compile = std::thread::Builder::new().name(crate::reload::COMPILE_THREAD.into()).spawn(|| panic!("bang")).unwrap();
        assert!(compile.join().is_err());
        eprintln!("the game kept running");
    }
}

#[test]
fn a_panic_while_compiling_a_save_is_not_fatal() {
    assert!(super::survives_panic(Some(crate::reload::COMPILE_THREAD)));
    assert!(!super::survives_panic(Some("main")) && !super::survives_panic(None));
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["tests::panic_hook_compile_child", "--exact", "--nocapture"])
        .env(COMPILE_CHILD_ENV, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("kestrel: internal compiler error: ") && stderr.contains("bang"), "{stderr}");
    assert!(stderr.contains("the game kept running"), "{stderr}");
}
