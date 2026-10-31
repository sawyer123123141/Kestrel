//! The rollback Pong demo (`demos/pong`): two peers in one process over a simulated network,
//! run headless (`KESTREL_HEADLESS`, `KESTREL_HEADLESS_QUIET=1`), so no window ever opens. Player
//! 1 is a bot when no key is held and player 2 always is, so a headless run is scripted: the
//! same frames give the same game, in the JIT, a checked exe and a `--release` exe.
//!
//! A run writes `pong.replay` next to the program, so every run here is of a copy of the demo in
//! the test's own scratch folder: never in `demos/pong`, and never two runs writing one file.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

mod common;

use common::hermetic;
use common::session::Session;

/// The demo's five files. `pong.kes` is the program; the others are its modules.
const FILES: [&str; 5] = ["pong.kes", "game.kes", "net.kes", "peer.kes", "replay.kes"];

/// The demo's folder.
fn demo_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../demos/pong")
}

/// A fresh, empty folder for one test's files under the target folder's scratch space. Each test
/// needs its own `name`: tests run in parallel, and the folder is cleared when a test starts.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("pong_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copies the demo's files into `dir`, and gives the path of its program there.
fn copy_demo(dir: &Path) -> PathBuf {
    for file in FILES {
        fs::copy(demo_dir().join(file), dir.join(file)).unwrap_or_else(|e| panic!("copy {file}: {e}"));
    }
    dir.join("pong.kes")
}

/// A `kestrel` command with a clean environment.
fn kestrel() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(&mut cmd);
    cmd
}

/// Sets up a quiet headless window of `frames` frames.
fn headless(cmd: &mut Command, frames: u32) -> &mut Command {
    cmd.env("KESTREL_HEADLESS", frames.to_string()).env("KESTREL_HEADLESS_QUIET", "1")
}

/// Runs `cmd`, which must succeed, and gives its output.
fn ok(cmd: &mut Command, what: &str) -> Output {
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{what}: exit {:?}\n{}", out.status.code(), String::from_utf8_lossy(&out.stderr));
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `kestrel run` of the program at `main` (checks on), headless for `frames` frames.
fn run(main: &Path, frames: u32) -> Output {
    ok(headless(kestrel().arg("run").arg(main), frames), &format!("run {}", main.display()))
}

/// Builds the demo into `dir`, checked or `--release`, and gives the exe's path.
fn build(dir: &Path, release: bool) -> PathBuf {
    let exe = dir.join(format!("pong{}", std::env::consts::EXE_SUFFIX));
    let mut cmd = kestrel();
    cmd.arg("build").arg(demo_dir().join("pong.kes")).arg("-o").arg(&exe);
    if release {
        cmd.arg("--release");
    }
    ok(&mut cmd, if release { "build --release" } else { "build" });
    exe
}

/// The words of the output line that starts with `prefix`, after the prefix.
fn line_after<'a>(out: &'a str, prefix: &str) -> Vec<&'a str> {
    let line = out.lines().find_map(|l| l.strip_prefix(prefix)).unwrap_or_else(|| panic!("no line starting {prefix:?} in:\n{out}"));
    line.split(' ').collect()
}

/// The number at `i` of `words`.
fn number(words: &[&str], i: usize) -> i64 {
    words[i].parse().unwrap_or_else(|_| panic!("expected a number at word {i} of {words:?}"))
}

#[test]
fn pong_runs_headless_and_the_three_builds_agree() {
    let jit = stdout(&run(&copy_demo(&scratch("jit")), 600));
    let checked = stdout(&ok(headless(&mut Command::new(build(&scratch("checked"), false)), 600), "checked exe"));
    let release = stdout(&ok(headless(&mut Command::new(build(&scratch("release"), true)), 600), "release exe"));
    assert_eq!(checked, jit, "the checked exe printed something else than the JIT");
    assert_eq!(release, jit, "the release exe printed something else than the JIT");

    // `frames 600 confirmed <F> checksum <C>`, then `replay matches (<F> frames)`.
    let words = line_after(&jit, "frames 600 confirmed ");
    assert_eq!((words.len(), words[1]), (3, "checksum"), "unexpected frames line in:\n{jit}");
    let confirmed = number(&words, 0);
    number(&words, 2);
    assert!(confirmed > 500 && confirmed <= 600, "confirmed {confirmed} frames of 600:\n{jit}");
    assert!(jit.lines().any(|l| l == format!("replay matches ({confirmed} frames)")), "no matching replay line in:\n{jit}");
    assert!(!jit.contains("differ"), "the peers differ:\n{jit}");
    // The peers really compared their checksums (every 30 frames), not just never found a
    // difference: both got at least to frame 480 of 600.
    let words = line_after(&jit, "checksums compared up to frame ");
    let compared = number(&words, 0);
    assert!((480..=confirmed).contains(&compared), "checksums compared up to frame {compared}, {confirmed} confirmed:\n{jit}");
}

