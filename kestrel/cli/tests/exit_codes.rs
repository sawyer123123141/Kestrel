//! Exit codes (the language round's spec, decision 5): `exit(code)` ends `kestrel run`, a built
//! exe and a reloadable program's `frame` with its code; `kestrel run --record` keeps the code as
//! the replay's last entry, and `kestrel replay` compares how the two runs ended. No program here
//! opens a window. Each test works in its own scratch folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

use common::hermetic;

/// A fresh, empty folder for one test's files (each test has its own `name`).
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("exit_codes_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `src` as `prog.kes` in `dir`, and gives its path.
fn program(dir: &Path, src: &str) -> PathBuf {
    let path = dir.join("prog.kes");
    fs::write(&path, src).unwrap();
    path
}

/// A `kestrel` command with a clean environment.
fn kestrel() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(&mut cmd);
    cmd
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Asserts the command exited with `code`, showing its output if not.
fn exits(out: &Output, code: i32, what: &str) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{what}:\nstdout:\n{}\nstderr:\n{}",
        stdout(out),
        String::from_utf8_lossy(&out.stderr)
    );
}

const BYE: &str = "main() {\n    print(\"bye\")\n    exit(3)\n    print(\"never printed\")\n}\n";

#[test]
fn run_and_an_exe_end_with_the_code() {
    let dir = scratch("run_exe");
    let main = program(&dir, BYE);
    let out = kestrel().arg("run").arg(&main).output().unwrap();
    exits(&out, 3, "kestrel run");
    assert_eq!(stdout(&out), "bye\n");

    let exe = dir.join(format!("prog{}", std::env::consts::EXE_SUFFIX));
    let built = kestrel().arg("build").arg(&main).arg("-o").arg(&exe).output().unwrap();
    exits(&built, 0, "kestrel build");
    let mut cmd = Command::new(&exe);
    hermetic(&mut cmd);
    let out = cmd.output().unwrap();
    exits(&out, 3, "the built exe");
    assert_eq!(stdout(&out), "bye\n");
}

#[test]
fn a_reloadable_frame_ends_the_session_with_the_code() {
    let src = r#"struct Count {
    n: int
}

pub setup() -> Count {
    Count(n: 0)
}

pub frame(var s: Count) -> bool {
    s.n += 1
    print("frame {s.n}")
    if s.n == 3 {
        exit(4)
    }
    true
}
"#;
    let dir = scratch("frame");
    let out = kestrel().arg("run").arg(program(&dir, src)).output().unwrap();
    exits(&out, 4, "the reloadable program");
    assert_eq!(stdout(&out), "frame 1\nframe 2\nframe 3\n");
}

#[test]
fn a_recorded_exit_replays_to_a_match() {
    let dir = scratch("record");
    let (main, file) = (program(&dir, BYE), dir.join("run.replay"));
    let recorded = kestrel().arg("run").arg(&main).arg("--record").arg(&file).output().unwrap();
    exits(&recorded, 3, "the recorded run");
    let out = kestrel().arg("replay").arg(&main).arg(&file).output().unwrap();
    exits(&out, 0, "the replay");
    assert_eq!(stdout(&out), "bye\nreplay matches (0 frames, 1 lines)\n");
}

#[test]
fn a_different_exit_code_differs_at_the_end() {
    // `file.exists` isn't recorded, so the replay sees the flag file made after the recording.
    let src = "use file\nmain() {\n    print(\"start\")\n    if file.exists(\"flag.txt\") {\n        exit(1)\n    }\n}\n";
    let dir = scratch("differs");
    let (main, file) = (program(&dir, src), dir.join("run.replay"));
    let recorded = kestrel().arg("run").arg(&main).arg("--record").arg(&file).output().unwrap();
    exits(&recorded, 0, "the recorded run");
    fs::write(dir.join("flag.txt"), "set\n").unwrap();
    let out = kestrel().arg("replay").arg(&main).arg(&file).output().unwrap();
    exits(&out, 1, "the replay");
    assert_eq!(stdout(&out), "start\nreplay differs at the end: the recording exited with code 0, this run with 1\n");
}

#[test]
fn a_version_1_file_still_replays() {
    let dir = scratch("version_1");
    let (main, file) = (program(&dir, "main() {\n    print(\"one\")\n}\n"), dir.join("run.replay"));
    let recorded = kestrel().arg("run").arg(&main).arg("--record").arg(&file).output().unwrap();
    exits(&recorded, 0, "the recorded run");
    // A file with no exit entry is laid out the same in both versions.
    let mut bytes = fs::read(&file).unwrap();
    assert_eq!(bytes[4], 2);
    bytes[4] = 1;
    fs::write(&file, bytes).unwrap();
    let out = kestrel().arg("replay").arg(&main).arg(&file).output().unwrap();
    exits(&out, 0, "the replay of a version 1 file");
    assert_eq!(stdout(&out), "one\nreplay matches (0 frames, 1 lines)\n");
}
