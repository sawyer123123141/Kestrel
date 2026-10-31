//! The window's state, and the switch between the two backends.
//!
//! A program has one window, used from the thread that runs `main`, so the
//! state is a `thread_local`. It is *not opened*, *open* or *closed*.
//!
//! - The **real** backend drives raylib.
//! - The **headless** backend (`KESTREL_HEADLESS=<n>`) makes no raylib call:
//!   `frame()` gives `true` `n` times, the clock is 60 frames a second, and
//!   the natives print what they would have drawn (see `headless.rs`), unless
//!   `KESTREL_HEADLESS_QUIET=1` turns the lines off.
//! - The **replay** backend (`kestrel replay`) is the headless one, whatever the environment
//!   says, with as many frames as the recording ran (`kestrel_runtime::replay::note_frame`). It
//!   prints the headless lines only when the recording was made headless and not quiet, since
//!   only that recording has them to compare.
//!
//! Both follow the same rules, so a program behaves alike on either:
//! `close()` only asks, and the next `frame()` closes the window and gives
//! `false`. After that `frame()` keeps giving `false`, drawing and
//! `window.screenshot` are errors, and the queries (`delta`, `time`, `ticks_usec`,
//! `size`, `fps`) give the values they had at the end, because raylib has nothing
//! left to ask. Input reads as nothing.
//!
//! The GPU objects (`gpu.rs`) are freed at the start of each frame, once raylib has drawn
//! everything batched, and all of them when the window closes, in the real case before
//! raylib closes. A 3D pass (`pass.rs`) ends before the frame does, and every frame starts in 2D.
//!
//! The `check_*` functions give an error message instead of failing, so tests
//! can see the message. The same functions without the prefix call
//! `kestrel_runtime::panic::fail_at_call` with it, which adds the line of the program's call.

use std::cell::RefCell;
use std::env;

use kestrel_runtime::panic::fail_at_call;
use kestrel_runtime::replay::{self, WindowMode};

use crate::abi::V2;
use crate::gpu;
use crate::headless;
use crate::pass;
use crate::raylib as rl;
use crate::real;

/// Which backend a window uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Real,
    Headless,
}

/// The values of the window queries when raylib isn't asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fixed {
    pub size: V2,
    pub time: f32,
    /// `window.ticks_usec()`: microseconds, exact however long the program has run.
    pub ticks: i64,
    pub delta: f32,
    pub fps: i32,
}

/// Where a query reads from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    /// Ask raylib. `time_base` is raylib's `GetTime()` when the window opened:
    /// `window.time()` counts from there, not from when GLFW started.
    Real { time_base: f64 },
    /// These values: a headless window, or one that has closed.
    Fixed(Fixed),
}

/// What the environment says about the window, read at `window.open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `KESTREL_HEADLESS=<n>`: no window, and `n` frames.
    pub headless: Option<u64>,
    /// `KESTREL_HIDDEN=1`: a real window that isn't shown.
    pub hidden: bool,
    /// `KESTREL_HEADLESS_QUIET=1`: a headless window prints none of its lines.
    pub quiet: bool,
}

impl Settings {
    /// The window mode a recording made with these settings names in its file.
    pub fn window_mode(&self) -> WindowMode {
        match (self.headless, self.quiet, self.hidden) {
            (Some(_), true, _) => WindowMode::HeadlessQuiet,
            (Some(_), false, _) => WindowMode::Headless,
            (None, _, true) => WindowMode::Hidden,
            (None, _, false) => WindowMode::Real,
        }
    }

    pub fn from_env() -> Result<Settings, String> {
        let var = |name| env::var(name).ok();
        Settings::parse(var("KESTREL_HEADLESS").as_deref(), var("KESTREL_HIDDEN").as_deref(), var("KESTREL_HEADLESS_QUIET").as_deref())
    }

    pub fn parse(headless: Option<&str>, hidden: Option<&str>, quiet: Option<&str>) -> Result<Settings, String> {
        let headless = match headless.map(str::trim) {
            None | Some("") => None,
            Some(n) => Some(
                n.parse::<u64>()
                    .map_err(|_| format!("KESTREL_HEADLESS must be a whole number of frames, like KESTREL_HEADLESS=3, not {n:?}"))?,
            ),
        };
        Ok(Settings { headless, hidden: hidden == Some("1"), quiet: quiet == Some("1") })
    }
}

enum Kind {
    Real { time_base: f64 },
    Headless { frames_to_run: u64 },
    /// A replay: the headless path, its frames counted by the replay.
    Replay,
}

struct Window {
    kind: Kind,
    /// The size it was opened with, or last opened with: what a headless window reports.
    size: V2,
    /// How many frames have started: the `true`s `frame()` has given.
    frames: u64,
    close_requested: bool,
}

enum State {
    NotOpened,
    Open(Window),
    Closed(Fixed),
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State::NotOpened) };
}

fn before_open(name: &str) -> String {
    format!("{name} was called before window.open")
}

fn after_close(name: &str) -> String {
    format!("{name} was called after the window closed")
}

impl Window {
    fn mode(&self) -> Mode {
        match self.kind {
            Kind::Real { .. } => Mode::Real,
            Kind::Headless { .. } | Kind::Replay => Mode::Headless,
        }
    }

    /// A headless window's query values.
    fn headless_values(&self) -> Fixed {
        let fps = headless::FPS;
        let ticks = (self.frames.saturating_mul(1_000_000) / fps as u64) as i64;
        Fixed { size: self.size, time: self.frames as f32 / fps as f32, ticks, delta: 1.0 / fps as f32, fps }
    }