#[test]
fn pong_rolls_back_and_recovers_lost_packets() {
    let out = stdout(&run(&copy_demo(&scratch("rollbacks")), 600));
    let words = line_after(&out, "rollbacks ");
    assert_eq!((words.len(), words[1], words[3]), (5, "deepest", "lost"), "unexpected rollbacks line in:\n{out}");
    let (rollbacks, deepest, lost) = (number(&words, 0), number(&words, 2), number(&words, 4));
    assert!(rollbacks > 0, "no rollback happened:\n{out}");
    assert!((1..=12).contains(&deepest), "the deepest rollback was {deepest} frames:\n{out}");
    assert!(lost > 0, "no packet was lost:\n{out}");
    // Every frame was confirmed in the end except the last few, so the lost packets' inputs
    // came with later packets.
    assert!(out.contains("replay matches ("), "no replay line:\n{out}");
}

#[test]
fn pong_reports_an_injected_desync() {
    let dir = scratch("desync");
    let mut replaced = 0;
    for file in FILES {
        let src = fs::read_to_string(demo_dir().join(file)).unwrap_or_else(|e| panic!("read {file}: {e}"));
        replaced += src.matches("let DESYNC_AT = -1").count();
        fs::write(dir.join(file), src.replace("let DESYNC_AT = -1", "let DESYNC_AT = 300")).unwrap();
    }
    assert_eq!(replaced, 1, "the demo should declare `let DESYNC_AT = -1` once");
    let out = stdout(&run(&dir.join("pong.kes"), 600));
    assert!(out.lines().any(|l| l == "peers differ at frame 330"), "the desync wasn't reported at frame 330:\n{out}");
    assert!(!out.contains("replay matches"), "the run should stop at the desync:\n{out}");
}

/// The program arena's size that a headless run of `frames` frames reports.
fn arena_bytes(frames: u32) -> u64 {
    let main = copy_demo(&scratch(&format!("arena_{frames}")));
    let out = ok(headless(kestrel().arg("run").arg(main), frames).env("KESTREL_ARENA_REPORT", "1"), "run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("kestrel: program arena "))
        .unwrap_or_else(|| panic!("no arena report in stderr:\n{stderr}"));
    line.strip_suffix(" bytes").and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("can't read the arena report {line:?}"))
}

#[test]
fn pong_memory_stays_flat() {
    // Everything is a ring or lives in a frame's `arena { }`, except the replay, which keeps
    // every confirmed frame's inputs and a checksum every 60 frames: about 12,400 bytes at 3,000
    // frames (twice that with the buffers its lists left behind when they doubled), which still
    // fit in the arena's first block. So both runs report the same size (65,536 bytes today),
    // and any other growth with the run's length shows. The replay file's bytes are made once,
    // at the end, in an `arena { }` of their own, so they are not in the program arena either.
    let (short, long) = (arena_bytes(600), arena_bytes(3000));
    assert_eq!(long, short, "the program arena grew from {short} bytes (600 frames) to {long} bytes (3,000 frames)");
}

#[test]
fn pong_proves_its_contracts() {
    for release in [false, true] {
        let mut cmd = kestrel();
        cmd.arg("check");
        if release {
            cmd.arg("--release");
        }
        let out = cmd.arg(demo_dir().join("pong.kes")).output().unwrap();
        let what = if release { "check --release" } else { "check" };
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{what}: exit {:?}\n{stderr}", out.status.code());
        assert!(stdout.is_empty() && stderr.is_empty(), "{what} printed:\n{stdout}{stderr}");
    }
}

/// The frame count of the line `replay matches (<F> frames)` in `out`.
fn matched_frames(out: &str) -> i64 {
    number(&line_after(out, "replay matches ("), 0)
}

/// What Pong prints when it is given `file` as its argument: it checks that replay file instead
/// of playing. The window is headless anyway, so a mistake can't open a real one.
fn check_file(main: &Path, file: &Path) -> String {
    stdout(&ok(headless(kestrel().arg("run").arg(main).arg(file), 600), "check the replay file"))
}

/// Where a replay file's inputs start: after the 13-byte header, the `int` seed and the inputs'
/// `u32` length. Each frame's `Inputs` is then 4 bools: player 1's up and down, then player 2's.
const INPUTS_AT: usize = 13 + 4 + 4;

