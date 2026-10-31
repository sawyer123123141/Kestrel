//! Hot reload under `kestrel run` (the hot reload design, decisions 2 to 7): each test copies a
//! program into a scratch folder, runs it headless with a large frame count, edits the file while
//! it runs and reads what the host says on stderr and what the program prints on stdout. Every
//! program prints `frame N value V` once a frame, so a test can see the state carry on (N counts
//! on) and the new code run (V is N times the program's constant `K`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

use common::hermetic;
use common::session::Session;

/// A fresh, empty folder for one test's files (each test has its own `name`).
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("reload_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `kestrel run <path> <flags>`, headless for `frames` frames and quiet.
fn kestrel_run(path: &Path, frames: u32, flags: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(&mut cmd);
    cmd.arg("run").arg(path).args(flags);
    cmd.env("KESTREL_HEADLESS", frames.to_string()).env("KESTREL_HEADLESS_QUIET", "1");
    cmd
}

/// Writes `src` as `prog.kes` in a scratch folder and runs it with `flags`.
fn start(name: &str, src: &str, frames: u32, flags: &[&str]) -> Session {
    let path = scratch(name).join("prog.kes");
    fs::write(&path, src).unwrap();
    Session::start(kestrel_run(&path, frames, flags), &path)
}

/// A reloadable counter: `@K@` is the constant, `@FIELDS@` more fields of the state, `@START@`
/// where the count starts and `@INIT@` the values of the extra fields.
const COUNTER: &str = r#"use window
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
fn counter_with(k: i64, fields: &str, start: i64, init: &str) -> String {
    COUNTER.replace("@K@", &k.to_string()).replace("@FIELDS@", fields).replace("@START@", &start.to_string()).replace("@INIT@", init)
}

/// The counter with constant `k`.
fn counter(k: i64) -> String {
    counter_with(k, "", 0, "")
}

/// `frame N value V`: N and V.
fn parse(line: &str) -> Option<(i64, i64)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["frame", n, "value", v, ..] => Some((n.parse().ok()?, v.parse().ok()?)),
        _ => None,
    }
}

/// Reads frame lines until `count` of them show the constant `new` (V = N * new), checking that
/// N counts on by one from `last` (the state was kept) and that the lines before show `old`.
/// Gives the last N.
fn follow(s: &mut Session, last: i64, old: i64, new: i64, count: usize) -> i64 {
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
fn begin(s: &mut Session, k: i64, frames: usize) -> i64 {
    assert_eq!(s.next_out(), "setup", "{}", s.transcript());
    follow(s, 0, k, k, frames)
}

#[test]
fn a_constant_edit_keeps_the_counter() {
    let mut s = start("constant", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&counter(3));
    let line = s.wait_err("reloaded");
    assert!(line.starts_with("reloaded prog.kes in ") && line.ends_with(" s"), "{line}");
    follow(&mut s, last, 2, 3, 5);
}

#[test]
fn a_shape_change_resets_with_a_sentence() {
    let mut s = start("shape", &counter(2), 100_000, &[]);
    begin(&mut s, 2, 3);
    s.edit(&counter_with(2, "\n    m: int", 0, ", m: 7"));
    s.wait_err("state reset: Count changed shape; setup() ran again");
    // `reloaded` comes first, as after any swap.
    let at = |start: &str| s.errs.iter().position(|l| l.starts_with(start));
    let (reloaded, reset) = (at("reloaded prog.kes in "), at("state reset:"));
    assert!(matches!((reloaded, reset), (Some(a), Some(b)) if a < b), "{}", s.transcript());
    // `setup` ran again, and the count started over.
    while s.next_out() != "setup" {}
    follow(&mut s, 0, 2, 2, 3);
}

#[test]
fn a_setup_only_edit_keeps_state_and_hints() {
    // Review Focus 3.
    let mut s = start("setup_only", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&counter_with(2, "", 1000, ""));
    s.wait_err("reloaded prog.kes in ");
    s.wait_err("setup() changed but the state was kept; restart to apply it");
    // The count goes on: the new `setup` did not run.
    let last = follow(&mut s, last, 2, 2, 20);
    assert!(last < 1000, "{}", s.transcript());
}

/// The counter with constant `k` and a type error holding `text` on line 23.
fn broken(k: i64, text: &str) -> String {
    let bad = format!("    time.sleep(0.005)\n    let oops: int = \"{text}\"\n");
    counter(k).replace("    time.sleep(0.005)\n", &bad)
}

#[test]
fn a_compile_error_keeps_the_old_code_then_a_fix_reloads() {
    let mut s = start("error", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&broken(2, "broken"));
    s.wait_err("still running the previous code; fix the error and save again");
    assert!(s.errs.iter().any(|l| l.contains("error")), "the diagnostics are printed:\n{}", s.transcript());
    let last = follow(&mut s, last, 2, 2, 10);
    s.edit(&counter(5));
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 5, 5);
    // The error was reported once, not on every poll.
    assert_eq!(s.errs.iter().filter(|l| l.contains("still running")).count(), 1, "{}", s.transcript());
}

