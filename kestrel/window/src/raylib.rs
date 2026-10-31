//! Hand-written declarations for the raylib 6.0 functions Kestrel calls.
//!
//! There is no bindgen on this machine, so each type here is copied from
//! `window/raylib/raylib.h` with its exact C layout (structs travel by value).
//! Add a declaration when a native needs one.
#![allow(non_snake_case)]

use std::ffi::{c_char, c_int, c_uint, c_void};

/// `Color`: four bytes, passed in one register.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// `Vector2`: two floats.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vector2 {
    pub x: f32,
    pub y: f32,
}

/// `Vector3`: three floats.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vector3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// `Camera3D` (also `Camera`): where it is, what it looks at, which way is up, the vertical field
/// of view in degrees (for a perspective projection) and the projection.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera3D {
    pub position: Vector3,
    pub target: Vector3,
    pub up: Vector3,
    pub fovy: f32,
    pub projection: c_int,
}

/// `Rectangle`: top-left corner and size.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rectangle {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// `Image`: pixels in CPU memory. Free it with `UnloadImage`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Image {
    pub data: *mut c_void,
    pub width: c_int,
    pub height: c_int,
    pub mipmaps: c_int,
    pub format: c_int,
}

/// `Texture` (also `Texture2D`): an image in GPU memory.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    pub id: c_uint,
    pub width: c_int,
    pub height: c_int,
    pub mipmaps: c_int,
    pub format: c_int,
}

/// `Mesh`: vertex data in CPU memory and, once uploaded, the GPU buffers that hold it. Free it
/// with `UnloadMesh`, which also `free`s every CPU array.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Mesh {
    pub vertex_count: c_int,
    pub triangle_count: c_int,
    pub vertices: *mut f32,
    pub texcoords: *mut f32,
    pub texcoords2: *mut f32,
    pub normals: *mut f32,
    pub tangents: *mut f32,
    pub colors: *mut u8,
    pub indices: *mut u16,
    pub bone_count: c_int,
    pub bone_indices: *mut u8,
    pub bone_weights: *mut f32,
    pub anim_vertices: *mut f32,
    pub anim_normals: *mut f32,
    pub vao_id: c_uint,
    pub vbo_id: *mut c_uint,
}

/// `Matrix`: four by four floats, stored row by row as raylib names them (`m0, m4, m8, m12` is
/// the first row), which OpenGL reads as column-major.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub m0: f32,
    pub m4: f32,
    pub m8: f32,
    pub m12: f32,
    pub m1: f32,
    pub m5: f32,
    pub m9: f32,
    pub m13: f32,
    pub m2: f32,
    pub m6: f32,
    pub m10: f32,
    pub m14: f32,
    pub m3: f32,
    pub m7: f32,
    pub m11: f32,
    pub m15: f32,
}

/// `Shader`: a GPU program and its uniform locations (`RL_MAX_SHADER_LOCATIONS` of them).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Shader {
    pub id: c_uint,
    pub locs: *mut c_int,
}

/// `MaterialMap`: one of a material's maps: a texture, a colour and a value.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MaterialMap {
    pub texture: Texture,
    pub color: Color,
    pub value: f32,
}

/// `Material`: a shader and its maps (`MAX_MATERIAL_MAPS` of them, allocated by raylib).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub shader: Shader,
    pub maps: *mut MaterialMap,
    pub params: [f32; 4],
}

/// `Font`: a glyph atlas. It is only passed back to raylib, so the two
/// pointers (`Rectangle *recs`, `GlyphInfo *glyphs`) are never read here.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Font {
    pub base_size: c_int,
    pub glyph_count: c_int,
    pub glyph_padding: c_int,
    pub texture: Texture,
    pub recs: *mut Rectangle,
    pub glyphs: *mut c_void,
}

// `ConfigFlags` (the `FLAG_*` values).
pub const FLAG_WINDOW_RESIZABLE: c_uint = 0x0000_0004;
pub const FLAG_MSAA_4X_HINT: c_uint = 0x0000_0020;
pub const FLAG_VSYNC_HINT: c_uint = 0x0000_0040;
pub const FLAG_WINDOW_HIDDEN: c_uint = 0x0000_0080;

/// `TraceLogLevel::LOG_ERROR`.
pub const LOG_ERROR: c_int = 5;

/// `PixelFormat::PIXELFORMAT_UNCOMPRESSED_R8G8B8A8`: four bytes a pixel, red first.
pub const PIXELFORMAT_UNCOMPRESSED_R8G8B8A8: c_int = 7;

/// `TextureFilter::TEXTURE_FILTER_POINT`: no smoothing, so pixels stay crisp when scaled.
pub const TEXTURE_FILTER_POINT: c_int = 0;

