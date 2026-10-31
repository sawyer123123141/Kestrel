//! The real backend's raylib calls that aren't part of drawing: opening the
//! window, resizing and retitling it, and the queries that ask it for its size and the time. `state.rs`
//! decides when to call them.

use crate::abi::{c_string, V2};
use crate::raylib as rl;
use crate::state::Settings;

/// The real window's size in pixels.
pub fn size() -> V2 {
    // SAFETY: only called while a real window is open.
    unsafe { V2 { x: rl::GetScreenWidth() as f32, y: rl::GetScreenHeight() as f32 } }
}

/// Seconds since `time_base`, a value `GetTime()` gave.
pub fn time(time_base: f64) -> f32 {
    // SAFETY: only called while a real window is open.
    unsafe { (rl::GetTime() - time_base) as f32 }
}

/// Microseconds since `time_base`, a value `GetTime()` gave.
pub fn ticks(time_base: f64) -> i64 {
    // SAFETY: only called while a real window is open.
    unsafe { ((rl::GetTime() - time_base) * 1_000_000.0) as i64 }
}

/// Opens the real window. Gives `GetTime()` as of now, the start of `window.time()`.
pub fn init(settings: &Settings, width: i32, height: i32, title: &[u8]) -> Result<f64, String> {
    let mut flags = rl::FLAG_WINDOW_RESIZABLE | rl::FLAG_VSYNC_HINT | rl::FLAG_MSAA_4X_HINT;
    if settings.hidden {
        flags |= rl::FLAG_WINDOW_HIDDEN;
    }
    let title = c_string(title);
    // SAFETY: `title` is a NUL-terminated string that lives through the call. Nothing else has
    // used raylib on this thread yet.
    unsafe {
        // Errors only: raylib logs its progress on stdout, which would mix with the program's output.
        rl::SetTraceLogLevel(rl::LOG_ERROR);
        rl::SetConfigFlags(flags);
        rl::InitWindow(width, height, title.as_ptr());
        if !rl::IsWindowReady() {
            return Err("window.open could not open a window; check that the graphics driver supports OpenGL 3.3".to_string());
        }
        // Escape doesn't quit; the program decides.
        rl::SetExitKey(rl::KEY_NULL);
        Ok(rl::GetTime())
    }
}

/// `window.open` while the window is open: gives the open window the new size and title.
pub fn reopen(width: i32, height: i32, title: &[u8]) {
    let title = c_string(title);
    // SAFETY: only called while a real window is open, on the thread that opened it; `title` is a
    // NUL-terminated string that lives through the call (raylib copies it into GLFW).
    unsafe {
        rl::SetWindowSize(width, height);
        rl::SetWindowTitle(title.as_ptr());
    }
}
