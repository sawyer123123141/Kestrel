//! Text built for a native call is freed after the call. A frame loop that draws
//! `"n {n}"` every frame must not grow the program arena, however long it runs.
//!
//! Memory is measured with the runtime's own report (`KESTREL_ARENA_REPORT=1`: the size
//! of the program arena's blocks when `main` returns), which is exact, unlike a
//! process's working set. The headless window runs the frames with no display, and
//! `KESTREL_HEADLESS_QUIET=1` stops it printing a line per draw call.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

use common::hermetic;

const FEW_FRAMES: u32 = 1_000;
const MANY_FRAMES: u32 = 100_000;

/// A program that opens a window and runs `setup`, then a frame loop with `body`.
fn frame_loop(setup: &str, body: &str) -> String {
    format!(
        "use window\nuse draw\n\nmain() {{\n    window.open(64, 64, \"text memory\")\n    var n = 0\n{setup}    while window.frame() {{\n        n += 1\n{body}    }}\n}}\n"
    )
}

/// Builds `src` as an executable named `name` and gives its path.
fn build(name: &str, src: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("text_memory");
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join(format!("{name}.kes"));
    fs::write(&source, src).unwrap();
    let exe = dir.join(name).with_extension(std::env::consts::EXE_EXTENSION);
    let out = hermetic(Command::new(env!("CARGO_BIN_EXE_kestrel")).arg("build").arg(&source).arg("-o").arg(&exe)).output().unwrap();
    assert!(out.status.success(), "building {name}:\n{}", String::from_utf8_lossy(&out.stderr));
    exe
}

/// Runs `exe` headless for `frames` frames and gives the program arena's size it reported.
fn arena_bytes(exe: &Path, frames: u32) -> u64 {
    let out = hermetic(&mut Command::new(exe))
        .env("KESTREL_HEADLESS", frames.to_string())
        .env("KESTREL_HEADLESS_QUIET", "1")
        .env("KESTREL_ARENA_REPORT", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{} frames: exit {:?}\n{stderr}", frames, out.status.code());
    assert!(out.stdout.is_empty(), "the quiet window should print nothing, got {} bytes", out.stdout.len());
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("kestrel: program arena "))
        .unwrap_or_else(|| panic!("no arena report in stderr:\n{stderr}"));
    line.strip_suffix(" bytes").and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("can't read the arena report {line:?}"))
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn text_built_for_a_native_does_not_grow_the_program_arena() {
    let src = frame_loop("", "        draw.text(\"n {n}\", vec2(0, 0), 10, draw.WHITE)\n");
    let exe = build("text_scratch", &src);
    let (few, many) = (arena_bytes(&exe, FEW_FRAMES), arena_bytes(&exe, MANY_FRAMES));
    assert!(
        few.abs_diff(many) < 64 * 1024,
        "the program arena grew from {few} bytes ({FEW_FRAMES} frames) to {many} bytes ({MANY_FRAMES} frames)"
    );
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_heap_constant_in_the_frame_loop_does_not_grow_the_program_arena() {
    // A constant array is built once, before `main`, and read in place. Before that, each
    // `PALETTE[n % 3]` built the whole array again in the program arena (about 4.78 MB over
    // 100,000 frames).
    let src = "use window\nuse draw\nuse { Color from draw }\n\nlet PALETTE = [draw.RED, draw.GREEN, draw.BLUE]\n\nmain() {\n    window.open(64, 64, \"constant memory\")\n    var n = 0\n    while window.frame() {\n        n += 1\n        draw.rect(vec2(0, 0), vec2(1, 1), PALETTE[n % 3])\n    }\n}\n";
    let exe = build("constant_palette", src);
    let (few, many) = (arena_bytes(&exe, FEW_FRAMES), arena_bytes(&exe, MANY_FRAMES));
    assert!(
        few.abs_diff(many) < 64 * 1024,
        "the program arena grew from {few} bytes ({FEW_FRAMES} frames) to {many} bytes ({MANY_FRAMES} frames)"
    );
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn text_kept_in_a_variable_does_grow_it() {
    // The control: a string stored in a variable stays in the program arena, so the
    // measurement can see growth. (`"n {n}"` is about 7 bytes a frame, which comes to 0.6 MiB
    // over the 100,000 frames, short of the 1 MiB the test asks for; this text is longer.)
    let src = frame_loop(
        "    var last = \"\"\n",
        "        last = \"frame {n}, kept in the program arena\"\n        draw.text(last, vec2(0, 0), 10, draw.WHITE)\n",
    );
    let exe = build("text_kept", &src);
    let (few, many) = (arena_bytes(&exe, FEW_FRAMES), arena_bytes(&exe, MANY_FRAMES));
    assert!(
        many > few + 1024 * 1024,
        "expected more than 1 MiB of growth, got {few} bytes ({FEW_FRAMES} frames) and {many} bytes ({MANY_FRAMES} frames)"
    );
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn text_built_with_plus_or_a_string_method_for_a_native_does_not_grow_the_program_arena() {
    // Every argument that builds a new string goes in the scratch arena, not only `"{...}"`:
    // a string `+`, a string method, a list's `join` and `s[i]`. Before, `+` grew the arena by
    // about 36 bytes a frame and `upper` by about 29.
    let setup = "    let name = \"player-one-with-a-long-name\"\n    let parts: list[string] = [\"left-part-of-the-text\", \"right-part-of-the-text\"]\n";
    let body = concat!(
        "        draw.text(name + \": \" + \"{n}\", vec2(0, 0), 10, draw.WHITE)\n",
        "        draw.text(name.upper(), vec2(0, 0), 10, draw.WHITE)\n",
        "        draw.text(name.slice(0, n % 20), vec2(0, 0), 10, draw.WHITE)\n",
        "        draw.text(parts.join(\", \"), vec2(0, 0), 10, draw.WHITE)\n",
        "        draw.text(name[n % 20], vec2(0, 0), 10, draw.WHITE)\n",
    );
    let exe = build("text_built_scratch", &frame_loop(setup, body));
    let (few, many) = (arena_bytes(&exe, FEW_FRAMES), arena_bytes(&exe, MANY_FRAMES));
    assert!(
        few.abs_diff(many) < 64 * 1024,
        "the program arena grew from {few} bytes ({FEW_FRAMES} frames) to {many} bytes ({MANY_FRAMES} frames)"
    );
}
