//! The real-window tests for textures, sprites, bulk rectangles and 3D shapes: draw on the GPU,
//! take a screenshot and check its pixels, under both `kestrel run` (the JIT) and `kestrel build`
//! (an exe). The shared machinery is in `common/smoke.rs`; the shapes and text are in
//! `window_smoke.rs`, and meshes in `window_smoke_mesh.rs`.
//!
//! The window is hidden (`KESTREL_HIDDEN=1`), so no window appears while these run. If this
//! machine's driver doesn't render hidden windows, set `KESTREL_SMOKE_VISIBLE=1` in the
//! environment of `cargo test`: then, when the hidden run behaved (exit 0, the expected stdout)
//! and only the screenshot's pixels differ, the program runs again with a visible window and the
//! test asserts on that run. Without it a hidden run whose pixels differ fails at once.

mod common;

use common::smoke::{smoke, Case, How, BLACK, BLUE, GREEN, RED, WHITE, YELLOW};

/// Both ways of making a texture and all three ways of drawing one, in a 64 by 64 window:
///
/// - `checker.png` (red, green, blue and white 2 by 2 cells) stretched over the top-left 32 by 32;
/// - a 2 by 1 texture of red and blue stretched over (32, 0) to (64, 16);
/// - the top row of the checker as a region, drawn with a negative width, so its right-hand
///   colour comes out on the left: green, then red, over (32, 32) to (64, 48);
/// - the whole checker as a region twice as wide as the texture, which repeats it side by side,
///   over (0, 32) to (16, 40);
/// - the checker with a negative height, so it is flipped upside down over (24, 44) to (32, 52);
/// - the checker at its natural size, plain and tinted red;
/// - a texture made in each frame, so the real frees (`UnloadTexture`) run too.
///
/// The PNG sits next to the program: the program loads it by a relative path, which also checks
/// the program-folder rule under both `kestrel run` and the built exe.
const TEXTURES: Case = Case {
    name: "textures",
    program: r#"use window
use draw
use { Rect from draw }

main() {
    window.open(64, 64, "textures")
    let checker = draw.load_texture("checker.png")
    let pair = draw.make_texture(2, 1, [draw.RED, draw.BLUE])
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw.texture_rect(checker, vec2(0, 0), vec2(32, 32), draw.WHITE)
        draw.texture_rect(pair, vec2(32, 0), vec2(32, 16), draw.WHITE)
        let top_row = Rect(pos: vec2(0, 0), size: vec2(4, 2))
        draw.texture_region(checker, top_row, vec2(64, 32), vec2(-32, 16), draw.WHITE)
        // A region twice as wide as the texture repeats it.
        let wide = Rect(pos: vec2(0, 0), size: vec2(8, 4))
        draw.texture_region(checker, wide, vec2(0, 32), vec2(16, 8), draw.WHITE)
        // A negative height covers the rectangle above `pos`, upside down.
        draw.texture_rect(checker, vec2(24, 52), vec2(8, -8), draw.WHITE)
        draw.texture(checker, vec2(4, 52), draw.WHITE)
        draw.texture(checker, vec2(20, 52), draw.RED)
        // A texture made every frame is freed at the next frame, and the last one when the window closes.
        let dot = draw.make_texture(1, 1, [draw.GREEN])
        draw.texture_rect(dot, vec2(36, 56), vec2(8, 4), draw.WHITE)
        if n == 3 {
            window.screenshot("SHOT_PATH")
            window.close()
        }
    }
    print("frames", n)
}
"#,
    stdout: "frames 3\n",
    size: (64, 64),
    pixels: &[
        (8, 8, RED, "the checker's top-left cell, stretched"),
        (24, 8, GREEN, "its top-right cell"),
        (8, 24, BLUE, "its bottom-left cell"),
        (24, 24, WHITE, "its bottom-right cell"),
        (40, 8, RED, "the made texture's left half"),
        (56, 8, BLUE, "the made texture's right half"),
        (40, 40, GREEN, "the mirrored region's left side"),
        (56, 40, RED, "the mirrored region's right side"),
        (48, 24, BLACK, "between the made texture and the mirrored region"),
        (5, 53, RED, "the natural-size checker's top-left cell"),
        (7, 53, GREEN, "its top-right cell"),
        (5, 55, BLUE, "its bottom-left cell"),
        (7, 55, WHITE, "its bottom-right cell"),
        (21, 53, RED, "the red-tinted checker's red cell"),
        (23, 53, BLACK, "its green cell, which a red tint removes"),
        (23, 55, RED, "its white cell, which a red tint turns red"),
        (12, 54, BLACK, "beside the natural-size checker"),
        (2, 34, RED, "the repeating region's first copy, top-left cell"),
        (6, 34, GREEN, "its first copy, top-right cell"),
        (10, 34, RED, "its second copy, which only a repeating texture has: top-left cell"),
        (14, 34, GREEN, "its second copy, top-right cell"),
        (10, 38, BLUE, "its second copy, bottom-left cell"),
        (14, 38, WHITE, "its second copy, bottom-right cell"),
        (26, 46, BLUE, "the upside-down checker's top-left cell, which is the bottom-left one"),
        (30, 46, WHITE, "its top-right cell, which is the bottom-right one"),
        (26, 50, RED, "its bottom-left cell, which is the top-left one"),
        (30, 50, GREEN, "its bottom-right cell, which is the top-right one"),
        (40, 58, GREEN, "the one-pixel texture made in the frame, stretched"),
    ],
    inked: &[],
    assets: &[("assets/checker.png", "checker.png")],
};

