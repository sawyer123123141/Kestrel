//! Record and replay (the I/O round's spec, decision 7).
//!
//! `kestrel run --record` turns recording on before the program starts; `kestrel replay` turns
//! replaying on with a file's contents. Every native of `window`, `draw`, `draw3d`, `input` and
//! `time` that gives a result passes it through `record_or_play` (`window.frame` through
//! `frame`): a recording appends it to the native log, and a replay hands back the logged value
//! instead, after checking that the log names the same native. Every line `print` writes to
//! stdout reaches `print_bytes`, which keeps it (recording) or compares it with the recorded line
//! (replaying). `finish` ends either when `main` returns, and the runtime's `fail` calls
//! `on_failure` before it exits, so a failure is recorded, and replayed, too.
//!
//! The native log's ids are `NativeId`'s (`ids.rs`), one per logged native in module order.
//!
//! A `par for` body can't print (the checker refuses it), but natives and `print` can run on the
//! compiler's thread as well as the program's, so the state is behind a `Mutex`. When nothing is recorded or replayed, a native pays one atomic load.

mod compare;
mod ids;
mod log;

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

pub use compare::{split, Ending, Report};
pub use ids::{NativeId, WindowMode};
pub use log::LogValue;

use crate::replay_file::ReplayFile;

/// What a recording is of, and where its file goes.
pub struct Recording {
    pub path: PathBuf,
    pub program: String,
    pub version: String,
    /// `checked` or `release`.
    pub build: String,
    pub window: WindowMode,
    pub args: Vec<String>,
}

/// A recording under way.
struct Recorder {
    path: PathBuf,
    /// The file so far: the header, and the native log as it grows.
    file: ReplayFile,
    output: compare::Keep,
    frames: u64,
}

/// A replay under way.
struct Player {
    file: ReplayFile,
    window: WindowMode,
    natives: log::Cursor,
    output: compare::Check,
    /// The `window.frame()` calls that gave `true` so far.
    frames: u64,
}

enum Mode {
    Off,
    Record(Recorder),
    Replay(Player),
}

/// Whether `MODE` is anything but `Off`: the one thing a native reads when it is.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static MODE: Mutex<Mode> = Mutex::new(Mode::Off);

fn mode() -> MutexGuard<'static, Mode> {
    // A panic while it was held can't leave it half-changed in a way that matters here.
    MODE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set(new: Mode) {
    let active = !matches!(new, Mode::Off);
    *mode() = new;
    ACTIVE.store(active, Ordering::SeqCst);
}

/// A difference that stops a replay: its report, printed when the program stops.
#[derive(Debug)]
pub struct Stop(String);

impl Stop {
    /// Prints the report on stdout and ends the process with exit code 1. The caller flushes the
    /// program's own output first, so the report comes after it.
    pub fn exit(self) -> ! {
        set(Mode::Off);
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", self.0);
        let _ = out.flush();
        std::process::exit(1);
    }
}

/// Stops the replay from inside a native: the program's output so far, then the report.
fn stop_in_native(msg: String) -> ! {
    crate::print::flush();
    Stop(msg).exit()
}

/// From now on, results and output are recorded into the file `recording.path` names.
pub fn start_recording(recording: Recording) {
    let file = ReplayFile {
        program: recording.program,
        version: recording.version,
        build: recording.build,
        window: recording.window.as_str().to_string(),
        args: recording.args,
        frames: 0,
        natives: Vec::new(),
        output: Vec::new(),
    };
    set(Mode::Record(Recorder { path: recording.path, file, output: compare::Keep::default(), frames: 0 }));
}

/// From now on, natives give `file`'s results and output is compared with its lines.
pub fn start_replay(file: ReplayFile) {
    // `ReplayFile::from_bytes` refuses an unknown window mode; one made in code is taken as quiet.
    let window = WindowMode::parse(&file.window).unwrap_or(WindowMode::HeadlessQuiet);
    set(Mode::Replay(Player { file, window, natives: log::Cursor::default(), output: compare::Check::default(), frames: 0 }));
}

/// The result of native `id`: `compute`'s, which a recording logs, or the logged one in a replay
/// (which still runs `compute`, on the headless path, for what it does besides). A replay whose
/// log names another native next, or has ended, stops with the call-order message.
pub fn record_or_play<T: LogValue>(id: NativeId, compute: impl FnOnce() -> T) -> T {
    debug_assert_eq!(T::SIZE, id.width(), "{} logs {} bytes", id.name(), id.width());
    if !ACTIVE.load(Ordering::Relaxed) {
        return compute();
    }
    // `compute` may fail the program, which records the failure: the lock isn't held meanwhile.
    let value = compute();
    let played = match &mut *mode() {
        Mode::Off => Ok(value),
        Mode::Record(r) => {
            r.file.natives.push(id as u8);
            value.put(&mut r.file.natives);
            Ok(value)
        }
        Mode::Replay(p) => p.natives.next(&p.file.natives, id, p.frames),
    };
    played.unwrap_or_else(|msg| stop_in_native(msg))
}

