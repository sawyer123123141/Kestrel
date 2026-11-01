//! Hot reload measurements (the hot reload round, 2026-10-31), not tests: each is `#[ignore]`d and
//! prints its numbers, which `bench/determinism/results.md` reports. Run them on an idle machine,
//! one at a time, with the CLI they measure (`--release` for the release `kestrel`, without it for
//! the debug one):
//!
//!   cargo test --release -p kestrel-cli --test reload_bench -- --ignored --nocapture --test-threads=1
//!
//! Every program runs headless and quiet (no window opens) from a copy in the system's temporary
//! folder, outside the synced checkout.
//! - `latency`: save to `reloaded`, twenty saves each for the counter of `reload.rs`, Pong and
//!   the voxel engine (the first five give the median; the fastest of the twenty, a save that
//!   landed just before a poll, is about the compile alone), and a one-frame `--no-reload` run of
//!   the same files (the compile, plus the process's start and one `setup` and `frame`).
//! - `poll_cost`: Pong headless with and without `--no-reload` at 1, 2,000, 60,000 and 600,000
//!   frames, five rounds.
//! - `memory`: the voxel engine's working set and private bytes after its world has streamed in,
//!   after ten same-shape reloads and after ten that change `VoxelState`'s shape.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

mod common;

use common::session::Session;
use common::{hermetic, output_within};

/// The counter of `reload.rs`, with its constant `K` as `@K@`.
const COUNTER: &str = r#"use window
use time

let K = @K@

struct Count {
    n: int
}

pub setup() -> Count {
    window.open(64, 64, "reload")
    print("setup")
    Count(n: 0)
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

/// A fresh, empty folder for one measurement's files, outside the synced checkout.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("kestrel_reload_bench").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copies the `.kes` files directly in the repository folder `from` (relative to `kestrel/`) into
/// `dir`.
fn copy_kes(from: &str, dir: &Path) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(from);
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|x| x == "kes") {
            fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
    }
}

/// Turns `TRACE` on in `file` and paces each frame by `sleep` seconds before its trace print, as
/// the demos' reload tests do, so a run lasts and says where it is.
fn trace_on(file: &Path, print: &str, sleep: &str) {
    let src = fs::read_to_string(file).unwrap();
    assert!(src.contains("let TRACE = false") && src.contains(print), "{} has no trace line", file.display());
    let paced = src.replacen("use window", "use time\nuse window", 1).replace("let TRACE = false", "let TRACE = true");
    fs::write(file, paced.replace(print, &format!("        time.sleep({sleep})\n{print}"))).unwrap();
}

/// `kestrel run <main> <flags>`, headless for `frames` frames and quiet, with freed arena memory
/// reused as in a normal run (`hermetic` turns poisoning on, which never reuses it: memory grows
/// every frame and each frame is slower).
fn kestrel_run(main: &Path, frames: u64, flags: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(&mut cmd).env("KESTREL_POISON", "0");
    cmd.arg("run").arg(main).args(flags);
    cmd.env("KESTREL_HEADLESS", frames.to_string()).env("KESTREL_HEADLESS_QUIET", "1");
    cmd
}

/// The whole process of `cmd`, which must succeed, in seconds.
fn timed(mut cmd: Command) -> f64 {
    let start = Instant::now();
    let out = output_within(&mut cmd, Duration::from_secs(300)).expect("the run took over 300 s");
    let took = start.elapsed().as_secs_f64();
    assert!(out.status.success(), "exit {:?}\n{}", out.status.code(), String::from_utf8_lossy(&out.stderr));
    took
}

fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// `median (min-max)`, scaled by `k`, with `digits` decimals.
fn summary(xs: &[f64], k: f64, digits: usize) -> String {
    let lo = xs.iter().copied().fold(f64::INFINITY, f64::min) * k;
    let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max) * k;
    format!("{:.d$} ({:.d$}-{:.d$})", median(xs) * k, lo, hi, d = digits)
}

fn list(xs: &[f64]) -> String {
    xs.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(", ")
}

/// The next trace line of a demo with `TRACE` on, split into words.
fn trace(s: &mut Session) -> Vec<String> {
    loop {
        let line = s.next_out();
        if line.starts_with("trace ") {
            return line.split(' ').map(str::to_string).collect();
        }
    }
}

/// The voxel engine's trace: the position, whether the player spawned, the columns in use.
fn voxel_trace(s: &mut Session) -> (String, bool, i64) {
    let w = trace(s);
    assert!(w.len() == 9 && w[1] == "pos" && w[7] == "columns", "not a trace line: {w:?}");
    (w[2..5].join(" "), w[6] == "true", w[8].parse().unwrap())
}

