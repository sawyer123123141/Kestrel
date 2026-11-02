//! `kestrel run --record` and `kestrel replay` (the I/O round's spec, decision 7). Every run is
//! headless (`KESTREL_HEADLESS`) or windowless, and a replay never opens a window, so no window
//! ever shows. Each test works in its own scratch folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

use common::hermetic;
use kestrel_runtime::replay::NativeId;
use kestrel_runtime::replay_file::{Entry, ReplayFile};

/// A fresh, empty folder for one test's files. Each test needs its own `name`: tests run in
/// parallel, and the folder is cleared when a test starts.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("replay_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copies the Pong demo's `.kes` files into `dir`, and gives the path of its program.
fn copy_pong(dir: &Path) -> PathBuf {
    let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../demos/pong");
    for entry in fs::read_dir(&demo).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "kes") {
            fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
    }
    dir.join("pong.kes")
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

/// How the recorded run's window is set up: `frames` headless frames, quiet or not, or no window.
#[derive(Clone, Copy)]
enum Window {
    Quiet(u32),
    Loud(u32),
    None,
}

/// `kestrel run <main> --record <file> <words>` with `window`.
fn record(main: &Path, file: &Path, window: Window, words: &[&str]) -> Output {
    let mut cmd = kestrel();
    cmd.arg("run").arg(main).arg("--record").arg(file).args(words);
    match window {
        Window::Quiet(frames) => cmd.env("KESTREL_HEADLESS", frames.to_string()).env("KESTREL_HEADLESS_QUIET", "1"),
        Window::Loud(frames) => cmd.env("KESTREL_HEADLESS", frames.to_string()),
        Window::None => &mut cmd,
    };
    cmd.output().unwrap()
}

/// `kestrel replay <main> <file>`, with `env` set on the child.
fn replay(main: &Path, file: &Path, env: &[(&str, &str)]) -> Output {
    kestrel().arg("replay").arg(main).arg(file).envs(env.iter().copied()).output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Asserts the command exited with `code`, showing its output if not.
fn exits(out: &Output, code: i32, what: &str) {
    assert_eq!(out.status.code(), Some(code), "{what}:\nstdout:\n{}\nstderr:\n{}", stdout(out), stderr(out));
}

/// The last line of the output: the replay's report.
fn report(out: &Output) -> String {
    stdout(out).lines().last().unwrap_or_default().to_string()
}

/// The number of lines in a `replay matches (N frames, M lines)` report with `frames` frames.
fn matched_lines(report: &str, frames: u64) -> usize {
    let prefix = format!("replay matches ({frames} frames, ");
    let rest = report.strip_prefix(&prefix).unwrap_or_else(|| panic!("expected {prefix:?}..., got {report:?}"));
    rest.strip_suffix(" lines)").and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("can't read the line count of {report:?}"))
}

