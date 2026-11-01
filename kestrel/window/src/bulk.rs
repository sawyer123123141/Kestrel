//! `draw.rects`: many rectangles of one size and colour in one GPU draw call (a negative size is
//! `abi::span`'s, as for `draw.rect`).
//!
//! raylib's shape functions feed each rectangle's vertices to its batch one call at a time, which
//! is most of what a `draw.rect` costs. Here a call's rectangles are written into one reused
//! vertex buffer and drawn as a dynamic raylib mesh, with the window's default material
//! (`material.rs`) and the 2D projection raylib set up for the window. raylib means meshes for 3D,
//! so drawing one in 2D needs care:
//!
//! - **Order.** raylib's batch still holds the shapes drawn before the call. It is drawn first
//!   (`rlDrawRenderBatchActive`), so they stay underneath. Shapes drawn after the call go into the
//!   emptied batch and come out on top.
//! - **Winding.** The 2D projection turns y upside down, so culling is turned off around the
//!   draw, and either winding shows.
//! - **Depth.** See `Z`.
//! - **Colour.** The mesh has no colours or texture coordinates. A mesh without colours gets
//!   OpenGL's current value of the colour attribute, which raylib only ever sets to white
//!   (`UploadMesh` does, for a mesh like this one); and the default material's texture is a white
//!   pixel. So the shader's result is the material's diffuse colour: the rectangles' colour, in the
//!   same 8 bits a `draw.rect` of that colour uses.
//! - **What `DrawMesh` leaves behind.** It puts raylib's modelview and projection matrices back
//!   itself, and unbinds its shader, vertex array, buffers and texture. It leaves the default
//!   shader's `colDiffuse` uniform at the rectangles' colour, but raylib's batch sets that back to
//!   white whenever it draws, along with its own shader, vertex array, texture and MVP matrix.
//!   Culling is turned back on here (raylib's 2D state). So the batch draws as before.
//!
//! The mesh is one per window. Its capacity doubles when a call needs more rectangles, up to
//! `MOST_PER_DRAW`; a bigger call draws in parts of that many, in order. It is freed when the
//! window closes (`gpu::close_all`).

use std::cell::RefCell;
use std::ffi::c_int;

use crate::abi::{span, V2};
use crate::gpu;
use crate::material;
use crate::raylib as rl;

/// The floats one rectangle takes: two triangles of three vertices, each `x, y, z`.
const FLOATS_PER_RECT: usize = 18;

/// The depth of every vertex. raylib's 2D projection (`rlOrtho(0, width, height, 0, 0, 1)`) shows
/// depths from -1 (far) to 0 (near). raylib's own 2D vertices start at -1, exactly on the far
/// plane, and rise by 1/20000 with each shape until the batch is drawn. Depth testing is off in 2D
/// (rlgl turns it off at start and only `BeginMode3D` turns it on), so the depth decides nothing
/// about what covers what, which draw order does. It only has to be inside the range: -0.5 is in
/// the middle of it, where rounding can't push it past either plane.
const Z: f32 = -0.5;

/// The fewest rectangles the mesh holds.
const FIRST_CAPACITY: usize = 16;

/// The most rectangles one draw call takes: a 72 MiB vertex buffer. It keeps the vertex count and
/// the byte size well inside a C `int`.
const MOST_PER_DRAW: usize = 1 << 20;

/// raylib's identity matrix: the mesh is drawn where its vertices say.
const IDENTITY: rl::Matrix = rl::Matrix {
    m0: 1.0,
    m4: 0.0,
    m8: 0.0,
    m12: 0.0,
    m1: 0.0,
    m5: 1.0,
    m9: 0.0,
    m13: 0.0,
    m2: 0.0,
    m6: 0.0,
    m10: 1.0,
    m14: 0.0,
    m3: 0.0,
    m7: 0.0,
    m11: 0.0,
    m15: 1.0,
};

/// Writes the vertices of the rectangle `size` spans from each of `positions` into `buf`, and
/// gives them: `FLOATS_PER_RECT` floats a rectangle. `buf` only grows, so once it has held the
/// biggest call nothing is allocated.
pub(crate) fn fill_into<'a>(buf: &'a mut Vec<f32>, positions: &[V2], size: V2) -> &'a [f32] {
    let len = positions.len() * FLOATS_PER_RECT;
    if buf.len() < len {
        buf.resize(len, 0.0);
    }
    // The rectangles have one size, so each one's corner is the same offset from its position.
    let (offset, size) = span(V2 { x: 0.0, y: 0.0 }, size);
    let out = &mut buf[..len];
    for (rect, p) in out.as_chunks_mut::<FLOATS_PER_RECT>().0.iter_mut().zip(positions) {
        let (left, top) = (p.x + offset.x, p.y + offset.y);
        let (right, bottom) = (left + size.x, top + size.y);
        // Top-left, top-right, bottom-right; then top-left, bottom-right, bottom-left.
        *rect = [left, top, Z, right, top, Z, right, bottom, Z, left, top, Z, right, bottom, Z, left, bottom, Z];
    }
    out
}