/// Reads the voxel engine's trace until the player has spawned and stood still for 10 frames and
/// the columns in use have not changed for 2 s: the world around it has streamed in.
fn voxel_settled(s: &mut Session) -> i64 {
    let (mut pos, mut still) = (String::new(), 0);
    while still < 10 {
        let (now, spawned, _) = voxel_trace(s);
        still = if spawned && now == pos { still + 1 } else { 0 };
        pos = now;
    }
    let end = Instant::now() + Duration::from_secs(120);
    let (mut columns, mut since) = (-1, Instant::now());
    loop {
        let (_, _, n) = voxel_trace(s);
        if n != columns {
            (columns, since) = (n, Instant::now());
        } else if since.elapsed() > Duration::from_secs(2) {
            return columns;
        }
        assert!(Instant::now() < end, "the voxel world never settled:\n{}", s.transcript());
    }
}

/// One program to reload: its files in a scratch folder, the file the edits go to and its two
/// versions (swapped back and forth), and what to read before the first edit.
struct Case {
    name: &'static str,
    main: PathBuf,
    edited: PathBuf,
    texts: [String; 2],
    flags: &'static [&'static str],
    ready: fn(&mut Session),
}

fn counter_case() -> Case {
    let dir = scratch("counter");
    let main = dir.join("prog.kes");
    let texts = [COUNTER.replace("@K@", "2"), COUNTER.replace("@K@", "3")];
    fs::write(&main, &texts[0]).unwrap();
    let ready = |s: &mut Session| {
        assert_eq!(s.next_out(), "setup", "{}", s.transcript());
        for _ in 0..20 {
            s.next_out();
        }
    };
    Case { name: "counter", edited: main.clone(), main, texts, flags: &[], ready }
}

fn pong_case(flags: &'static [&'static str]) -> Case {
    let dir = scratch(if flags.is_empty() { "pong" } else { "pong_release" });
    copy_kes("demos/pong", &dir);
    let main = dir.join("pong.kes");
    trace_on(&main, "        print(\"trace tick", "0.005");
    let src = fs::read_to_string(&main).unwrap();
    assert!(src.contains("let MAX_STEPS = 4"));
    let texts = [src.clone(), src.replace("let MAX_STEPS = 4", "let MAX_STEPS = 5")];
    let ready = |s: &mut Session| {
        for _ in 0..30 {
            trace(s);
        }
    };
    Case { name: "pong", edited: main.clone(), main, texts, flags, ready }
}

fn voxel_case(flags: &'static [&'static str]) -> Case {
    let dir = scratch(if flags.is_empty() { "voxel" } else { "voxel_release" });
    copy_kes("voxel", &dir);
    let main = dir.join("main.kes");
    trace_on(&main, "        print(\"trace pos", "0.002");
    let edited = dir.join("player.kes");
    let src = fs::read_to_string(&edited).unwrap();
    assert!(src.contains("let WALK = 4.3"));
    let texts = [src.clone(), src.replace("let WALK = 4.3", "let WALK = 6.0")];
    let ready = |s: &mut Session| {
        voxel_settled(s);
    };
    Case { name: "voxel", main, edited, texts, flags, ready }
}

/// `reloaded <file> in <t> s`: t.
fn host_seconds(line: &str) -> f64 {
    let w: Vec<&str> = line.split_whitespace().collect();
    assert!(w.len() == 5 && w[0] == "reloaded" && w[2] == "in" && w[4] == "s", "{line}");
    w[3].parse().unwrap()
}

/// How many saves `latency` makes of each program: the first five give the median it reports,
/// and the fastest of all of them the compile (a save that happened to land just before a poll).
const SAVES: usize = 20;

/// `SAVES` saves of `case` while it runs: the wall time from just before each save to its
/// `reloaded` line and the host's own number; then five one-frame `--no-reload` runs.
fn measure_latency(case: &Case) {
    let mut s = Session::start(kestrel_run(&case.main, 4_000_000_000, case.flags), &case.edited);
    (case.ready)(&mut s);
    let (mut wall, mut host) = (Vec::new(), Vec::new());
    for i in 0..SAVES {
        // Saves at different points of the quarter-second poll.
        std::thread::sleep(Duration::from_millis(400 + 37 * i as u64));
        let start = Instant::now();
        let edited = case.edited.clone();
        s.edit_file(&edited, &case.texts[(i + 1) % 2]);
        let line = s.wait_err("reloaded ");
        wall.push(start.elapsed().as_secs_f64());
        host.push(host_seconds(&line));
    }
    assert!(!s.errs.iter().any(|l| l.contains("state reset")), "{}", s.transcript());
    drop(s);
    let mut once = vec!["--no-reload"];
    once.extend(case.flags);
    let one: Vec<f64> = (0..5).map(|_| timed(kestrel_run(&case.main, 1, &once))).collect();
    let flags = if case.flags.is_empty() { String::new() } else { format!(" {}", case.flags.join(" ")) };
    let name = format!("{}{flags}", case.name);
    println!("bench: latency {name}: wall, first five {} s [{}]", summary(&wall[..5], 1.0, 3), list(&wall[..5]));
    println!("bench: latency {name}: host, first five {} s [{}]", summary(&host[..5], 1.0, 2), list(&host[..5]));
    println!("bench: latency {name}: wall, all {SAVES} {} s [{}]", summary(&wall, 1.0, 3), list(&wall));
    println!("bench: latency {name}: host, all {SAVES} {} s [{}]", summary(&host, 1.0, 2), list(&host));
    println!("bench: latency {name}: one-frame --no-reload run {} s [{}]", summary(&one, 1.0, 3), list(&one));
}

