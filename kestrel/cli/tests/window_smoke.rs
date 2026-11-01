//! The tests that open a real window: draw on the GPU, take a screenshot and check its pixels,
//! under both `kestrel run` (the JIT) and `kestrel build` (an exe). The shared machinery is in
//! `common/smoke.rs`; the textures are in `window_smoke_gpu.rs`.
//!
//! The window is hidden (`KESTREL_HIDDEN=1`) so the test doesn't flash a window at whoever runs
//! it, and a window never appears unless you ask. A hidden window might not render on some
//! machine's driver. If this machine's driver doesn't render hidden windows, set
//! `KESTREL_SMOKE_VISIBLE=1` in the environment of `cargo test`: then, when the hidden run
//! behaved (exit 0, the expected stdout) and only the screenshot's pixels differ, the program
//! runs again with a visible window and the test asserts on that run, saying on stderr which
//! mode passed. Without it a hidden run whose pixels differ fails at once. Any other failure
//! (a crash, a wrong exit code, wrong output) fails at once either way: a visible window would
//! not fix it.

mod common;

use common::smoke::{hidden_pixels_failure, smoke, visible_retry_wanted, Case, How, BLACK, BLUE, GREEN, RED, WHITE, YELLOW};

/// Three frames of a red square on black, a screenshot on the third, then `close`. After the
/// loop it prints what `frame` and `time` say, so the real backend's behaviour is pinned.
const SQUARE: Case = Case {
    name: "square",
    program: r#"use window
use draw

main() {
    window.open(64, 64, "smoke")
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw.rect(vec2(16, 16), vec2(32, 32), draw.RED)
        if n == 3 {
            window.screenshot("SHOT_PATH")
            print("time small", window.time() >= 0 and window.time() < 5)
            window.close()
        }
    }
    print("frames", n)
    print("frame after close", window.frame())
}
"#,
    stdout: "time small true\nframes 3\nframe after close false\n",
    size: (64, 64),
    pixels: &[(32, 32, RED, "the square's centre"), (2, 2, BLACK, "top left corner"), (61, 61, BLACK, "bottom right corner")],
    inked: &[],
    assets: &[],
};

/// One of each shape on black: a filled circle, a line, a triangle in each winding (raylib draws
/// only one of them as given), a circle outline and some text.
const SHAPES: Case = Case {
    name: "shapes",
    program: r#"use window
use draw

main() {
    window.open(128, 128, "shapes")
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw.circle(vec2(24, 24), 14, draw.GREEN)
        draw.line(vec2(50, 14), vec2(110, 14), 6, draw.BLUE)
        draw.triangle(vec2(10, 50), vec2(10, 80), vec2(40, 80), draw.YELLOW)
        draw.triangle(vec2(60, 50), vec2(90, 80), vec2(60, 80), draw.RED)
        draw.circle_outline(vec2(100, 70), 18, 6, draw.WHITE)
        draw.text("Hello", vec2(8, 96), 24, draw.PURPLE)
        if n == 3 {
            window.screenshot("SHOT_PATH")
            window.close()
        }
    }
    print("frames", n)
}
"#,
    stdout: "frames 3\n",
    size: (128, 128),
    pixels: &[
        (24, 24, GREEN, "the circle's centre"),
        (44, 24, BLACK, "beside the circle"),
        (80, 14, BLUE, "the line's middle"),
        (80, 26, BLACK, "below the line"),
        (20, 70, YELLOW, "inside the counter-clockwise triangle"),
        (30, 55, BLACK, "outside its slanted edge"),
        (70, 70, RED, "inside the clockwise triangle"),
        (85, 55, BLACK, "outside its slanted edge"),
        (115, 70, WHITE, "on the circle outline's ring"),
        (100, 70, BLACK, "the circle outline's hollow middle"),
        (122, 70, BLACK, "outside the ring"),
    ],
    inked: &[(8, 96, 60, 24, "the text")],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_real_window_draws_under_the_jit() {
    smoke(How::Jit, &SQUARE);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_real_window_draws_in_an_exe() {
    smoke(How::Exe, &SQUARE);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn every_shape_and_text_draw_under_the_jit() {
    smoke(How::Jit, &SHAPES);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn every_shape_and_text_draw_in_an_exe() {
    smoke(How::Exe, &SHAPES);
}

#[test]
fn a_visible_window_is_only_tried_when_asked_for() {
    assert!(!visible_retry_wanted(None));
    assert!(!visible_retry_wanted(Some("")));
    assert!(!visible_retry_wanted(Some("0")));
    assert!(!visible_retry_wanted(Some("true")));
    assert!(visible_retry_wanted(Some("1")));
}

#[test]
fn wrong_hidden_pixels_fail_at_once_and_say_how_to_try_a_visible_window() {
    let message = hidden_pixels_failure(How::Exe, "(2, 2), the square's centre: want [255, 0, 0, 255], got Some([0, 0, 0, 255])");
    assert!(message.starts_with("kestrel build with a hidden window drew the wrong pixels:"), "{message}");
    assert!(message.contains("(2, 2), the square's centre"), "{message}");
    assert!(message.contains("set KESTREL_SMOKE_VISIBLE=1"), "{message}");
}