/// The capacity a mesh that holds `capacity` rectangles grows to when it needs `needed`: doubled
/// until they fit. Capacities are powers of two from `FIRST_CAPACITY`, and `needed` is at most
/// `MOST_PER_DRAW`, so the result is too.
fn grown(capacity: usize, needed: usize) -> usize {
    let mut grown = capacity.max(FIRST_CAPACITY);
    while grown < needed {
        grown *= 2;
    }
    grown
}

/// The window's mesh, and the buffer each call's vertices are written in.
#[derive(Default)]
struct Bulk {
    /// The mesh on the GPU, once made, and how many rectangles its vertex buffer holds.
    mesh: Option<(rl::Mesh, usize)>,
    vertices: Vec<f32>,
}

thread_local! {
    /// The window's thread's mesh.
    static BULK: RefCell<Bulk> = const { RefCell::new(Bulk { mesh: None, vertices: Vec::new() }) };
}

impl Bulk {
    /// The mesh, first made or grown if it holds fewer than `n` rectangles (`n` is at most
    /// `MOST_PER_DRAW`). A grown mesh replaces the old one, which is freed.
    ///
    /// # Safety
    /// A real window must be open, on this thread.
    unsafe fn mesh_for(&mut self, n: usize) -> rl::Mesh {
        let held = match self.mesh {
            Some((mesh, capacity)) if capacity >= n => return mesh,
            Some((_, capacity)) => capacity,
            None => 0,
        };
        let capacity = grown(held, n);
        // SAFETY: every field of `Mesh` is an integer or a raw pointer, for which zero (null) is valid.
        let mut mesh: rl::Mesh = unsafe { std::mem::zeroed() };
        mesh.vertex_count = (6 * capacity) as c_int;
        mesh.triangle_count = (2 * capacity) as c_int;
        // SAFETY: the window is open, on this thread. With every CPU array null, `UploadMesh` makes
        // a dynamic position buffer for `vertex_count` vertices without reading any memory. The old
        // mesh was uploaded by raylib, is unloaded once (it is replaced here), and `UnloadMesh` only
        // frees what `without_cpu_pointers` leaves: raylib's own buffer ids.
        unsafe {
            rl::UploadMesh(&mut mesh, true);
            if let Some((old, _)) = self.mesh.replace((mesh, capacity)) {
                rl::UnloadMesh(gpu::without_cpu_pointers(old));
            }
        }
        mesh
    }
}

/// Draws the rectangle `size` spans from each of `positions`, in order, in `color`: what a
/// `draw.rect` for each would draw. An empty `positions` touches nothing.
///
/// # Safety
/// Unless `positions` is empty, a real window must be open, on this thread, drawing in 2D.
pub(crate) unsafe fn draw(positions: &[V2], size: V2, color: rl::Color) {
    if positions.is_empty() {
        return;
    }
    BULK.with_borrow_mut(|bulk| {
        // SAFETY: the window is open, on this thread, in 2D (this function's contract). The mesh
        // holds at least `part.len()` rectangles, so the update stays inside its vertex buffer and
        // the draw reads only vertices just written.
        unsafe {
            let material = material::tinted(color);
            rl::rlDrawRenderBatchActive();
            rl::rlDisableBackfaceCulling();
            for part in positions.chunks(MOST_PER_DRAW) {
                let mesh = bulk.mesh_for(part.len());
                let vertices = fill_into(&mut bulk.vertices, part, size);
                // At most `MOST_PER_DRAW` rectangles, so these fit a C `int`.
                let (n, bytes) = (part.len() as c_int, std::mem::size_of_val(vertices) as c_int);
                rl::UpdateMeshBuffer(mesh, rl::MESH_BUFFER_POSITIONS, vertices.as_ptr().cast(), bytes, 0);
                rl::DrawMesh(rl::Mesh { vertex_count: 6 * n, triangle_count: 2 * n, ..mesh }, material, IDENTITY);
            }
            rl::rlEnableBackfaceCulling();
        }
    });
}