#[cfg(windows)]
#[test]
fn a_save_still_being_written_is_waited_for() {
    // An editor that holds the file shut while it writes: the host waits for it, and reports no
    // error that the finished file doesn't have.
    let mut s = start("held", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit_held(&counter(3), std::time::Duration::from_millis(600));
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 3, 3);
    assert!(!s.errs.iter().any(|l| l.contains("still running") || l.contains("can't read")), "{}", s.transcript());
}

#[test]
fn two_saves_give_two_reloads() {
    let mut s = start("two_saves", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&counter(3));
    s.wait_err("reloaded prog.kes in ");
    let last = follow(&mut s, last, 2, 3, 3);
    s.edit(&counter(4));
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 3, 4, 3);
}

/// The counter with constant `k` and `pub` functions enough to make a compile take a while.
fn slow_counter(k: i64) -> String {
    let mut src = counter(k);
    for i in 0..300 {
        src += &format!("\npub pad{i}(x: int) -> int {{\n    var y = x\n    for j from 0 to x {{\n        y += j * {i}\n    }}\n    y\n}}\n");
    }
    src
}

#[test]
fn a_save_during_a_compile_is_not_lost() {
    // Review Focus 1: the second save lands while the first one compiles, and must still show.
    let mut s = start("during_compile", &slow_counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&slow_counter(3));
    std::thread::sleep(std::time::Duration::from_millis(400));
    s.edit(&slow_counter(4));
    s.drain_err();
    assert!(
        !s.errs.iter().any(|l| l.starts_with("reloaded")),
        "the first compile ended before the second save, so this test shows nothing; pad `slow_counter` more:\n{}",
        s.transcript()
    );
    s.wait_err("reloaded prog.kes in ");
    // Whatever ran in between, the latest save is what runs in the end, on the same count.
    let (mut last, mut fours) = (last, 0);
    while fours < 3 {
        let line = s.next_out();
        let (n, v) = parse(&line).unwrap_or_else(|| panic!("not a frame line: {line:?}"));
        assert_eq!(n, last + 1, "{}", s.transcript());
        last = n;
        fours = if v == 4 * n { fours + 1 } else { 0 };
    }
}

#[test]
fn an_error_after_a_reload_reports_the_new_file_and_keeps_the_latest_good_code() {
    // Review Focus 2.
    let mut s = start("error_after_reload", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&counter(3));
    s.wait_err("reloaded prog.kes in ");
    let last = follow(&mut s, last, 2, 3, 3);
    s.edit(&broken(3, "after the reload"));
    s.wait_err("still running the previous code; fix the error and save again");
    let errs = s.errs.join("\n");
    assert!(errs.contains("after the reload") && errs.contains(":23:"), "the error is in the new file:\n{errs}");
    // The latest good code (K = 3) keeps running, not the first (K = 2).
    follow(&mut s, last, 3, 3, 10);
}