/// The same program in a folder whose name has a space and a non-ASCII letter. raylib's own file
/// functions can't open such a path on Windows, so this checks that Rust reads the texture and
/// writes the screenshot.
const TEXTURES_ACCENTED: Case = Case { name: "textures caf\u{e9}", ..TEXTURES };

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn textures_and_sprites_draw_under_the_jit() {
    smoke(How::Jit, &TEXTURES);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn textures_and_sprites_draw_in_an_exe() {
    smoke(How::Exe, &TEXTURES);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_folder_with_a_non_ascii_name_works_under_the_jit() {
    smoke(How::Jit, &TEXTURES_ACCENTED);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_folder_with_a_non_ascii_name_works_in_an_exe() {
    smoke(How::Exe, &TEXTURES_ACCENTED);
}

/// `draw.rects` and negative-size rectangles, in a 64 by 64 window on black:
///
/// - 50 red 2 by 2 dots from one `draw.rects` call, in five rows of ten, 4 pixels apart, from
///   (0, 40) to (38, 58);
/// - a green square drawn before the dots where dot 25 goes, which the dots must cover;
/// - a one-dot `draw.rects` with a negative size before them, which spans back from its position.
///   It is small, so the 50 dots that follow make the mesh grow;
/// - a blue square over dot 9 and a white square drawn after the dots, which must be on top and
///   keep their own colours (the 2D batch still works after the mesh draw);
/// - a `draw.rect` and a `draw.rect_outline` with negative sizes.
const RECTS: Case = Case {
    name: "rects",
    program: r#"use window
use draw

main() {
    window.open(64, 64, "rects")
    // Dot i is at (4 * (i % 10), 40 + 4 * (i / 10)).
    var dots = list[vec2]()
    for i from 0 to 50 {
        dots.add(vec2(float(4 * (i % 10)), float(40 + 4 * (i / 10))))
    }
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw.rect(vec2(20, 48), vec2(2, 2), draw.GREEN)
        draw.rects([vec2(50, 30)], vec2(-4, -4), draw.BLUE)
        draw.rects(dots, vec2(2, 2), draw.RED)
        draw.rect(vec2(36, 40), vec2(2, 2), draw.BLUE)
        draw.rect(vec2(48, 4), vec2(8, 8), draw.WHITE)
        // Negative sizes: the square covers (20, 20) to (30, 30), the outline (2, 2) to (14, 14).
        draw.rect(vec2(30, 30), vec2(-10, -10), draw.YELLOW)
        draw.rect_outline(vec2(14, 14), vec2(-12, -12), 2, draw.WHITE)
        if n == 3 {
            window.screenshot("SHOT_PATH")
            window.close()
        }
    }
    print("frames", n)
}
"#,
    stdout: "frames 3\n",
    size: (64, 64),
    pixels: &[
        (1, 41, RED, "inside dot 0, the first"),
        (37, 57, RED, "inside dot 49, the last"),
        (2, 41, BLACK, "just right of dot 0"),
        (1, 42, BLACK, "just below dot 0"),
        (21, 49, RED, "dot 25, which covers the green square drawn before the dots"),
        (48, 28, BLUE, "the one-dot draw.rects with size (-4, -4) at (50, 30)"),
        (50, 28, BLACK, "at its position's column, which a negative size doesn't reach"),
        (37, 41, BLUE, "the blue square drawn after the dots, over dot 9"),
        (52, 8, WHITE, "the white square drawn after the dots, not tinted by them"),
        (25, 25, YELLOW, "the square with size (-10, -10) at (30, 30)"),
        (21, 21, YELLOW, "near its top-left corner, (20, 20)"),
        (31, 31, BLACK, "past (30, 30), which a negative size doesn't reach"),
        (3, 8, WHITE, "the left border of the outline with size (-12, -12) at (14, 14)"),
        (12, 8, WHITE, "its right border"),
        (8, 8, BLACK, "inside the outline"),
    ],
    inked: &[],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn bulk_and_negative_size_rectangles_draw_under_the_jit() {
    smoke(How::Jit, &RECTS);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn bulk_and_negative_size_rectangles_draw_in_an_exe() {
    smoke(How::Exe, &RECTS);
}

/// A red cube of size 2 at the origin, a blue sphere and a green line, seen by a camera at
/// (0, 0, -5) looking at the origin, in a 64 by 64 window on black. The screenshot is taken while
/// the 3D pass is still going (it ends the pass before it captures).
///
/// **Where things land.** The camera looks along +z with y up, so the screen's right is
/// `(0, 0, 1) × (0, 1, 0) = (-1, 0, 0)`: +x is on the screen's left. The field of view is 75
/// degrees (the default), so a point at depth d (its distance along z from the camera) is
/// `32 / (d × tan 37.5°) = 41.70 / d` pixels from the centre for every world unit: a world
/// point (x, y) at z = 0 (depth 5) lands at pixel (32 - 8.34 x, 32 - 8.34 y). Pixels are checked
/// at least two pixels inside or outside an edge, because the window is multisampled.
///
/// - **The cube.** Its front face is at z = -1 (depth 4), 10.43 pixels a unit, so it covers x and y
///   from 21.6 to 42.4. (Nothing of its sides shows: the camera is level with its middle.)
/// - **The sphere**, centre (-2.4, 2.4, 0), radius 0.8. Its centre lands at (32 + 20.0, 32 - 20.0)
///   = (52.0, 12.0). A sphere off the middle of the screen is an ellipse. This one is 6.04 units
///   from the camera, 34 degrees off the axis, and its half-angle is 7.6 degrees. That makes an
///   ellipse with semi-axes of 8.2 pixels along the diagonal from the screen's middle and 6.8
///   across it, centred 0.7 pixels farther out than the sphere's centre. So it holds every point
///   within 4 pixels of (52, 12), and spans x from 45 to 60 and y from 4 to 19.
/// - **The line**, from (1.5, -2.5, 0) to (-1.5, -2.5, 0), is at depth 5, so it runs along y = 32 +
///   8.34 × 2.5 = 52.85, from x = 32 - 12.5 = 19.5 to 44.5. It is one pixel wide, and the window
///   is multisampled, so its pixels have partial coverage and blended colours: the check is that
///   the rectangle around it has ink, and that the pixels above and below it are black.
const SHAPES_3D: Case = Case {
    name: "shapes_3d",
    program: r#"use window
use draw
use draw3d
use { Camera from draw3d }

main() {
    window.open(64, 64, "shapes")
    let cam = Camera(pos: vec3(0, 0, -5), target: vec3(0, 0, 0))
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw3d.camera(cam)
        draw3d.cube(vec3(0, 0, 0), vec3(2, 2, 2), draw.RED)
        draw3d.sphere(vec3(-2.4, 2.4, 0), 0.8, draw.BLUE)
        draw3d.line(vec3(1.5, -2.5, 0), vec3(-1.5, -2.5, 0), draw.GREEN)
        if n == 3 {
            window.screenshot("SHOT_PATH")
            window.close()
        }
    }
    print("frames", n)
}
"#,
    stdout: "frames 3\n",
    size: (64, 64),
    pixels: &[
        (32, 32, RED, "the centre, which the cube covers"),
        (24, 24, RED, "near the front face's top-left corner"),
        (40, 40, RED, "near its bottom-right corner"),
        (19, 32, BLACK, "left of the cube"),
        (45, 32, BLACK, "right of the cube"),
        (32, 19, BLACK, "above the cube"),
        (32, 45, BLACK, "below the cube"),
        (0, 0, BLACK, "the top-left corner"),
        (63, 0, BLACK, "the top-right corner, which the sphere stops short of"),
        (0, 63, BLACK, "the bottom-left corner"),
        (63, 63, BLACK, "the bottom-right corner"),
        (52, 12, BLUE, "the sphere's centre"),
        (48, 12, BLUE, "4 pixels left of it, inside the sphere"),
        (56, 12, BLUE, "4 pixels right of it"),
        (52, 8, BLUE, "4 pixels above it"),
        (52, 24, BLACK, "below the sphere"),
        (40, 12, BLACK, "left of the sphere"),
        (32, 47, BLACK, "above the line, below the cube"),
        (32, 58, BLACK, "below the line"),
    ],
    inked: &[(20, 51, 24, 4, "the one-pixel line from x = 19.5 to 44.5 along y = 52.85")],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_cube_a_sphere_and_a_line_draw_under_the_jit() {
    smoke(How::Jit, &SHAPES_3D);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_cube_a_sphere_and_a_line_draw_in_an_exe() {
    smoke(How::Exe, &SHAPES_3D);
}

/// Going between 2D and 3D in one frame, in a 64 by 64 window, with the camera of `CUBE`:
///
/// - a blue 2D rectangle over the left half, then the red cube of `CUBE` and a green slab behind it
///   (drawn after it, so only the depth test keeps the red cube in front). The slab's front face
///   is 6.5 units away and covers x and y from 7 to 56;
/// - a white 2D square over the middle of the cube (28 to 35): `draw.rect` ends the 3D pass, and
///   2D is drawn on top of 3D again with the pixel coordinates it had before;
/// - a yellow cube below the middle that a `draw3d` call draws after the square: the pass is in 3D
///   again, with the same camera. Its size is negative, which is the same box as a positive one;
/// - `draw3d.camera` is called in the first frame only, so the later frames, and the screenshot in
///   the third, show that the camera stays set;
/// - after the screenshot a `draw3d.sphere` begins 3D again and the window closes in that pass.
const SWITCH: Case = Case {
    name: "switch",
    program: r#"use window
use draw
use draw3d
use { Camera from draw3d }

main() {
    window.open(64, 64, "switch")
    let cam = Camera(pos: vec3(0, 0, -5), target: vec3(0, 0, 0))
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw.rect(vec2(0, 0), vec2(32, 64), draw.BLUE)
        if n == 1 {
            draw3d.camera(cam)
        }
        draw3d.cube(vec3(0, 0, 0), vec3(2, 2, 2), draw.RED)
        draw3d.cube(vec3(0, 0, 2), vec3(8, 8, 1), draw.GREEN)
        draw.rect(vec2(28, 28), vec2(8, 8), draw.WHITE)
        draw3d.cube(vec3(0, -2.5, 0), vec3(-1, -1, -1), draw.YELLOW)
        if n == 3 {
            window.screenshot("SHOT_PATH")
            draw3d.sphere(vec3(0, 0, 0), 1, draw.WHITE)
            window.close()
        }
    }
    print("frames", n)
}
"#,
    stdout: "frames 3\n",
    size: (64, 64),
    pixels: &[
        (3, 32, BLUE, "the 2D background, left of the green slab"),
        (3, 60, BLUE, "the 2D background, lower down"),
        (60, 32, BLACK, "right of the green slab, where nothing was drawn"),
        (15, 15, GREEN, "the green slab, which is 3D drawn over the 2D background"),
        (50, 50, GREEN, "the green slab over the black"),
        (24, 24, RED, "the red cube, which is in front of the green slab although it was drawn first"),
        (40, 40, RED, "the red cube's other corner"),
        (30, 30, WHITE, "the 2D square, which is on top of the 3D cube"),
        (32, 32, WHITE, "the middle of the 2D square"),
        (25, 32, RED, "the cube just left of the square, so the square didn't take the cube's place"),
        (32, 55, YELLOW, "the yellow cube, drawn in 3D after the 2D square"),
        (32, 58, YELLOW, "the yellow cube below the green slab"),
        (32, 62, BLACK, "below the yellow cube"),
    ],
    inked: &[],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_2d_and_3d_passes_switch_under_the_jit() {
    smoke(How::Jit, &SWITCH);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_2d_and_3d_passes_switch_in_an_exe() {
    smoke(How::Exe, &SWITCH);
}