/// Frees the mesh, if this thread made one, and the vertex buffer.
///
/// # Safety
/// If there is a mesh, the window that made it must still be open (before `CloseWindow`), on this thread.
pub(crate) unsafe fn close() {
    if let Some((mesh, _)) = BULK.take().mesh {
        // SAFETY: the window is open, on this thread (this function's contract), and the mesh is
        // unloaded once: it was taken out. See `mesh_for` for `without_cpu_pointers`.
        unsafe { rl::UnloadMesh(gpu::without_cpu_pointers(mesh)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2(x: f32, y: f32) -> V2 {
        V2 { x, y }
    }

    /// `fill_into` with a buffer of its own.
    fn fill(positions: &[V2], size: V2) -> Vec<f32> {
        let mut buf = Vec::new();
        let n = fill_into(&mut buf, positions, size).len();
        buf.truncate(n);
        buf
    }

    /// The `(x, y)` of each vertex in `floats`, checking every `z` on the way.
    fn corners(floats: &[f32]) -> Vec<(f32, f32)> {
        assert_eq!(floats.len() % 3, 0);
        floats
            .as_chunks::<3>()
            .0
            .iter()
            .map(|v| {
                assert_eq!(v[2], Z, "every vertex sits at the same depth");
                (v[0], v[1])
            })
            .collect()
    }

    /// The area the triangles in `floats` cover, counting each one's area once.
    fn area(floats: &[f32]) -> f32 {
        corners(floats)
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let ((ax, ay), (bx, by), (cx, cy)) = (t[0], t[1], t[2]);
                ((bx - ax) * (cy - ay) - (by - ay) * (cx - ax)).abs() / 2.0
            })
            .sum()
    }

    /// The smallest box around the vertices in `floats`: `(left, top, right, bottom)`.
    fn bounds(floats: &[f32]) -> (f32, f32, f32, f32) {
        let points = corners(floats);
        let xs = points.iter().map(|p| p.0);
        let ys = points.iter().map(|p| p.1);
        (
            xs.clone().fold(f32::INFINITY, f32::min),
            ys.clone().fold(f32::INFINITY, f32::min),
            xs.fold(f32::NEG_INFINITY, f32::max),
            ys.fold(f32::NEG_INFINITY, f32::max),
        )
    }

    #[test]
    fn fill_gives_two_triangles_per_rectangle() {
        let floats = fill(&[v2(1.0, 2.0), v2(10.0, 20.0)], v2(3.0, 4.0));
        assert_eq!(floats.len(), 2 * FLOATS_PER_RECT, "18 floats per rectangle");
        let (first, second) = floats.split_at(FLOATS_PER_RECT);
        assert_eq!(
            corners(first),
            [(1.0, 2.0), (4.0, 2.0), (4.0, 6.0), (1.0, 2.0), (4.0, 6.0), (1.0, 6.0)],
            "top-left, top-right, bottom-right, then top-left, bottom-right, bottom-left"
        );
        assert_eq!(bounds(second), (10.0, 20.0, 13.0, 24.0));
        assert_eq!(area(second), 12.0, "the two triangles cover the 3 by 4 box once");
        assert!(fill(&[], v2(3.0, 4.0)).is_empty());
    }

    #[test]
    fn fill_covers_the_spanned_box_for_a_negative_size() {
        let floats = fill(&[v2(30.0, 30.0)], v2(-10.0, -10.0));
        assert_eq!(floats.len(), FLOATS_PER_RECT);
        assert_eq!(bounds(&floats), (20.0, 20.0, 30.0, 30.0));
        assert_eq!(area(&floats), 100.0);
        let floats = fill(&[v2(50.0, 30.0)], v2(-4.0, 6.0));
        assert_eq!(bounds(&floats), (46.0, 30.0, 50.0, 36.0), "x only");
        assert_eq!(area(&floats), 24.0);
    }

    #[test]
    fn fill_into_reuses_its_buffer_once_it_has_grown() {
        let mut buf = Vec::new();
        assert_eq!(fill_into(&mut buf, &[v2(0.0, 0.0); 100], v2(1.0, 1.0)).len(), 100 * FLOATS_PER_RECT);
        let (pointer, capacity) = (buf.as_ptr(), buf.capacity());
        let three = [v2(5.0, 6.0), v2(7.0, 8.0), v2(9.0, 10.0)];
        let filled = fill_into(&mut buf, &three, v2(1.0, 2.0)).to_vec();
        assert_eq!(filled, fill(&three, v2(1.0, 2.0)), "only the rectangles asked for, none left over");
        assert_eq!((buf.as_ptr(), buf.capacity()), (pointer, capacity), "no new allocation");
    }

    #[test]
    fn an_empty_draw_makes_and_touches_nothing() {
        // On a thread of its own, with no window: a raylib call here would crash the test.
        std::thread::spawn(|| {
            let red = rl::Color { r: 255, g: 0, b: 0, a: 255 };
            // SAFETY: `positions` is empty, which needs no window. With nothing made, `close` frees nothing.
            unsafe {
                draw(&[], v2(2.0, 2.0), red);
                assert!(BULK.with_borrow(|bulk| bulk.mesh.is_none() && bulk.vertices.is_empty()), "no mesh, no buffer");
                assert!(!material::made(), "no material");
                close();
            }
            material::close();
        })
        .join()
        .unwrap();
    }

    #[test]
    fn the_capacity_doubles_until_the_rectangles_fit() {
        assert_eq!(grown(0, 1), FIRST_CAPACITY, "a first mesh holds a few");
        assert_eq!(grown(0, 50), 64);
        assert_eq!(grown(FIRST_CAPACITY, FIRST_CAPACITY + 1), 2 * FIRST_CAPACITY);
        assert_eq!(grown(64, 1000), 1024);
        assert_eq!(grown(1024, 1000), 1024, "enough already");
        assert_eq!(grown(1024, MOST_PER_DRAW), MOST_PER_DRAW, "up to the most one draw takes");
    }
}