/// `window.frame()`: `compute` finishes the frame and says whether another started. It is logged
/// like any result, and counted when `true`. A replay runs `compute` (whose window asks
/// `note_frame` when to stop) and stops if it disagrees with the log.
pub fn frame(compute: impl FnOnce() -> bool) -> bool {
    if !ACTIVE.load(Ordering::Relaxed) {
        return compute();
    }
    let started = compute();
    let result = match &mut *mode() {
        Mode::Off => Ok(()),
        Mode::Record(r) => {
            r.file.natives.push(NativeId::WindowFrame as u8);
            started.put(&mut r.file.natives);
            r.frames += u64::from(started);
            Ok(())
        }
        Mode::Replay(p) => match p.natives.next::<bool>(&p.file.natives, NativeId::WindowFrame, p.frames) {
            Ok(recorded) if recorded == started => {
                p.frames += u64::from(started);
                Ok(())
            }
            Ok(recorded) => Err(format!(
                "replay differs at frame {}: window.frame gave {started} where the recording has {recorded}",
                p.frames
            )),
            Err(msg) => Err(msg),
        },
    };
    if let Err(msg) = result {
        stop_in_native(msg);
    }
    started
}

/// Whether a replay's window may start another frame: `false` once it has run as many as the
/// recording did. Always `true` when not replaying.
pub fn note_frame() -> bool {
    if !ACTIVE.load(Ordering::Relaxed) {
        return true;
    }
    match &*mode() {
        Mode::Replay(p) => p.frames < p.file.frames,
        _ => true,
    }
}

/// Bytes `print` wrote to stdout: kept by a recording, compared by a replay. `Err` when a replay
/// found a different line; the caller flushes stdout, then calls `Stop::exit`.
pub fn print_bytes(bytes: &[u8]) -> Result<(), Stop> {
    if !ACTIVE.load(Ordering::Relaxed) {
        return Ok(());
    }
    match &mut *mode() {
        Mode::Off => Ok(()),
        Mode::Record(r) => {
            r.output.print(bytes);
            Ok(())
        }
        Mode::Replay(p) => p.output.print(&p.file.output, bytes, p.frames).map_err(Stop),
    }
}

/// The recorded arguments, which `args.all()` gives in a replay.
pub fn recorded_args() -> Option<Vec<String>> {
    if !ACTIVE.load(Ordering::Relaxed) {
        return None;
    }
    match &*mode() {
        Mode::Replay(p) => Some(p.file.args.clone()),
        _ => None,
    }
}

/// The recording's window mode in a replay, whose window must then be the replay backend.
pub fn window_mode_for_replay() -> Option<WindowMode> {
    if !ACTIVE.load(Ordering::Relaxed) {
        return None;
    }
    match &*mode() {
        Mode::Replay(p) => Some(p.window),
        _ => None,
    }
}

/// Ends recording or replaying, once: a recording writes its file, and a replay compares the end
/// of the output and gives its result. `Err` when the file couldn't be written.
pub fn finish(ending: Ending) -> Result<Report, String> {
    let taken = std::mem::replace(&mut *mode(), Mode::Off);
    ACTIVE.store(false, Ordering::SeqCst);
    match taken {
        Mode::Off => Ok(Report::Off),
        Mode::Record(mut r) => {
            r.output.end(&ending);
            r.file.output = r.output.entries;
            r.file.frames = r.frames;
            r.file.write(&r.path).map(|()| Report::Recorded)
        }
        Mode::Replay(mut p) => {
            let lines = match p.output.end(&p.file.output, &ending, p.frames) {
                Ok(lines) => lines,
                Err(msg) => return Ok(Report::Differs(msg)),
            };
            Ok(match p.natives.left_over(&p.file.natives, p.frames) {
                Some(msg) => Report::Differs(msg),
                None => Report::Matches { frames: p.frames, lines },
            })
        }
    }
}

/// The runtime's `fail` path, before it reports `msg` and exits 101: a recording writes its file
/// with the failure as the output's last entry. A replay prints the failure and its report, and
/// gives the exit code to end with instead. `None` when the failure goes on as usual.
pub fn on_failure(msg: &str) -> Option<i32> {
    if !ACTIVE.load(Ordering::Relaxed) {
        return None;
    }
    match finish(Ending::Failure(msg.to_string())) {
        Ok(Report::Off | Report::Recorded) => None,
        Ok(report) => {
            eprintln!("kestrel: {msg}");
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", report.text().unwrap_or_default());
            let _ = out.flush();
            Some(report.exit_code())
        }
        Err(e) => {
            eprintln!("kestrel: {e}");
            None
        }
    }
}

/// The runtime's `exit` path, after printed output is flushed and before the process ends with
/// `code`: the exit code to end with instead, or `None` to end with `code`.
///
/// For now it does nothing: recording the code (entry kind 3) and comparing it in a replay come
/// with the replay format's version 2.
pub fn on_exit(code: i32) -> Option<i32> {
    let _ = code;
    None
}

#[cfg(test)]
#[path = "../replay_test.rs"]
mod tests;
