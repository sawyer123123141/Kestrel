//! The real-window tests for what is still waiting in raylib's batch of shapes when something else
//! happens: a GPU object is freed (`gpu::drain`), or `draw.clear` clears the screen. Each draws in
//! a hidden window, takes a screenshot and checks its pixels, under both `kestrel run` (the JIT)
//! and `kestrel build` (an exe). The shared machinery is in `common/smoke.rs`.
//!
//! raylib batches shapes: `draw.rect`, a sprite and the like are only drawn on the GPU when the
//! batch is flushed (before a mesh draw, a screenshot, the end of the frame...), while a clear acts
//! at once.
//!
//! The window is hidden (`KESTREL_HIDDEN=1`), so no window appears while these run (see
//! `common/smoke.rs` for `KESTREL_SMOKE_VISIBLE`).

mod common;

use common::smoke::{smoke, Case, How, BLACK, GREEN, RED};

/// A texture is drawn before the first frame and dropped before it: `early` makes a red one, draws
/// it over the top-left quarter and returns, which drops it. The first `window.frame()` frees it,
/// and the frame then makes a green one, which the GPU may give the freed one's name. The batch
/// must have been drawn before the free, or the red sprite is drawn with the green texture.
///
/// There is no `draw.clear`, which would erase the early drawing and hide the difference; the
/// screenshot is taken in the first frame, so the early drawing is what is left of it.
const EARLY_TEXTURE: Case = Case {
    name: "early_texture",
    program: r#"use window
use draw

early() {
    let red = draw.make_texture(1, 1, [draw.RED])
    draw.texture_rect(red, vec2(0, 0), vec2(32, 32), draw.WHITE)
}

main() {
    window.open(64, 64, "early")
    early()
    var n = 0
    while window.frame() {
        n += 1
        let green = draw.make_texture(1, 1, [draw.GREEN])
        draw.texture_rect(green, vec2(32, 32), vec2(32, 32), draw.WHITE)
        window.screenshot("SHOT_PATH")
        window.close()
    }
    print("frames", n)
}
"#,
    stdout: "frames 1\n",
    size: (64, 64),
    pixels: &[
        (16, 16, RED, "the early red sprite, which must not take the new green texture"),
        (4, 4, RED, "near its top-left corner"),
        (28, 28, RED, "near its bottom-right corner"),
        (48, 48, GREEN, "the green sprite drawn in the first frame"),
    ],
    inked: &[],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_texture_dropped_before_the_first_frame_is_drawn_first_under_the_jit() {
    smoke(How::Jit, &EARLY_TEXTURE);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_texture_dropped_before_the_first_frame_is_drawn_first_in_an_exe() {
    smoke(How::Exe, &EARLY_TEXTURE);
}

/// `draw.clear` erases everything drawn before it in the frame, whichever way it was drawn. A
/// blue `draw.rects` square draws at once; a red `draw.rect` and a white circle after it are only
/// batched when the clear comes, and must be erased too. Then a green square drawn after the clear
/// shows. The screenshot is in the second frame, so the first frame's drawing is behind it.
const CLEAR: Case = Case {
    name: "clear",
    program: r#"use window
use draw

main() {
    window.open(64, 64, "clear")
    var n = 0
    while window.frame() {
        n += 1
        draw.rects([vec2(32, 0)], vec2(32, 32), draw.BLUE)
        draw.rect(vec2(0, 0), vec2(32, 32), draw.RED)
        draw.circle(vec2(16, 48), 10, draw.WHITE)
        draw.clear(draw.BLACK)
        draw.rect(vec2(32, 32), vec2(32, 32), draw.GREEN)
        if n == 2 {
            window.screenshot("SHOT_PATH")
            window.close()
        }
    }
    print("frames", n)
}
"#,
    stdout: "frames 2\n",
    size: (64, 64),
    pixels: &[
        (16, 16, BLACK, "the red rectangle, which was batched when the clear came"),
        (48, 16, BLACK, "the blue rectangles, which were drawn before the clear"),
        (16, 48, BLACK, "the white circle, which was batched when the clear came"),
        (48, 48, GREEN, "the green rectangle, drawn after the clear"),
    ],
    inked: &[],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn clear_erases_shapes_still_in_the_batch_under_the_jit() {
    smoke(How::Jit, &CLEAR);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn clear_erases_shapes_still_in_the_batch_in_an_exe() {
    smoke(How::Exe, &CLEAR);
}