/// `TextureWrap::TEXTURE_WRAP_REPEAT`: coordinates outside a texture repeat it.
pub const TEXTURE_WRAP_REPEAT: c_int = 0;

/// `CameraProjection::CAMERA_PERSPECTIVE`.
pub const CAMERA_PERSPECTIVE: c_int = 0;

/// `KeyboardKey::KEY_NULL`: no key.
pub const KEY_NULL: c_int = 0;

/// `MaterialMapIndex::MATERIAL_MAP_DIFFUSE` (`MATERIAL_MAP_ALBEDO`), as an index into `Material::maps`.
pub const MATERIAL_MAP_DIFFUSE: usize = 0;

/// `RL_DEFAULT_SHADER_ATTRIB_LOCATION_POSITION` (rlgl.h): the vertex buffer of a mesh that holds
/// its positions, as `UpdateMeshBuffer`'s `index`.
pub const MESH_BUFFER_POSITIONS: c_int = 0;

/// `rlCullMode::RL_CULL_FACE_FRONT` (rlgl.h): cull OpenGL's front faces, the counter-clockwise ones.
pub const RL_CULL_FACE_FRONT: c_int = 0;
/// `rlCullMode::RL_CULL_FACE_BACK` (rlgl.h): cull OpenGL's back faces, rlgl's setting from the start.
pub const RL_CULL_FACE_BACK: c_int = 1;