#[test]
#[ignore = "a measurement; run it by hand on an idle machine"]
fn latency() {
    println!("bench: cli {}", env!("CARGO_BIN_EXE_kestrel"));
    for case in [counter_case(), pong_case(&[]), pong_case(&["--release"]), voxel_case(&[]), voxel_case(&["--release"])] {
        measure_latency(&case);
    }
}

#[test]
#[ignore = "a measurement; run it by hand on an idle machine"]
fn poll_cost() {
    println!("bench: cli {}", env!("CARGO_BIN_EXE_kestrel"));
    let dir = scratch("pong_poll");
    copy_kes("demos/pong", &dir);
    let main = dir.join("pong.kes");
    let frames = [1u64, 2_000, 60_000, 600_000];
    let modes: [&[&str]; 2] = [&[], &["--no-reload"]];
    // times[f][m]: the runs of frames[f] in mode m, in seconds.
    let mut times = vec![vec![Vec::new(); 2]; frames.len()];
    for _round in 0..5 {
        for (f, &n) in frames.iter().enumerate() {
            for (m, flags) in modes.iter().enumerate() {
                times[f][m].push(timed(kestrel_run(&main, n, flags)));
            }
        }
    }
    for (f, &n) in frames.iter().enumerate() {
        for (m, name) in ["reload on", "--no-reload"].iter().enumerate() {
            println!("bench: poll {n} frames {name}: {} ms [{}]", summary(&times[f][m], 1e3, 1), list(&times[f][m]));
        }
        if n > 1 {
            let per = |m: usize| (median(&times[f][m]) - median(&times[0][m])) / (n - 1) as f64 * 1e6;
            let (on, off) = (per(0), per(1));
            println!("bench: poll {n} frames per frame: on {on:.3} us, off {off:.3} us, difference {:.3} us", on - off);
        }
    }
}

/// The working set and private bytes of the `kestrel` this test started, in MB.
fn kestrel_memory() -> (f64, f64) {
    let script = format!(
        "$p = Get-CimInstance Win32_Process | Where-Object {{ $_.ParentProcessId -eq {} -and $_.Name -eq 'kestrel.exe' }}; \
         $g = Get-Process -Id $p.ProcessId; $g.WorkingSet64; $g.PrivateMemorySize64",
        std::process::id()
    );
    let out = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let n: Vec<f64> = text.split_whitespace().map(|w| w.parse::<f64>().unwrap() / (1024.0 * 1024.0)).collect();
    assert_eq!(n.len(), 2, "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    (n[0], n[1])
}

fn show(what: &str, columns: i64) -> (f64, f64) {
    let (ws, private) = kestrel_memory();
    println!("bench: memory {what}: working set {ws:.1} MB, private {private:.1} MB, columns {columns}");
    (ws, private)
}

#[test]
#[ignore = "a measurement; run it by hand on an idle machine"]
fn memory() {
    println!("bench: cli {}", env!("CARGO_BIN_EXE_kestrel"));
    let case = voxel_case(&[]);
    let mut s = Session::start(kestrel_run(&case.main, 4_000_000_000, &[]), &case.main);
    let columns = voxel_settled(&mut s);
    let start = show("after setup", columns);
    for i in 0..10 {
        let edited = case.edited.clone();
        s.edit_file(&edited, &case.texts[(i + 1) % 2]);
        s.wait_err("reloaded main.kes in ");
        std::thread::sleep(Duration::from_millis(300));
    }
    let columns = voxel_settled(&mut s);
    let same = show("after 10 same-shape reloads", columns);
    // The checkout may have CRLF line ends; both versions are written with LF.
    let src = fs::read_to_string(&case.main).unwrap().replace("\r\n", "\n");
    let (field, init) = ("    work_ms: float\n}", "fps: 0.0, work_ms: 0.0)");
    assert!(src.contains(field) && src.contains(init), "main.kes changed: no `work_ms` to add a field after");
    let grown = src.replace(field, "    work_ms: float\n    extra: int\n}").replace(init, "fps: 0.0, work_ms: 0.0, extra: 0)");
    let mut after = Vec::new();
    for i in 0..10 {
        let main = case.main.clone();
        s.edit_file(&main, if i % 2 == 0 { &grown } else { &src });
        s.wait_err("reloaded main.kes in ");
        s.wait_err("state reset: VoxelState changed shape");
        let columns = voxel_settled(&mut s);
        after.push(show(&format!("after shape change {}", i + 1), columns));
    }
    let end = *after.last().unwrap();
    println!(
        "bench: memory per shape change: working set {:.1} MB, private {:.1} MB; same-shape reloads added {:.1} MB, {:.1} MB",
        (end.0 - same.0) / 10.0,
        (end.1 - same.1) / 10.0,
        same.0 - start.0,
        same.1 - start.1
    );
}
