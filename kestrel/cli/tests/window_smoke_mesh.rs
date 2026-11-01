//! The real-window tests for meshes: draw on the GPU, take a screenshot and check its pixels,
//! under both `kestrel run` (the JIT) and `kestrel build` (an exe). The shared machinery is in
//! `common/smoke.rs`; textures, bulk rectangles and 3D shapes are in `window_smoke_gpu.rs`.
//!
//! The window is hidden (`KESTREL_HIDDEN=1`), so no window appears while these run (see
//! `common/smoke.rs` for `KESTREL_SMOKE_VISIBLE`).
//!
//! **Where things land.** Every case's camera is at (0, 0, -3), looking at the origin with y up,
//! and the window is 64 by 64. The camera looks along +z, so the screen's right is
//! `forward × up = (0, 0, 1) × (0, 1, 0) = (-1, 0, 0)`: **+x is on the screen's left**. With the
//! default 75-degree field of view, a point at z = 0 is 3 units away, where the screen's half-height
//! is `3 × tan(37.5°) = 2.302` units, so one unit is `32 / 2.302 = 13.9` pixels: a world point
//! (x, y, 0) lands at pixel (32 - 13.9 x, 32 - 13.9 y). The square with corners at x and y = ±1
//! covers pixels 18.1 to 45.9 both ways. Pixels are checked at least two pixels inside or outside
//! an edge, because the window is multisampled.

mod common;

use common::smoke::{smoke, Case, How, BLACK, BLUE, GREEN, RED, YELLOW};

