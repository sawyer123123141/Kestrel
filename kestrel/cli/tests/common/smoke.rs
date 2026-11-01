//! What the tests that open a real window share: run a program in a hidden window, take a
//! screenshot, and check its pixels, under both `kestrel run` (the JIT) and `kestrel build` (an exe).
//!
//! The window is hidden (`KESTREL_HIDDEN=1`), so nothing appears on screen. Some machine's driver
//! might not render a hidden window; for that, `KESTREL_SMOKE_VISIBLE=1` in the environment of the
//! test process lets a run whose pixels were off (and nothing else) repeat with a visible window.
//!
//! A machine without a graphics driver that has OpenGL 3.3 (a CI runner) sets
//! `KESTREL_TEST_NO_WINDOW=1` in the environment of the test process: these tests then open no
//! window at all and pass with a line saying they were skipped. Builds without the window (every
//! platform but Windows for now) ignore them.
//!
//! Every CLI test crate includes this through `common`, and most don't use it, so
//! `common/mod.rs` allows dead code.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::hermetic;

pub const RED: [u8; 4] = [255, 0, 0, 255];
pub const GREEN: [u8; 4] = [0, 255, 0, 255];
pub const BLUE: [u8; 4] = [0, 0, 255, 255];
pub const YELLOW: [u8; 4] = [255, 255, 0, 255];
pub const WHITE: [u8; 4] = [255, 255, 255, 255];
pub const BLACK: [u8; 4] = [0, 0, 0, 255];

/// A program to run in a window, what it prints, and what its screenshot must show.
pub struct Case {
    pub name: &'static str,
    /// Kestrel source; `SHOT_PATH` stands for where the screenshot goes.
    pub program: &'static str,
    pub stdout: &'static str,
    /// The screenshot's width and height.
    pub size: (i32, i32),
    /// Pixels that must be exactly this colour: where, what colour, and what is there (for the
    /// failure message).
    pub pixels: &'static [(i32, i32, [u8; 4], &'static str)],
    /// Rectangles `(x, y, width, height, what)` that must hold at least one pixel that isn't black.
    pub inked: &'static [(i32, i32, i32, i32, &'static str)],
    /// Image files the program loads by a relative path: a file in `tests/programs`, and the name
    /// it gets next to the program.
    pub assets: &'static [(&'static str, &'static str)],
}

/// Which window a run used.
#[derive(Clone, Copy)]
pub enum Window {
    Hidden,
    Visible,
}

impl Window {
    fn name(self) -> &'static str {
        match self {
            Window::Hidden => "hidden",
            Window::Visible => "visible",
        }
    }
}

/// How a run starts the program: under the JIT, or as a built exe.
#[derive(Clone, Copy)]
pub enum How {
    Jit,
    Exe,
}

impl How {
    fn name(self) -> &'static str {
        match self {
            How::Jit => "kestrel run",
            How::Exe => "kestrel build",
        }
    }

    fn dir_name(self) -> &'static str {
        match self {
            How::Jit => "jit",
            How::Exe => "exe",
        }
    }
}

/// A scratch folder for one test. Its name may have non-ASCII letters: Rust opens every file.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("window_smoke").join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The command that runs the program: `kestrel run <source>`, or the built exe. No `KESTREL_*`
/// variable of the shell reaches it (a stray `KESTREL_HEADLESS` would make the run headless,
/// and a headless run takes no screenshot).
fn command(how: How, source: &Path, exe: &Path) -> Command {
    let mut cmd = match how {
        How::Jit => {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
            cmd.arg("run").arg(source);
            cmd
        }
        How::Exe => Command::new(exe),
    };
    hermetic(&mut cmd);
    cmd
}

/// Runs the program with `window`.
fn run(how: How, window: Window, source: &Path, exe: &Path, shot: &Path) -> Output {
    let _ = fs::remove_file(shot);
    let mut cmd = command(how, source, exe);
    if matches!(window, Window::Hidden) {
        cmd.env("KESTREL_HIDDEN", "1");
    }
    cmd.output().expect("run the program")
}

/// Whether the program exited 0 and printed what it should.
fn ran_right(out: &Output, case: &Case) -> bool {
    out.status.success() && String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n") == case.stdout
}

/// Compares the screenshot with what the case expects; says what differs.
fn check_shot(shot: &Path, case: &Case) -> Result<(), String> {
    if !shot.exists() {
        // (raylib logs a warning to stdout when it can't open a file, so don't ask it for a missing one.)
        return Err("the program wrote no screenshot".to_string());
    }
    let (w, h) = case.size;
    // The last pixel is inside, one past it in either direction is outside: the exact size.
    let mut points = vec![(w - 1, h - 1), (w, h - 1), (w - 1, h)];
    points.extend(case.pixels.iter().map(|&(x, y, _, _)| (x, y)));
    let rects = || case.inked.iter().flat_map(|&(x, y, w, h, _)| (y..y + h).flat_map(move |py| (x..x + w).map(move |px| (px, py))));
    points.extend(rects());
    let Some(read) = png_pixels(shot, &points) else { return Err("the screenshot isn't a readable image".to_string()) };

    let mut wrong = Vec::new();
    if read[0].is_none() || read[1].is_some() || read[2].is_some() {
        wrong.push(format!("the screenshot isn't {w}x{h}"));
    }
    let mut next = 3;
    for &(x, y, want, what) in case.pixels {
        if read[next] != Some(want) {
            wrong.push(format!("({x}, {y}), {what}: want {want:?}, got {:?}", read[next]));
        }
        next += 1;
    }
    for &(x, y, w, h, what) in case.inked {
        let area = (w * h) as usize;
        if !read[next..next + area].iter().flatten().any(|p| p[..3] != [0, 0, 0]) {
            wrong.push(format!("({x}, {y}) {w}x{h}, {what}: only black pixels"));
        }
        next += area;
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(wrong.join("\n"))
    }
}