    /// Ends the previous frame and starts the next, or closes the window.
    /// `None` means a new frame has started; `Some` means the window has
    /// closed, with the values its queries keep giving.
    fn next_frame(&mut self) -> Option<Fixed> {
        match self.kind {
            Kind::Headless { .. } | Kind::Replay => {
                let more = match self.kind {
                    Kind::Headless { frames_to_run } => self.frames < frames_to_run,
                    _ => replay::note_frame(),
                };
                if self.close_requested || !more {
                    gpu::close_all(false);
                    return Some(self.headless_values());
                }
                gpu::drain(false);
            }
            Kind::Real { time_base } => {
                pass::enter_2d(true); // a frame, or the window, never ends inside a 3D pass
                // SAFETY: raylib calls on the thread that opened the window, which is still open.
                unsafe {
                    if self.frames > 0 {
                        rl::EndDrawing();
                    } else {
                        // Before the first frame no `EndDrawing` has drawn the batch, and a program may
                        // have drawn into it: draw it now, so it holds nothing `gpu::drain` frees below.
                        rl::rlDrawRenderBatchActive();
                    }
                    if self.close_requested || rl::WindowShouldClose() {
                        let last = Fixed {
                            size: real::size(),
                            time: real::time(time_base),
                            ticks: real::ticks(time_base),
                            delta: rl::GetFrameTime(),
                            fps: rl::GetFPS(),
                        };
                        gpu::close_all(true);
                        rl::CloseWindow();
                        return Some(last);
                    }
                    gpu::drain(true);
                    rl::BeginDrawing();
                }
            }
        }
        pass::start_frame();
        self.frames += 1;
        None
    }
}

/// `window.open`: opens the window, or says why it can't. While the window is open it
/// applies the size and title instead: the real window is resized and retitled, a headless
/// one reports the new size, and a replay ignores sizes. Its frames and clock go on.
pub fn open(settings: &Settings, width: i32, height: i32, title: &[u8]) -> Result<Mode, String> {
    STATE.with_borrow_mut(|state| {
        if let State::Closed(_) = state {
            return Err(after_close("window.open"));
        }
        if width <= 0 || height <= 0 {
            return Err(format!("window.open needs a width and height above 0, not {width} by {height}"));
        }
        let size = V2 { x: width as f32, y: height as f32 };
        if let State::Open(window) = state {
            match window.kind {
                Kind::Real { .. } => real::reopen(width, height, title),
                Kind::Headless { .. } => window.size = size,
                Kind::Replay => {}
            }
            return Ok(window.mode());
        }
        let kind = match (replay::window_mode_for_replay(), settings.headless) {
            (Some(recorded), _) => {
                headless::set_quiet(recorded != WindowMode::Headless);
                Kind::Replay
            }
            (None, Some(frames_to_run)) => {
                headless::set_quiet(settings.quiet);
                Kind::Headless { frames_to_run }
            }
            (None, None) => Kind::Real { time_base: real::init(settings, width, height, title)? },
        };
        let window = Window { kind, size, frames: 0, close_requested: false };
        let mode = window.mode();
        *state = State::Open(window);
        Ok(mode)
    })
}

/// `window.frame`: finishes the previous frame and starts the next. `false`
/// once the window has closed.
pub fn frame() -> Result<bool, String> {
    STATE.with_borrow_mut(|state| match state {
        State::NotOpened => Err(before_open("window.frame")),
        State::Closed(_) => Ok(false),
        State::Open(window) => match window.next_frame() {
            None => Ok(true),
            Some(last) => {
                *state = State::Closed(last);
                Ok(false)
            }
        },
    })
}

/// `window.close`: asks for the window to close at the next `frame()`.
/// Gives the mode when it asked, and `None` when the window had closed already.
pub fn close() -> Result<Option<Mode>, String> {
    STATE.with_borrow_mut(|state| match state {
        State::NotOpened => Err(before_open("window.close")),
        State::Closed(_) => Ok(None),
        State::Open(window) => {
            window.close_requested = true;
            Ok(Some(window.mode()))
        }
    })
}

/// For a native that draws (or saves a screenshot): which backend, if the window is open.
pub fn check_drawing(name: &str) -> Result<Mode, String> {
    STATE.with_borrow(|state| match state {
        State::NotOpened => Err(before_open(name)),
        State::Closed(_) => Err(after_close(name)),
        State::Open(window) => Ok(window.mode()),
    })
}

/// For a native that reads the window or the input: where to read from.
pub fn check_view(name: &str) -> Result<View, String> {
    STATE.with_borrow(|state| match state {
        State::NotOpened => Err(before_open(name)),
        State::Closed(last) => Ok(View::Fixed(*last)),
        State::Open(window) => match window.kind {
            Kind::Real { time_base } => Ok(View::Real { time_base }),
            Kind::Headless { .. } | Kind::Replay => Ok(View::Fixed(window.headless_values())),
        },
    })
}

/// `check_drawing`, failing the program with the message.
pub fn drawing(name: &str) -> Mode {
    check_drawing(name).unwrap_or_else(|msg| fail_at_call(&msg))
}

/// `check_view`, failing the program with the message.
pub fn view(name: &str) -> View {
    check_view(name).unwrap_or_else(|msg| fail_at_call(&msg))
}

#[cfg(test)]
#[path = "state_test.rs"]
mod tests;