/// Kestrel's winding rule, against raylib's: a quad that is clockwise as seen from the camera is
/// drawn, and the same quad wound the other way is not.
///
/// The red quad is `quad(a, b, c, d)` with a = (-1, -1, 0), b = (1, -1, 0), c = (1, 1, 0),
/// d = (-1, 1, 0). On the screen (x to the right is -x in the world, y up) its corners are
/// (1, -1), (-1, -1), (-1, 1), (1, 1). The shoelace sum, `Σ x_i y_(i+1) - x_(i+1) y_i`, is
/// (-1 - 1) + (-1 - 1) + (-1 - 1) + (-1 - 1) = -8: negative, so clockwise (with y up), and the
/// spec's example, `quad((0,0,0), (1,0,0), (1,1,0), (0,1,0))` from a camera at z = -3, gives
/// -2 the same way. So the red quad faces the camera. The green one is a d c b: counter-clockwise,
/// so it faces away and must not show.
///
/// The red quad is drawn moved by (1.1, 0, 0), so it covers x from 0.1 to 2.1: pixels 2.8 to 30.6
/// across. The green one is moved by (-1.1, 0, 0): pixels 33.4 to 61.2. Both cover 18.1 to 45.9 down.
///
/// The mesh draws must leave raylib's culling as it was (back faces culled). A cube around the
/// camera, drawn before the meshes and again after them, checks it: seen from inside, each of its
/// faces is a back face to raylib, so it shows nothing, unless the meshes' setting reached it (the
/// one before is still in raylib's batch when the first mesh is drawn). Then the whole screen would
/// be yellow. A small cube above the quads shows that raylib's cubes still draw.
const WINDING: Case = Case {
    name: "winding",
    program: r#"use window
use draw
use draw3d
use { Camera, MeshData from draw3d }

main() {
    window.open(64, 64, "winding")
    var front = MeshData()
    front.quad(vec3(-1, -1, 0), vec3(1, -1, 0), vec3(1, 1, 0), vec3(-1, 1, 0), draw.RED)
    var back = MeshData()
    back.quad(vec3(-1, -1, 0), vec3(-1, 1, 0), vec3(1, 1, 0), vec3(1, -1, 0), draw.GREEN)
    let shown = draw3d.make_mesh(front)
    let hidden = draw3d.make_mesh(back)
    let cam = Camera(pos: vec3(0, 0, -3), target: vec3(0, 0, 0))
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw3d.camera(cam)
        draw3d.cube(vec3(0, 0, -3), vec3(1, 1, 1), draw.YELLOW)
        draw3d.mesh(shown, vec3(1.1, 0, 0), draw.WHITE)
        draw3d.mesh(hidden, vec3(-1.1, 0, 0), draw.WHITE)
        draw3d.cube(vec3(0, 0, -3), vec3(1, 1, 1), draw.YELLOW)
        draw3d.cube(vec3(0, 1.5, 0), vec3(0.4, 0.4, 0.4), draw.YELLOW)
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
        (16, 32, RED, "the middle of the clockwise quad, which faces the camera"),
        (5, 21, RED, "near its top-left corner on screen, which is its world corner (2.1, 1, 0)"),
        (28, 43, RED, "near its bottom-right corner on screen, (0.1, -1, 0)"),
        (26, 22, RED, "well inside its second triangle, a c d, which a draw that ignored the indices would miss"),
        (48, 32, BLACK, "the middle of the counter-clockwise quad, which faces away"),
        (36, 21, BLACK, "near its top-left corner"),
        (59, 43, BLACK, "near its bottom-right corner"),
        (32, 32, BLACK, "between the two"),
        (16, 15, BLACK, "above the red quad"),
        (16, 49, BLACK, "below it"),
        (32, 9, YELLOW, "the small cube above the quads, drawn after the meshes"),
        (2, 2, BLACK, "a corner, which the cube around the camera would cover if its faces weren't culled"),
        (61, 61, BLACK, "the opposite corner"),
    ],
    inked: &[],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_clockwise_quad_shows_and_a_counter_clockwise_one_does_not_under_the_jit() {
    smoke(How::Jit, &WINDING);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_clockwise_quad_shows_and_a_counter_clockwise_one_does_not_in_an_exe() {
    smoke(How::Exe, &WINDING);
}

/// `mesh_textured` with a 2 by 2 texture of red and blue over green and yellow, and a white tint.
///
/// The square of `WINDING`'s red quad, at the origin. A texture's uv (0, 0) is its top-left and
/// (1, 1) its bottom-right, so for the texture to show upright and unmirrored, each corner gets
/// the uv of where it is on the screen. With +x on the screen's left:
///
/// - a = (-1, -1, 0) is at the screen's bottom right: uv (1, 1);
/// - b = (1, -1, 0) is at the bottom left: uv (0, 1);
/// - c = (1, 1, 0) is at the top left: uv (0, 0);
/// - d = (-1, 1, 0) is at the top right: uv (1, 0).
///
/// So the screen's top-left quarter of the square is the texture's top-left pixel (red), the
/// top-right one blue, the bottom-left green and the bottom-right yellow. (The brief's 2 by 1
/// texture checks only across; a second row checks down too.) The texture is nearest-filtered,
/// so each quarter is one flat colour.
const TEXTURED: Case = Case {
    name: "textured",
    program: r#"use window
use draw
use draw3d
use { Camera, MeshData from draw3d }

main() {
    window.open(64, 64, "textured")
    var data = MeshData()
    data.quad(vec3(-1, -1, 0), vec3(1, -1, 0), vec3(1, 1, 0), vec3(-1, 1, 0), draw.WHITE)
    data.uvs.add(vec2(1, 1))
    data.uvs.add(vec2(0, 1))
    data.uvs.add(vec2(0, 0))
    data.uvs.add(vec2(1, 0))
    let square = draw3d.make_mesh(data)
    let tex = draw.make_texture(2, 2, [draw.RED, draw.BLUE, draw.GREEN, draw.YELLOW])
    let cam = Camera(pos: vec3(0, 0, -3), target: vec3(0, 0, 0))
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw3d.camera(cam)
        draw3d.mesh_textured(square, tex, vec3(0, 0, 0), draw.WHITE)
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
        (25, 25, RED, "the square's top-left quarter: uv near (0.25, 0.25), the texture's top-left pixel"),
        (39, 25, BLUE, "its top-right quarter: the top-right pixel"),
        (25, 39, GREEN, "its bottom-left quarter: the bottom-left pixel"),
        (39, 39, YELLOW, "its bottom-right quarter: the bottom-right pixel"),
        (21, 21, RED, "near the top-left corner"),
        (42, 42, YELLOW, "near the bottom-right corner"),
        (10, 32, BLACK, "left of the square"),
        (54, 32, BLACK, "right of the square"),
        (32, 10, BLACK, "above the square"),
        (32, 54, BLACK, "below the square"),
    ],
    inked: &[],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_textured_mesh_draws_its_uvs_under_the_jit() {
    smoke(How::Jit, &TEXTURED);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_textured_mesh_draws_its_uvs_in_an_exe() {
    smoke(How::Exe, &TEXTURED);
}

/// 2D under and over 3D in one frame, and again in the next: a green 2D rectangle over the left
/// half, then a 3D square, then a blue 2D rectangle over the right half. The screenshot is taken in
/// the second frame, so the first frame's switches back to 2D (and the camera, set again) are
/// behind it.
///
/// The square is a mesh with no indices and no colours: six vertices, the triangles a b c and
/// a c d of `WINDING`'s red quad, all white, and drawn with a yellow tint. So this also checks a
/// mesh drawn without indices (`glDrawArrays`) and raylib's white for a mesh without colours.
const OVERLAY: Case = Case {
    name: "overlay",
    program: r#"use window
use draw
use draw3d
use { Camera, MeshData from draw3d }

main() {
    window.open(64, 64, "overlay")
    let a = vec3(-1, -1, 0)
    let b = vec3(1, -1, 0)
    let c = vec3(1, 1, 0)
    let d = vec3(-1, 1, 0)
    var data = MeshData()
    data.vertices.add(a)
    data.vertices.add(b)
    data.vertices.add(c)
    data.vertices.add(a)
    data.vertices.add(c)
    data.vertices.add(d)
    let square = draw3d.make_mesh(data)
    let cam = Camera(pos: vec3(0, 0, -3), target: vec3(0, 0, 0))
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw.rect(vec2(0, 0), vec2(32, 64), draw.GREEN)
        draw3d.camera(cam)
        draw3d.mesh(square, vec3(0, 0, 0), draw.YELLOW)
        draw.rect(vec2(32, 0), vec2(32, 64), draw.BLUE)
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
        (25, 32, YELLOW, "the 3D square over the green 2D rectangle drawn before it"),
        (21, 21, YELLOW, "near the square's top-left corner"),
        (10, 32, GREEN, "the green rectangle beside the square"),
        (25, 10, GREEN, "the green rectangle above the square"),
        (39, 32, BLUE, "the blue 2D rectangle, on top of the square where they overlap"),
        (42, 42, BLUE, "and near the square's bottom-right corner"),
        (54, 32, BLUE, "the blue rectangle beside the square"),
    ],
    inked: &[],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn two_d_over_a_mesh_stays_on_top_under_the_jit() {
    smoke(How::Jit, &OVERLAY);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn two_d_over_a_mesh_stays_on_top_in_an_exe() {
    smoke(How::Exe, &OVERLAY);
}

/// A mesh of 70,000 vertices (17,500 quads), which `make_mesh` splits into a part of 65,536
/// vertices and one of 4,464. The first quad is `WINDING`'s red quad and the last its blue
/// twin; every quad between has all four corners at the origin, so it covers nothing. The red
/// quad is in the first part, and the blue one in the second, which starts at vertex 65,536, so
/// the blue quad's vertices 69,996 to 69,999 are 4,460 to 4,463 there, with their colours
/// gathered. Both must show where they are, in their colours.
const SPLIT: Case = Case {
    name: "split",
    program: r#"use window
use draw
use draw3d
use { Camera, MeshData from draw3d }

main() {
    window.open(64, 64, "split")
    var data = MeshData()
    data.quad(vec3(0.1, -1, 0), vec3(2.1, -1, 0), vec3(2.1, 1, 0), vec3(0.1, 1, 0), draw.RED)
    let o = vec3(0, 0, 0)
    for k from 0 to 17498 {
        data.quad(o, o, o, o, draw.GREEN)
    }
    data.quad(vec3(-2.1, -1, 0), vec3(-0.1, -1, 0), vec3(-0.1, 1, 0), vec3(-2.1, 1, 0), draw.BLUE)
    let big = draw3d.make_mesh(data)
    print(big.vertex_count, big.triangle_count)
    let cam = Camera(pos: vec3(0, 0, -3), target: vec3(0, 0, 0))
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw3d.camera(cam)
        draw3d.mesh(big, vec3(0, 0, 0), draw.WHITE)
        if n == 3 {
            window.screenshot("SHOT_PATH")
            window.close()
        }
    }
    print("frames", n)
}
"#,
    stdout: "70000 35000\nframes 3\n",
    size: (64, 64),
    pixels: &[
        (16, 32, RED, "the first quad, in the first part"),
        (5, 21, RED, "near its top-left corner"),
        (48, 32, BLUE, "the last quad, in the second part, renumbered"),
        (59, 43, BLUE, "near its bottom-right corner"),
        (32, 32, BLACK, "between them, where the empty quads are"),
        (16, 15, BLACK, "above the red quad"),
        (48, 49, BLACK, "below the blue quad"),
    ],
    inked: &[],
    assets: &[],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_mesh_split_in_two_parts_draws_both_under_the_jit() {
    smoke(How::Jit, &SPLIT);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_mesh_split_in_two_parts_draws_both_in_an_exe() {
    smoke(How::Exe, &SPLIT);
}