/// Each entry of a native log: where it starts, and its native.
fn entries(log: &[u8]) -> Vec<(usize, NativeId)> {
    let mut at = 0;
    let mut found = Vec::new();
    while at < log.len() {
        let id = NativeId::from_u8(log[at]).unwrap_or_else(|| panic!("byte {} at {at} names no native", log[at]));
        found.push((at, id));
        at += 1 + id.width();
    }
    assert_eq!(at, log.len(), "the log ends inside an entry");
    found
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn recorded_headless_pong_replays_and_matches() {
    let dir = scratch("pong");
    let (main, file) = (copy_pong(&dir), dir.join("run.replay"));
    let recorded = record(&main, &file, Window::Quiet(600), &[]);
    exits(&recorded, 0, "the recorded run");
    let read = ReplayFile::read(&file).unwrap();
    assert_eq!((read.frames, read.window.as_str(), read.build.as_str()), (600, "headless-quiet", "checked"));
    assert_eq!(read.output.len(), stdout(&recorded).lines().count(), "every printed line is in the output log");

    let out = replay(&main, &file, &[]);
    exits(&out, 0, "the replay");
    assert!(matched_lines(&report(&out), 600) >= 4, "{}", stdout(&out));
    assert!(stderr(&out).is_empty(), "nothing to note: {}", stderr(&out));
    // The program's own output is shown before the report, as it was printed.
    assert!(stdout(&out).starts_with(&stdout(&recorded)), "{}", stdout(&out));
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_flipped_input_byte_differs_at_the_last_frame() {
    let dir = scratch("flip");
    let (main, file) = (copy_pong(&dir), dir.join("run.replay"));
    exits(&record(&main, &file, Window::Quiet(600), &[]), 0, "the recorded run");
    let mut read = ReplayFile::read(&file).unwrap();
    // The value byte of the first `input.held` (W) in frame 300: W was held for that frame.
    let log = entries(&read.natives);
    let frame_300 = log.iter().filter(|(_, id)| *id == NativeId::WindowFrame).nth(299).unwrap().0;
    let (held, _) = *log.iter().find(|(at, id)| *at > frame_300 && *id == NativeId::InputHeld).unwrap();
    assert_eq!(read.natives[held + 1], 0, "nothing is held in a headless run");
    read.natives[held + 1] = 1;
    read.write(&file).unwrap();

    let out = replay(&main, &file, &[]);
    exits(&out, 1, "the replay");
    // Pong prints only at its end, so the difference shows there. Which of its lines differs
    // depends on the game: one held frame may change only the rollback count.
    let text = stdout(&out);
    assert!(text.contains("replay differs at frame 600, line "), "{text}");
    assert!(text.contains("\n  recorded: ") && text.contains("\n  this run: "), "{text}");
}

/// Calls `input.mouse` each frame only when `KES_MOUSE` is set, and prints the frame.
const MOUSE: &str = r#"use window
use input
use env
wants_mouse() -> bool {
    let v = env.get("KES_MOUSE") else { return false }
    return v == "1"
}
main() {
    window.open(100, 100, "replay test")
    var n = 0
    while window.frame() {
        if wants_mouse() {
            print(input.mouse())
        }
        let d = window.delta()
        n += 1
        print("frame {n} {d}")
    }
}
"#;

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_changed_call_order_is_reported() {
    let dir = scratch("order");
    let (main, file) = (program(&dir, MOUSE), dir.join("run.replay"));
    exits(&record(&main, &file, Window::Quiet(5), &[]), 0, "the recorded run");
    let out = replay(&main, &file, &[("KES_MOUSE", "1")]);
    exits(&out, 1, "the replay");
    assert_eq!(report(&out), "replay differs at frame 1: the program called input.mouse where the recording has window.delta");
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_replay_ignores_the_window_settings() {
    // A replay opens no window, so settings that would stop `kestrel run` (a headless frame
    // count that isn't a number) or hide the window change nothing.
    let dir = scratch("window_env");
    let (main, file) = (program(&dir, MOUSE), dir.join("run.replay"));
    exits(&record(&main, &file, Window::Quiet(5), &[]), 0, "the recorded run");
    let out = replay(&main, &file, &[("KESTREL_HEADLESS", "bad"), ("KESTREL_HIDDEN", "1")]);
    exits(&out, 0, "the replay");
    assert_eq!(report(&out), "replay matches (5 frames, 5 lines)");
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_version_mismatch_is_reported_but_runs() {
    let dir = scratch("version");
    let (main, file) = (program(&dir, MOUSE), dir.join("run.replay"));
    exits(&record(&main, &file, Window::Quiet(5), &[]), 0, "the recorded run");
    let mut read = ReplayFile::read(&file).unwrap();
    read.version = "0.0.1".to_string();
    read.write(&file).unwrap();

    let out = replay(&main, &file, &[]);
    exits(&out, 0, "the replay");
    assert_eq!(report(&out), "replay matches (5 frames, 5 lines)");
    let note = stderr(&out);
    assert!(note.contains("run.replay was recorded from prog.kes by Kestrel 0.0.1 in a checked build; this is "), "{note}");
    assert!(note.contains(env!("CARGO_PKG_VERSION")), "{note}");
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_short_log_reports_the_frame() {
    let dir = scratch("short");
    let (main, file) = (program(&dir, MOUSE), dir.join("run.replay"));
    exits(&record(&main, &file, Window::Quiet(10), &[]), 0, "the recorded run");
    let mut read = ReplayFile::read(&file).unwrap();
    // Keep the first five frames: `window.frame` and `window.delta` in each.
    let log = entries(&read.natives);
    assert_eq!(log.len(), 21, "ten frames of two calls, and the last frame() that gave false");
    read.natives.truncate(log[10].0);
    read.write(&file).unwrap();

    let out = replay(&main, &file, &[]);
    exits(&out, 1, "the replay");
    assert_eq!(report(&out), "replay differs at frame 5: the program called window.frame where the recording has ended");
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_recorded_failure_recurs() {
    let src = r#"use window
main() {
    window.open(100, 100, "replay test")
    let xs = [1, 2]
    var n = 0
    while window.frame() {
        print("frame {n}")
        print(xs[n])
        n += 1
    }
}
"#;
    let dir = scratch("failure");
    let (main, file) = (program(&dir, src), dir.join("run.replay"));
    let recorded = record(&main, &file, Window::Quiet(10), &[]);
    exits(&recorded, 101, "the recorded run");
    let read = ReplayFile::read(&file).unwrap();
    assert_eq!(read.frames, 3);
    match read.output.last() {
        Some(Entry::Failure(msg)) => assert!(msg.starts_with("index 2 out of bounds for length 2 on line 8"), "{msg}"),
        other => panic!("the output log should end in the failure, not {other:?}"),
    }

    let out = replay(&main, &file, &[]);
    exits(&out, 0, "the replay");
    assert_eq!(report(&out), "replay matches (3 frames, 5 lines)");
    assert!(stderr(&out).contains("index 2 out of bounds"), "the failure is still shown: {}", stderr(&out));

    // A run that gets further than the recorded failure is a difference.
    fs::write(&main, src.replace("[1, 2]", "[1, 2, 3]")).unwrap();
    let out = replay(&main, &file, &[]);
    exits(&out, 1, "the replay of the fixed program");
    let text = stdout(&out);
    assert!(text.contains("replay differs at frame 3, line 6:\n  recorded: <failure: index 2 out of bounds"), "{text}");
    assert!(text.contains("\n  this run: 3"), "{text}");
}

#[test]
fn recorded_args_reach_the_replay() {
    let src = r#"use args
main() {
    for a in args.all() {
        print("[{a}]")
    }
}
"#;
    let dir = scratch("args");
    let (main, file) = (program(&dir, src), dir.join("run.replay"));
    let recorded = record(&main, &file, Window::None, &["a", "b"]);
    exits(&recorded, 0, "the recorded run");
    assert_eq!(stdout(&recorded), "[a]\n[b]\n");
    assert_eq!(ReplayFile::read(&file).unwrap().args, ["a", "b"]);

    let out = replay(&main, &file, &[]);
    exits(&out, 0, "the replay");
    assert_eq!(stdout(&out), "[a]\n[b]\nreplay matches (0 frames, 2 lines)\n");
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_non_quiet_headless_recording_compares_its_draw_lines() {
    let dir = scratch("loud");
    let (main, file) = (copy_pong(&dir), dir.join("run.replay"));
    let recorded = record(&main, &file, Window::Loud(10), &[]);
    exits(&recorded, 0, "the recorded run");
    assert_eq!(ReplayFile::read(&file).unwrap().window, "headless");
    let printed = stdout(&recorded).lines().count();
    assert!(printed > 100, "ten frames of Pong draw more than 100 things:\n{}", stdout(&recorded));

    let out = replay(&main, &file, &[]);
    exits(&out, 0, "the replay");
    assert_eq!(matched_lines(&report(&out), 10), printed, "the draw lines were compared too");
}

#[test]
fn a_file_that_is_not_a_replay_is_refused() {
    let dir = scratch("refused");
    let main = program(&dir, MOUSE);
    let file = dir.join("run.replay");
    fs::write(&file, b"not a replay").unwrap();
    let out = replay(&main, &file, &[]);
    exits(&out, 2, "a replay of a text file");
    assert!(stderr(&out).contains("run.replay is not a Kestrel replay file"), "{}", stderr(&out));

    let mut newer = b"KESR".to_vec();
    newer.push(3);
    fs::write(&file, newer).unwrap();
    let out = replay(&main, &file, &[]);
    exits(&out, 2, "a replay from a newer Kestrel");
    assert!(stderr(&out).contains("run.replay was written by a newer Kestrel"), "{}", stderr(&out));

    let out = replay(&main, &dir.join("missing.replay"), &[]);
    exits(&out, 2, "a missing replay");
    assert!(stderr(&out).contains("missing.replay"), "{}", stderr(&out));
}

#[test]
fn an_exit_code_is_recorded_last_and_replayed() {
    let src = "main() {\n    print(\"bye\")\n    exit(3)\n}\n";
    let dir = scratch("exit_entry");
    let (main, file) = (program(&dir, src), dir.join("run.replay"));
    let recorded = record(&main, &file, Window::None, &[]);
    exits(&recorded, 3, "the recorded run");
    let read = ReplayFile::read(&file).unwrap();
    assert_eq!(read.output, [Entry::Line(b"bye".to_vec()), Entry::Exit(3)]);
    // The file is written in the format's version 2, which has the exit entry.
    assert_eq!(fs::read(&file).unwrap()[4], 2);

    let out = replay(&main, &file, &[]);
    exits(&out, 0, "the replay");
    assert_eq!(stdout(&out), "bye\nreplay matches (0 frames, 1 lines)\n");
}
