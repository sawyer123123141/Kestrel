//! Kestrel's window, drawing, 3D drawing and input natives, built on raylib.
//!
//! A staticlib of this crate holds `kestrel-runtime`'s `kestrel_rt_*` symbols as
//! well as raylib's, so a built program links this one library.
//!
//! Only Windows builds have the window for now: on any other platform this crate is empty and
//! raylib is not compiled, and `kestrel` refuses programs that use `window`, `draw`, `draw3d` or
//! `input` with a sentence saying so.
#![cfg(windows)]

// Naming the crate makes rustc link it, so its `kestrel_rt_*` exports land in
// this crate's staticlib.
pub use kestrel_runtime;
mod abi;
mod bulk;
mod gpu;
mod headless;
mod image_file;
mod keys;
mod material;
mod mesh_data;
mod natives_3d;
mod natives_draw;
mod natives_input;
mod natives_mesh;
mod natives_texture;
mod natives_window;
mod pass;
mod paths;
pub mod raylib;
mod real;
mod state;
#[cfg(test)]
mod native_tests;

use std::path::Path;

pub use paths::set_program_dir;

use raylib::{GetImageColor, UnloadImage};

/// The window mode the environment asks for (`KESTREL_HEADLESS`, `KESTREL_HEADLESS_QUIET`,
/// `KESTREL_HIDDEN`), as a recording names it in its file.
pub fn window_mode() -> Result<kestrel_runtime::replay::WindowMode, String> {
    state::Settings::from_env().map(|settings| settings.window_mode())
}

/// The native functions, as `(symbol, address)` pairs: what the JIT needs to
/// link a program that uses `window`, `draw`, `draw3d` or `input`. A built program finds
/// the same symbols in this crate's staticlib.
pub fn symbols() -> Vec<(&'static str, *const u8)> {
    use natives_3d::*;
    use natives_draw::*;
    use natives_input::*;
    use natives_mesh::*;
    use natives_texture::*;
    use natives_window::*;

    // Names come from the identifiers, so a symbol can't drift from its function.
    macro_rules! table {
        ($($native:ident),* $(,)?) => { vec![$((stringify!($native), $native as *const u8)),*] };
    }
    table![
        kestrel_window_open,
        kestrel_window_frame,
        kestrel_window_close,
        kestrel_window_delta,
        kestrel_window_time,
        kestrel_window_size,
        kestrel_window_fps,
        kestrel_window_ticks_usec,
        kestrel_window_screenshot,
        kestrel_draw_clear,
        kestrel_draw_rect,
        kestrel_draw_rect_outline,
        kestrel_draw_rects,
        kestrel_draw_circle,
        kestrel_draw_circle_outline,
        kestrel_draw_line,
        kestrel_draw_triangle,
        kestrel_draw_text,
        kestrel_draw_text_width,
        kestrel_draw_load_texture,
        kestrel_draw_make_texture,
        kestrel_draw_texture,
        kestrel_draw_texture_rect,
        kestrel_draw_texture_region,
        kestrel_draw3d_camera,
        kestrel_draw3d_make_mesh,
        kestrel_draw3d_mesh,
        kestrel_draw3d_mesh_textured,
        kestrel_draw3d_cube,
        kestrel_draw3d_sphere,
        kestrel_draw3d_line,
        kestrel_input_held,
        kestrel_input_pressed,
        kestrel_input_released,
        kestrel_input_mouse,
        kestrel_input_wheel,
        kestrel_input_capture,
        kestrel_input_mouse_delta,
    ]
}

/// Reads the pixel at `(x, y)` of the image file at `path` as `[r, g, b, a]`.
/// No window is needed. Gives `None` if the file can't be read as an image or
/// the point is outside it.
pub fn png_pixel(path: &Path, x: i32, y: i32) -> Option<[u8; 4]> {
    png_pixels(path, &[(x, y)])?.pop().flatten()
}

/// Reads the pixels at `points` of the image file at `path`, loading the file
/// once. Gives `None` if the file can't be read as an image; a point outside it
/// gives `None` in its place.
pub fn png_pixels(path: &Path, points: &[(i32, i32)]) -> Option<Vec<Option<[u8; 4]>>> {
    // Read by Rust, so a path raylib's own `fopen` can't open (a non-ASCII folder on Windows) still works.
    let image = image_file::load(path).ok()?;
    // SAFETY: `image` was just loaded. It is read only while loaded and is unloaded once, before returning.
    unsafe {
        let pixels = points
            .iter()
            .map(|&(x, y)| {
                let inside = (0..image.width).contains(&x) && (0..image.height).contains(&y);
                inside.then(|| GetImageColor(image, x, y)).map(|c| [c.r, c.g, c.b, c.a])
            })
            .collect();
        UnloadImage(image);
        Some(pixels)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::raylib::{Color, ExportImage, GenImageColor, ImageDrawPixel, SetTraceLogLevel, UnloadImage, LOG_ERROR};
    use super::*;

    /// The test image the texture programs load: four 2x2 cells, all opaque.
    #[test]
    fn the_checker_test_image_has_four_colour_cells() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/programs/assets/checker.png");
        let (red, green, blue, white) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]);
        for (y, row) in [[red, green], [red, green], [blue, white], [blue, white]].into_iter().enumerate() {
            for x in 0..4 {
                assert_eq!(png_pixel(&path, x, y as i32), Some(row[x as usize / 2]), "pixel ({x}, {y})");
            }
        }
        assert_eq!(png_pixel(&path, 4, 0), None, "4 pixels wide");
        assert_eq!(png_pixel(&path, 0, 4), None, "and 4 tall");
    }

    #[test]
    fn jpg_files_are_written_and_read_because_the_build_switches_them_on() {
        let red = Color { r: 255, g: 0, b: 0, a: 255 };
        let path = std::env::temp_dir().join(format!("kestrel_window_{}.jpg", std::process::id()));
        let c_path = CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: a fresh image, exported and freed once.
        unsafe {
            SetTraceLogLevel(LOG_ERROR);
            let image = GenImageColor(8, 8, red);
            assert!(ExportImage(image, c_path.as_ptr()), "raylib was built without JPG support");
            UnloadImage(image);
        }
        // JPG is lossy, so a red image comes back as almost red.
        let [r, g, b, a] = png_pixel(&path, 4, 4).expect("raylib was built without JPG support");
        assert!(r > 240 && g < 15 && b < 15 && a == 255, "{r} {g} {b} {a}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn png_round_trip() {
        let black = Color { r: 0, g: 0, b: 0, a: 255 };
        let red = Color { r: 255, g: 0, b: 0, a: 255 };
        let path = std::env::temp_dir().join(format!("kestrel_window_{}.png", std::process::id()));
        let c_path = CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: a fresh image, drawn within its bounds, exported and freed once.
        unsafe {
            SetTraceLogLevel(LOG_ERROR);
            let mut image = GenImageColor(4, 4, black);
            ImageDrawPixel(&mut image, 2, 1, red);
            assert!(ExportImage(image, c_path.as_ptr()));
            UnloadImage(image);
        }

        assert_eq!(png_pixel(&path, 2, 1), Some([255, 0, 0, 255]));
        assert_eq!(png_pixel(&path, 0, 0), Some([0, 0, 0, 255]));
        assert_eq!(png_pixel(&path, 4, 0), None, "outside the image");
        assert_eq!(png_pixels(&path, &[(2, 1), (0, 4), (3, 3)]), Some(vec![Some([255, 0, 0, 255]), None, Some([0, 0, 0, 255])]));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(png_pixel(&path, 0, 0), None, "missing file");
    }
}