/// The counter written with `main`: it is never reloaded.
const MAIN: &str = r#"use window
use time

let K = 2

main() {
    window.open(64, 64, "reload")
    print("setup")
    var n = 0
    while window.frame() {
        n += 1
        print("frame {n} value {n * K}")
        time.sleep(0.02)
    }
}
"#;

/// Edits a running program to K = 3 after its first frame, then checks that it ran K = 2 to its
/// end with no reload: `frames` frames at about 20 ms each give the host many polls. Gives stderr.
fn never_reloads(mut s: Session, frames: usize) -> Vec<String> {
    begin(&mut s, 2, 1);
    let src = fs::read_to_string(&s.path).unwrap().replace("let K = 2", "let K = 3");
    s.edit(&src);
    let (status, outs, errs) = s.finish();
    assert!(status.success(), "{status:?}\n{outs:?}\n{errs:?}");
    let seen: Vec<(i64, i64)> = outs.iter().filter_map(|l| parse(l)).collect();
    assert_eq!(seen.len(), frames, "{outs:?}");
    assert!(seen.iter().all(|&(n, v)| v == 2 * n), "{outs:?}");
    assert!(!errs.iter().any(|l| l.contains("reloaded") || l.contains("state reset")), "{errs:?}");
    errs
}

/// How many frames the programs that never reload run: about 3 s at 20 ms each, many polls.
const FRAMES: u32 = 150;

#[test]
fn a_main_program_never_reloads() {
    never_reloads(start("main", MAIN, FRAMES, &[]), FRAMES as usize);
}

#[test]
fn no_reload_and_record_never_reload() {
    let slow = counter(2).replace("time.sleep(0.005)", "time.sleep(0.02)");
    never_reloads(start("no_reload", &slow, FRAMES, &["--no-reload"]), FRAMES as usize);

    let dir = scratch("record");
    let (path, file) = (dir.join("prog.kes"), dir.join("run.replay"));
    fs::write(&path, &slow).unwrap();
    let s = Session::start(kestrel_run(&path, FRAMES, &["--record", file.to_str().unwrap()]), &path);
    let errs = never_reloads(s, FRAMES as usize);
    assert!(errs.iter().any(|l| l == "--record runs the program without reloading"), "{errs:?}");
    // A replay of a reloadable program runs it without reloading too, and matches.
    fs::write(&path, &slow).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    let out = hermetic(&mut cmd).arg("replay").arg(&path).arg(&file).output().unwrap();
    let (text, err) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains(&format!("replay matches ({FRAMES} frames, {} lines)", FRAMES + 1)), "{text}\n{err}");
}

#[test]
fn a_literal_string_in_state_survives() {
    // The label points into the first module's data, which stays mapped after the swap.
    let labelled = |k: i64| {
        counter(k)
            .replace("    n: int\n", "    label: string\n    n: int\n")
            .replace("Count(n: 0)", "Count(label: \"frame\", n: 0)")
            .replace("print(\"frame {s.n}", "print(\"{s.label} {s.n}")
    };
    let mut s = start("literal", &labelled(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&labelled(3));
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 3, 5);
}

/// A state holding a texture, drawn every frame: a freed one would stop the program.
const TEXTURE: &str = r#"use window
use draw
use time
use { Texture from draw }

let K = @K@

struct Pic {
    tex: Texture?
    n: int
}

pub setup() -> Pic {
    window.open(64, 64, "reload")
    print("setup")
    Pic(tex: draw.load_texture("checker.png"), n: 0)
}

shown(t: Texture?) -> string {
    let x = t else { return "none" }
    draw.texture(x, vec2(0, 0), draw.WHITE)
    "{x.width}x{x.height}"
}

pub frame(var s: Pic) -> bool {
    if not window.frame() {
        return false
    }
    s.n += 1
    print("frame {s.n} value {s.n * K} tex {shown(s.tex)}")
    time.sleep(0.005)
    true
}
"#;