extern "C" {
    // Window and frame loop (rcore.c).
    pub fn SetConfigFlags(flags: c_uint);
    pub fn SetTraceLogLevel(log_level: c_int);
    pub fn InitWindow(width: c_int, height: c_int, title: *const c_char);
    pub fn IsWindowReady() -> bool;
    pub fn SetWindowSize(width: c_int, height: c_int);
    pub fn SetWindowTitle(title: *const c_char);
    pub fn SetExitKey(key: c_int);
    pub fn WindowShouldClose() -> bool;
    pub fn CloseWindow();
    pub fn BeginDrawing();
    pub fn EndDrawing();
    pub fn ClearBackground(color: Color);
    pub fn GetFrameTime() -> f32;
    pub fn GetTime() -> f64;
    pub fn GetScreenWidth() -> c_int;
    pub fn GetScreenHeight() -> c_int;
    pub fn GetFPS() -> c_int;

    // Input (rcore.c).
    pub fn IsKeyPressed(key: c_int) -> bool;
    pub fn IsKeyDown(key: c_int) -> bool;
    pub fn IsKeyReleased(key: c_int) -> bool;
    pub fn IsMouseButtonPressed(button: c_int) -> bool;
    pub fn IsMouseButtonDown(button: c_int) -> bool;
    pub fn IsMouseButtonReleased(button: c_int) -> bool;
    pub fn GetMousePosition() -> Vector2;
    pub fn GetMouseWheelMove() -> f32;
    pub fn GetMouseDelta() -> Vector2;
    pub fn DisableCursor();
    pub fn EnableCursor();

    // Shapes (rshapes.c).
    pub fn DrawRectangleV(position: Vector2, size: Vector2, color: Color);
    pub fn DrawRectangleLinesEx(rec: Rectangle, line_thick: f32, color: Color);
    pub fn DrawCircleSector(center: Vector2, radius: f32, start_angle: f32, end_angle: f32, segments: c_int, color: Color);
    pub fn DrawRing(
        center: Vector2,
        inner_radius: f32,
        outer_radius: f32,
        start_angle: f32,
        end_angle: f32,
        segments: c_int,
        color: Color,
    );
    pub fn DrawLineEx(start_pos: Vector2, end_pos: Vector2, thick: f32, color: Color);
    pub fn DrawTriangle(v1: Vector2, v2: Vector2, v3: Vector2, color: Color);

    // Text (rtext.c).
    pub fn GetFontDefault() -> Font;
    pub fn DrawTextEx(font: Font, text: *const c_char, position: Vector2, font_size: f32, spacing: f32, tint: Color);
    pub fn MeasureTextEx(font: Font, text: *const c_char, font_size: f32, spacing: f32) -> Vector2;

    // Images (rtextures.c).
    pub fn GenImageColor(width: c_int, height: c_int, color: Color) -> Image;
    pub fn ImageDrawPixel(dst: *mut Image, pos_x: c_int, pos_y: c_int, color: Color);
    /// Writes with C's `fopen`, which takes the ANSI code page on Windows, so a path with a character
    /// outside it can't be written. Kestrel saves with `ExportImageToMemory`.
    pub fn ExportImage(image: Image, file_name: *const c_char) -> bool;
    /// Gives a buffer that `MemFree` releases. Only `".png"` is supported.
    pub fn ExportImageToMemory(image: Image, file_type: *const c_char, file_size: *mut c_int) -> *mut u8;
    /// Kestrel reads image files itself (`image_file.rs`), because raylib's `LoadImage` opens them with
    /// C's `fopen`, which takes the ANSI code page on Windows, so a path with a character outside it
    /// can't be read.
    pub fn LoadImageFromMemory(file_type: *const c_char, file_data: *const u8, data_size: c_int) -> Image;
    pub fn LoadImageFromScreen() -> Image;
    pub fn GetImageColor(image: Image, x: c_int, y: c_int) -> Color;
    pub fn UnloadImage(image: Image);
    pub fn LoadTextureFromImage(image: Image) -> Texture;
    pub fn UnloadTexture(texture: Texture);
    pub fn SetTextureFilter(texture: Texture, filter: c_int);
    pub fn SetTextureWrap(texture: Texture, wrap: c_int);
    pub fn DrawTexturePro(texture: Texture, source: Rectangle, dest: Rectangle, origin: Vector2, rotation: f32, tint: Color);

    // 3D mode (rcore.c). Each one flushes the batch first, so what was drawn stays under what follows.
    /// Starts a perspective projection (near 0.05, far 4000), the camera's view, and the depth
    /// test. Pushes the projection matrix, which `EndMode3D` pops.
    pub fn BeginMode3D(camera: Camera3D);
    /// Puts back the 2D projection and turns the depth test off. Backface culling is untouched.
    pub fn EndMode3D();

    // 3D shapes (rmodels.c).
    pub fn DrawLine3D(start_pos: Vector3, end_pos: Vector3, color: Color);
    pub fn DrawCube(position: Vector3, width: f32, height: f32, length: f32, color: Color);
    pub fn DrawSphere(center_pos: Vector3, radius: f32, color: Color);

    // Models (rmodels.c).
    /// Makes the mesh's vertex array and buffers, sized by `vertexCount`, from its CPU arrays. A
    /// null array that the shader needs (colours, texture coordinates) gets a fixed value instead.
    pub fn UploadMesh(mesh: *mut Mesh, dynamic: bool);
    /// Writes `data_size` bytes of `data` into the mesh's vertex buffer `index`, at byte `offset`.
    pub fn UpdateMeshBuffer(mesh: Mesh, index: c_int, data: *const c_void, data_size: c_int, offset: c_int);
    pub fn UnloadMesh(mesh: Mesh);
    pub fn DrawMesh(mesh: Mesh, material: Material, transform: Matrix);
    /// raylib's default shader and texture (both raylib's own), with `maps` allocated for this material.
    pub fn LoadMaterialDefault() -> Material;

    // Memory (rcore.c): frees what raylib allocated and handed over.
    pub fn MemFree(ptr: *mut c_void);

    // rlgl.h: flush the batch of shapes drawn so far.
    pub fn rlDrawRenderBatchActive();
    pub fn rlEnableBackfaceCulling();
    pub fn rlDisableBackfaceCulling();
    /// Which faces culling removes: `RL_CULL_FACE_FRONT` or `RL_CULL_FACE_BACK` (`glCullFace`).
    pub fn rlSetCullFace(mode: c_int);
}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use std::mem::{align_of, offset_of, size_of};

    use super::*;

    /// The structs `raylib.h` passes to the mesh functions, on a 64-bit target.
    #[test]
    fn the_material_layouts_are_raylibs() {
        assert_eq!((size_of::<Matrix>(), align_of::<Matrix>()), (64, 4));
        assert_eq!((offset_of!(Matrix, m4), offset_of!(Matrix, m1), offset_of!(Matrix, m15)), (4, 16, 60));
        assert_eq!((size_of::<Shader>(), offset_of!(Shader, locs)), (16, 8));
        assert_eq!(size_of::<Texture>(), 20);
        assert_eq!((size_of::<MaterialMap>(), offset_of!(MaterialMap, color), offset_of!(MaterialMap, value)), (28, 20, 24));
        assert_eq!((size_of::<Material>(), offset_of!(Material, maps), offset_of!(Material, params)), (40, 16, 24));
    }

    /// The camera `BeginMode3D` takes by value: three `Vector3`s, a float and an int.
    #[test]
    fn the_camera_layout_is_raylibs() {
        assert_eq!((size_of::<Vector3>(), align_of::<Vector3>()), (12, 4));
        assert_eq!((size_of::<Camera3D>(), align_of::<Camera3D>()), (44, 4));
        let offsets = (offset_of!(Camera3D, target), offset_of!(Camera3D, up), offset_of!(Camera3D, fovy), offset_of!(Camera3D, projection));
        assert_eq!(offsets, (12, 24, 36, 40));
    }
}
