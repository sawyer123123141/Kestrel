//! The real-window test for the voxel viewer's world: its faces drawn on the GPU, seen from above,
//! under both `kestrel run` (the JIT) and `kestrel build` (an exe). The shared machinery is in
//! `common/smoke.rs`; the winding rule itself is checked in `window_smoke_mesh.rs`, and the world's
//! faces are checked in pure Kestrel in `voxels_demo.rs`. This one checks that they meet: the
//! world's top faces are wound so that, seen from above, they are drawn, in full brightness.
//!
//! The window is hidden (`KESTREL_HIDDEN=1`), so no window appears while these run (see
//! `common/smoke.rs` for `KESTREL_SMOKE_VISIBLE`).

mod common;

use common::smoke::{smoke, Case, How, BLACK};

/// The top face of block (15, 92, 15), a snow block in a flat patch of snow near the column's
/// middle, as bytes: `world.kes`'s `SNOW`, `Color(0.93, 0.95, 0.97)`, times that block's variation
/// (about 0.98, so the bytes are under `SNOW`'s 237, 242, 247). The eight blocks around it in the air
/// layer above are all air, so all four corners have full ambient occlusion (brightness 1) and
/// the face is this one colour all over. Its four neighbours' tops differ (their own variation),
/// so no other block's top gives exactly these bytes there. (A scratch program printed each top
/// block of the column with its height, the air around it and its colour's bytes.)
const SNOW_TOP: [u8; 4] = [233, 238, 243, 255];

/// One column of the world, the four chunks at x and z 0, seen from straight above: the camera is
/// at (16, 300, 16), over the column's middle, looking straight down with a narrow 12-degree field
/// of view, so every ray is close to vertical and the view is nearly a map. A camera looking down
/// lines up with the usual up (+y), which `draw3d.camera` refuses, so up is +z here: +z is up on
/// the screen and +x is left.
///
/// The column covers 32 by 32 blocks. The window's half-height is `(300 - h) * tan(6°)` blocks at
/// height h, so the 128 pixels cover 43.5 blocks at h = 93, the top of the snow patch (2.94 pixels
/// a block, the column 94 pixels across), and 38.9 at h = 115, the top of the highest block (3.29
/// pixels a block, the column 105 pixels). The window's corners show the background.
///
/// Each block has its own slight colour, so a pixel is one face's colour only if all of its
/// samples land on that face, which needs faces a few pixels wide: hence the window of 128
/// pixels and the narrow view. Block (15, 92, 15)'s top (at height 93) covers x and y 64.0 to 66.9
/// on the screen, so pixel (65, 65) lies in its middle.
///
/// The top faces are what the camera sees, so if the world's top faces were wound the wrong way
/// they would be culled and the column would show no top at all. That exact pixel is the winding
/// check: a side face would be 0.8 or 0.7 as bright, and with the top faces wound the wrong way
/// (65, 65) comes out black (a scratch copy of `world.kes` with the top face's corners in the
/// other order gave that) while the `inked` rectangle can still pass (the view then holds other
/// faces), so `inked` alone would not catch it. `inked` only shows that something is drawn in the
/// middle.
const VOXEL_COLUMN: Case = Case {
    name: "voxel_column",
    program: r#"use window
use draw
use draw3d
use { Camera, Mesh from draw3d }
use { chunk_mesh, LAYERS from world }

main() {
    window.open(128, 128, "voxels")
    var meshes = list[Mesh]()
    for cy from 0 to LAYERS {
        meshes.add(draw3d.make_mesh(chunk_mesh(0, cy, 0, 1337)))
    }
    // Straight down from high above the column's middle, with a narrow view; up is +z, since +y
    // lines up with the view.
    let cam = Camera(pos: vec3(16, 300, 16), target: vec3(16, 0, 16), up: vec3(0, 0, 1), fov: 12)
    var n = 0
    while window.frame() {
        n += 1
        draw.clear(draw.BLACK)
        draw3d.camera(cam)
        for m in meshes {
            draw3d.mesh(m, vec3(0, 0, 0), draw.WHITE)
        }
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
        (1, 1, BLACK, "beside the column"),
        (126, 126, BLACK, "beside the column"),
        (65, 65, SNOW_TOP, "inside the top face of snow block (15, 92, 15), at full brightness, so top faces are wound to face the camera"),
    ],
    inked: &[(56, 56, 16, 16, "the column seen from above")],
    assets: &[("../../demos/voxels/world.kes", "world.kes")],
};

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_voxel_worlds_top_faces_show_from_above_under_the_jit() {
    smoke(How::Jit, &VOXEL_COLUMN);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_voxel_worlds_top_faces_show_from_above_in_an_exe() {
    smoke(How::Exe, &VOXEL_COLUMN);
}