/// Runs the texture program with K = 2 in a scratch folder named `name`, with the test texture
/// beside it.
fn start_texture(name: &str) -> Session {
    let dir = scratch(name);
    let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/programs/assets/checker.png");
    fs::copy(asset, dir.join("checker.png")).unwrap();
    let path = dir.join("prog.kes");
    fs::write(&path, TEXTURE.replace("@K@", "2")).unwrap();
    Session::start(kestrel_run(&path, 100_000, &[]), &path)
}

/// Whether every frame line from `from` on drew the 4 by 4 texture.
fn drew_the_texture(s: &Session, from: usize) -> bool {
    s.outs[from..].iter().filter(|l| l.starts_with("frame ")).all(|l| l.ends_with(" tex 4x4"))
}

#[test]
fn a_gpu_object_in_state_survives() {
    let mut s = start_texture("gpu");
    let last = begin(&mut s, 2, 3);
    s.edit(&TEXTURE.replace("@K@", "3"));
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 3, 5);
    // Every frame drew the same 4 by 4 texture, before the reload and after.
    assert!(drew_the_texture(&s, 0), "{}", s.transcript());
}

#[test]
fn a_shape_change_drops_the_texture_and_loads_it_again() {
    // The old module's `reload_drop` lets go of the texture, the new `setup` loads it again, and
    // the program goes on drawing it.
    let mut s = start_texture("gpu_shape");
    begin(&mut s, 2, 3);
    let reshaped = TEXTURE.replace("@K@", "2").replace("    n: int\n}", "    n: int\n    m: int\n}").replace("n: 0)", "n: 0, m: 0)");
    s.edit(&reshaped);
    s.wait_err("state reset: Pic changed shape; setup() ran again");
    while s.next_out() != "setup" {}
    follow(&mut s, 0, 2, 2, 10);
    assert!(drew_the_texture(&s, 0), "{}", s.transcript());
}

#[test]
fn an_edit_that_removes_setup_and_frame_keeps_the_old_code() {
    let mut s = start("to_main", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&MAIN.replace("let K = 2", "let K = 3"));
    s.wait_err(
        "prog.kes has no `setup` and `frame` now, so it can't be reloaded; still running the previous code; restart it to run the new one",
    );
    follow(&mut s, last, 2, 2, 10);
}

/// The counter with its value times `helper.k()`, from the module `helper.kes` beside it.
fn with_helper() -> String {
    counter(2)
        .replace("use time\n", "use time\nuse helper\n")
        .replace("    print(\"frame {s.n} value {s.n * K}\")\n", "    let v = s.n * K * helper.k()\n    print(\"frame {s.n} value {v}\")\n")
}

#[test]
fn a_module_only_a_broken_edit_uses_is_watched() {
    let mut s = start("helper_broken", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    let helper = s.path.with_file_name("helper.kes");
    fs::write(&helper, "pub k() -> int {\n    \"three\"\n}\n").unwrap();
    s.edit(&with_helper());
    s.wait_err("still running the previous code; fix the error and save again");
    assert!(s.errs.iter().any(|l| l.contains("helper.kes")), "the error is in the module:\n{}", s.transcript());
    let last = follow(&mut s, last, 2, 2, 5);
    // Only the module is fixed: the program file is not saved again.
    s.edit_file(&helper, "pub k() -> int {\n    3\n}\n");
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 6, 5);
    assert_eq!(s.errs.iter().filter(|l| l.contains("still running")).count(), 1, "{}", s.transcript());
}

#[test]
fn a_module_written_after_the_edit_that_uses_it_reloads() {
    let mut s = start("helper_missing", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&with_helper());
    s.wait_err("still running the previous code; fix the error and save again");
    let last = follow(&mut s, last, 2, 2, 5);
    let helper = s.path.with_file_name("helper.kes");
    s.edit_file(&helper, "pub k() -> int {\n    3\n}\n");
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 6, 5);
}