#[test]
fn writes_and_checks_its_replay_file() {
    let dir = scratch("file");
    let main = copy_demo(&dir);
    let out = stdout(&run(&main, 600));
    let frames = matched_frames(&out);
    assert!(out.lines().any(|l| l == format!("wrote pong.replay ({frames} frames)")), "no line saying the replay was written in:
{out}");

    // The header, the seed, each confirmed frame's inputs and a checksum every 60 frames.
    let file = dir.join("pong.replay");
    let bytes = fs::read(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    let f = frames as usize;
    assert_eq!(&bytes[..4], b"KESB", "pong.replay doesn't start with to_bytes's header");
    assert_eq!(u32::from_le_bytes(bytes[17..21].try_into().unwrap()) as usize, f, "the inputs' length");
    assert_eq!(bytes.len(), INPUTS_AT + 4 * f + 4 + 8 * (f / 60), "pong.replay's size for {f} frames");

    // Given the file, Pong checks it and plays nothing (a played run would print more lines).
    assert_eq!(check_file(&main, &file), format!("replay matches ({frames} frames)
"));
}

#[test]
fn an_injected_desync_shows_in_the_file_check() {
    // `DESYNC_AT` nudges peer 2's game, but the file holds peer 1's confirmed inputs and the
    // checksums of the states they give, which the nudge never reaches: a `DESYNC_AT` build
    // checks a clean run's file as matching. So the desync is injected into the file itself:
    // player 1's `up` in frame 300 is flipped, which moves the paddle from frame 301 on.
    let dir = scratch("file_desync");
    let main = copy_demo(&dir);
    run(&main, 600);
    let mut bytes = fs::read(dir.join("pong.replay")).unwrap();
    let at = INPUTS_AT + 4 * 300;
    assert!(bytes[at] <= 1, "byte {at} should be a bool, player 1's up in frame 300");
    bytes[at] ^= 1;
    let file = dir.join("changed.replay");
    fs::write(&file, &bytes).unwrap();
    // The file keeps a checksum every 60 frames, so the first one after the change shows it.
    assert_eq!(check_file(&main, &file), "replay differs at frame 360
");
}

#[test]
fn a_bad_file_is_refused() {
    let dir = scratch("bad_file");
    let main = copy_demo(&dir);
    let text = dir.join("notes.txt");
    fs::write(&text, "not a replay
").unwrap();
    let expected = format!("{} is not a Pong replay; pass a file that a run of Pong wrote, like pong.replay
", text.display());
    assert_eq!(check_file(&main, &text), expected);

    // A file that isn't there gives `file.error()`'s sentence, which names it.
    let missing = dir.join("missing.replay");
    let out = check_file(&main, &missing);
    assert!(out.starts_with("could not open `") && out.contains("missing.replay") && out.lines().count() == 1, "{out}");
}

#[test]
fn a_replay_that_cant_be_written_is_reported() {
    // A folder where the file should go makes `file.write` fail; the run still ends normally.
    let dir = scratch("unwritable");
    let main = copy_demo(&dir);
    fs::create_dir(dir.join("pong.replay")).unwrap();
    let out = stdout(&run(&main, 60));
    let line = out.lines().last().unwrap_or("");
    assert!(line.starts_with("could not write `pong.replay`: "), "the failed write wasn't reported last:
{out}");
    assert!(!out.contains("wrote pong.replay"), "{out}");
}

/// Pong's trace line (`TRACE` on), `trace tick <t> score <a> <b>`: the tick and the two scores.
fn trace(s: &mut Session) -> (i64, i64, i64) {
    let line = loop {
        let line = s.next_out();
        if line.starts_with("trace ") {
            break line;
        }
    };
    let w: Vec<&str> = line.split(' ').collect();
    let number = |i: usize| -> i64 { w[i].parse().unwrap_or_else(|_| panic!("not a trace line: {line:?}")) };
    assert!(w.len() == 6 && w[1] == "tick" && w[3] == "score", "not a trace line: {line:?}");
    (number(2), number(4), number(5))
}

/// The next trace line, which must go on from `last`: the tick one more, no score lower.
fn trace_after(s: &mut Session, last: (i64, i64, i64)) -> (i64, i64, i64) {
    let now = trace(s);
    assert!(now.0 == last.0 + 1 && now.1 >= last.1 && now.2 >= last.2, "{last:?} then {now:?}:\n{}", s.transcript());
    now
}

#[test]
fn an_edit_reloads_pong_and_keeps_the_game() {
    // A copy with `TRACE` on, each frame paced to 5 ms so the run lasts while the test edits it.
    let dir = scratch("reload");
    let main = copy_demo(&dir);
    let src = fs::read_to_string(&main).unwrap();
    let print = "        print(\"trace tick";
    assert!(src.contains("let TRACE = false") && src.contains(print), "pong.kes has no trace line");
    let paced = src.replacen("use window", "use time\nuse window", 1).replace("let TRACE = false", "let TRACE = true");
    fs::write(&main, paced.replace(print, &format!("        time.sleep(0.005)\n{print}"))).unwrap();
    let mut cmd = kestrel();
    headless(cmd.arg("run").arg(&main), 100_000);
    let mut s = Session::start(cmd, &main);
    let mut last = trace(&mut s);
    assert_eq!(last, (1, 0, 0), "the first frame runs network frame 0:\n{}", s.transcript());
    for _ in 0..30 {
        last = trace_after(&mut s, last);
    }
    // `MAX_STEPS` changes nothing in a headless run (its clock runs one step a frame), so the
    // peers stay equal however their frames fall around the swap.
    let src = fs::read_to_string(&main).unwrap();
    s.edit(&src.replace("let MAX_STEPS = 4", "let MAX_STEPS = 5"));
    let line = s.wait_err("reloaded pong.kes in ");
    assert!(line.ends_with(" s"), "{line}");
    // The lines printed before the swap are read at once; a second more of them came after it.
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end {
        last = trace_after(&mut s, last);
    }
    assert!(!s.errs.iter().any(|l| l.contains("state reset") || l.contains("setup()")), "{}", s.transcript());
}
