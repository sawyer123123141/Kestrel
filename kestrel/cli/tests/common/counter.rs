//! The reloadable counter the hot reload tests run (`reload.rs` and `reload_errors.rs`), and how
//! they start it and follow its frame lines.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::hermetic;
use super::session::Session;

/// A fresh, empty folder for one test's files (each test has its own `name`).
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("reload_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `kestrel run <path> <flags>`, headless for `frames` frames and quiet.
pub fn kestrel_run(path: &Path, frames: u32, flags: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(&mut cmd);
    cmd.arg("run").arg(path).args(flags);
    cmd.env("KESTREL_HEADLESS", frames.to_string()).env("KESTREL_HEADLESS_QUIET", "1");
    cmd
}

/// Writes `src` as `prog.kes` in a scratch folder and runs it with `flags`.
pub fn start(name: &str, src: &str, frames: u32, flags: &[&str]) -> Session {
    let path = scratch(name).join("prog.kes");
    fs::write(&path, src).unwrap();
    Session::start(kestrel_run(&path, frames, flags), &path)
}

/// A reloadable counter: `@K@` is the constant, `@FIELDS@` more fields of the state, `@START@`
/// where the count starts and `@INIT@` the values of the extra fields.
pub const COUNTER: &str = r#"use window
use time

let K = @K@

struct Count {
    n: int@FIELDS@
}

pub setup() -> Count {
    window.open(64, 64, "reload")
    print("setup")
    Count(n: @START@@INIT@)
}

pub frame(var s: Count) -> bool {
    if not window.frame() {
        return false
    }
    s.n += 1
    print("frame {s.n} value {s.n * K}")
    time.sleep(0.005)
    true
}
"#;

/// The counter with constant `k`, extra fields `fields` set to `init`, counting from `start`.
pub fn counter_with(k: i64, fields: &str, start: i64, init: &str) -> String {
    COUNTER.replace("@K@", &k.to_string()).replace("@FIELDS@", fields).replace("@START@", &start.to_string()).replace("@INIT@", init)
}

/// The counter with constant `k`.
pub fn counter(k: i64) -> String {
    counter_with(k, "", 0, "")
}

/// `frame N value V`: N and V.
pub fn parse(line: &str) -> Option<(i64, i64)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["frame", n, "value", v, ..] => Some((n.parse().ok()?, v.parse().ok()?)),
        _ => None,
    }
}

/// Reads frame lines until `count` of them show the constant `new` (V = N * new), checking that
/// N counts on by one from `last` (the state was kept) and that the lines before show `old`.
/// Gives the last N.
pub fn follow(s: &mut Session, last: i64, old: i64, new: i64, count: usize) -> i64 {
    let (mut last, mut seen) = (last, 0);
    while seen < count {
        let line = s.next_out();
        let Some((n, v)) = parse(&line) else { panic!("not a frame line: {line:?}\n{}", s.transcript()) };
        assert_eq!(n, last + 1, "the count went on from {last} to {n}:\n{}", s.transcript());
        last = n;
        if v == n * new {
            seen += 1;
        } else {
            assert!(seen == 0 && v == n * old, "line {line:?} is neither the old code nor the new:\n{}", s.transcript());
        }
    }
    last
}

/// Reads the `setup` line and the first `frames` frame lines of a counter with constant `k`.
pub fn begin(s: &mut Session, k: i64, frames: usize) -> i64 {
    assert_eq!(s.next_out(), "setup", "{}", s.transcript());
    follow(s, 0, k, k, frames)
}