/// Describes a run for a failure message.
fn describe(how: How, window: Window, out: &Output) -> String {
    format!(
        "{} with a {} window: exit {:?}\n--- stdout ---\n{}--- stderr ---\n{}",
        how.name(),
        window.name(),
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

/// Whether `value`, the `KESTREL_SMOKE_VISIBLE` of the process that runs the tests, asks for a
/// visible window when a hidden one drew the wrong pixels. Only `1` does.
pub fn visible_retry_wanted(value: Option<&str>) -> bool {
    value == Some("1")
}

/// The failure for a hidden run whose pixels were off when no visible retry was asked for.
pub fn hidden_pixels_failure(how: How, why: &str) -> String {
    format!(
        "{} with a hidden window drew the wrong pixels:\n{why}\n\
         If this machine's driver doesn't render hidden windows, set KESTREL_SMOKE_VISIBLE=1 and run the test again: \
         the program then runs once more in a visible window, which will appear on screen.",
        how.name()
    )
}

/// Copies the files in `case.assets` next to the program, which is where a relative path goes:
/// the source's folder under `kestrel run`, and the exe's folder for a built program. Both are `dir`.
fn copy_assets(case: &Case, dir: &Path) {
    let programs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/programs");
    for &(file, name) in case.assets {
        let from = programs.join(file);
        fs::copy(&from, dir.join(name)).unwrap_or_else(|e| panic!("asset {file}: can't copy {}: {e}", from.display()));
    }
}

/// The screenshot's pixels at `points`, read by the window crate (only Windows builds have it).
#[cfg(windows)]
fn png_pixels(shot: &Path, points: &[(i32, i32)]) -> Option<Vec<Option<[u8; 4]>>> {
    kestrel_window::png_pixels(shot, points)
}

/// Without the window crate a screenshot can't be read, and no test here takes one.
#[cfg(not(windows))]
fn png_pixels(_: &Path, _: &[(i32, i32)]) -> Option<Vec<Option<[u8; 4]>>> {
    None
}

/// Whether `value`, the `KESTREL_TEST_NO_WINDOW` of the process that runs the tests, says this
/// machine can't open a window. Only `1` does.
pub fn no_window_wanted(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Writes the program, builds it if `how` says to, then runs it with a hidden window. If the
/// hidden run behaved and just its pixels were off, the test fails, unless the environment
/// that runs the tests has `KESTREL_SMOKE_VISIBLE=1`: then the program runs again with a visible
/// window and the test asserts on that run. With `KESTREL_TEST_NO_WINDOW=1` it does nothing.
pub fn smoke(how: How, case: &Case) {
    if no_window_wanted(std::env::var("KESTREL_TEST_NO_WINDOW").ok().as_deref()) {
        println!("window smoke test ({}): skipped, KESTREL_TEST_NO_WINDOW=1 says this machine can't open a window", case.name);
        return;
    }
    let dir = scratch(&format!("{}_{}", case.name, how.dir_name()));
    let shot = dir.join("shot.png");
    let source = dir.join("smoke.kes");
    fs::write(&source, case.program.replace("SHOT_PATH", &shot.to_str().unwrap().replace('\\', "/"))).unwrap();
    copy_assets(case, &dir);
    let exe = dir.join("smoke").with_extension(std::env::consts::EXE_EXTENSION);
    if matches!(how, How::Exe) {
        let mut build = Command::new(env!("CARGO_BIN_EXE_kestrel"));
        let built = hermetic(build.arg("build").arg(&source).arg("-o").arg(&exe)).output().unwrap();
        assert!(built.status.success(), "building the program:\n{}", String::from_utf8_lossy(&built.stderr));
    }

    let out = run(how, Window::Hidden, &source, &exe, &shot);
    assert!(ran_right(&out, case), "{}", describe(how, Window::Hidden, &out));
    let hidden = match check_shot(&shot, case) {
        Ok(()) => {
            eprintln!("window smoke test ({}): {} passed with a hidden window", case.name, how.name());
            return;
        }
        Err(why) => why,
    };
    if !visible_retry_wanted(std::env::var("KESTREL_SMOKE_VISIBLE").ok().as_deref()) {
        panic!("{}", hidden_pixels_failure(how, &hidden));
    }
    eprintln!("window smoke test ({}): the hidden window's pixels were wrong, trying a visible one:\n{hidden}", case.name);
    let out = run(how, Window::Visible, &source, &exe, &shot);
    assert!(ran_right(&out, case), "{}", describe(how, Window::Visible, &out));
    if let Err(why) = check_shot(&shot, case) {
        panic!("{} with a visible window drew the wrong pixels:\n{why}", how.name());
    }
    eprintln!("window smoke test ({}): {} passed with a visible window", case.name, how.name());
}
