//! Compiles the vendored raylib (`raylib/`) into a static library.
//!
//! There is no bindgen: the functions Kestrel uses are declared by hand in
//! `src/raylib.rs`. Audio is switched off; the models module is on, for meshes, and JPG loading
//! is on, for textures (PNG and BMP are on in raylib's defaults).
//!
//! Only Windows builds have the window: elsewhere the crate is empty (see `src/lib.rs`), so
//! raylib is not compiled at all.

fn main() {
    println!("cargo:rerun-if-changed=raylib");
    println!("cargo:rerun-if-changed=build.rs");

    // The build script runs on the host, so ask for the target's OS.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    cc::Build::new()
        .files(
            ["rcore.c", "rshapes.c", "rtextures.c", "rtext.c", "rmodels.c", "rglfw.c"]
                .map(|file| format!("raylib/{file}")),
        )
        .include("raylib")
        .include("raylib/external/glfw/include")
        .define("PLATFORM_DESKTOP_GLFW", None)
        .define("GRAPHICS_API_OPENGL_33", None)
        .define("SUPPORT_MODULE_RAUDIO", "0")
        .define("SUPPORT_MODULE_RMODELS", "1")
        .define("SUPPORT_FILEFORMAT_JPG", "1")
        .warnings(false)
        .compile("raylib");

    for lib in ["opengl32", "gdi32", "winmm", "user32", "shell32"] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}
