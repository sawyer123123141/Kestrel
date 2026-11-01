//! Compiling a saved program off the game thread (hot reload design, decision 6): the same steps
//! as `kestrel run` (parse, check, prove, lower, JIT) on a thread of its own, so the game keeps
//! running frames while it works.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant, SystemTime};

use kestrel_codegen::{JitProgram, Options};
use kestrel_driver::reload::CompileFailure;
use kestrel_driver::{compile_for_reload, Compiled, DiagStyle};

/// The compiler recurses over the syntax tree, so it runs on a big stack, as `kestrel run` does.
const STACK_SIZE: usize = 256 * 1024 * 1024;

/// The compile thread's name. The CLI's panic hook lets a panic on it unwind, so the compile ends
/// with an error and the game keeps running.
pub const THREAD: &str = "kestrel compile";

/// What the host prints when the compiler panicked on a save (the panic hook printed the panic).
pub const PANICKED: &str = "kestrel: the compiler failed on this save (an internal error); the previous code keeps running. Save again after the next edit, and report the panic above.\n";

/// What a compile gives: the program, or why it failed.
pub type Outcome = Result<Built, CompileFailure>;

/// The compile step a compile thread runs: `build`, or a stand-in in a test.
type Build = fn(&Job, Instant, Option<SystemTime>) -> Outcome;

/// A compiled program, ready to swap in.
pub struct Built {
    pub program: JitProgram,
    pub compiled: Compiled,
    /// The compile's warnings, as `kestrel run` prints them (empty when there are none).
    pub warnings: String,
    /// When the compile started.
    pub started: Instant,
    /// When the file was saved, as its modification time says.
    pub saved: Option<SystemTime>,
}

/// What a compile thread was asked: the program's entry file and its build.
#[derive(Clone)]
pub struct Job {
    pub path: PathBuf,
    pub style: DiagStyle,
    pub release: bool,
    pub opts: Options,
}

/// How long a compile waits for a file an editor is still writing (Windows refuses to open a file
/// another program holds open for writing), before it reports the error it gets.
const SETTLE: Duration = Duration::from_secs(1);

/// Starts compiling `job`, saved at `saved`, on a thread of its own, once each of `files` (the
/// program's own files) can be opened. The receiver yields the program, or the text `kestrel run`
/// would print for its errors with the files the compile read.
pub fn spawn(job: &Job, files: Vec<PathBuf>, saved: Option<SystemTime>) -> Receiver<Outcome> {
    spawn_with(job, files, saved, build)
}

/// `spawn`, with the compile step `build`.
fn spawn_with(job: &Job, files: Vec<PathBuf>, saved: Option<SystemTime>, build: Build) -> Receiver<Outcome> {
    let (tx, rx) = channel();
    let job = job.clone();
    let started = Instant::now();
    let failed = tx.clone();
    let spawned = std::thread::Builder::new().name(THREAD.into()).stack_size(STACK_SIZE).spawn(move || {
        // Cranelift's constant folding and the compiler's casts follow this thread's float mode.
        kestrel_runtime::fpu::pin();
        settle(&files);
        let _ = tx.send(guarded(build, &job, started, saved));
    });
    if let Err(e) = spawned {
        let text = format!("kestrel: couldn't start compiling the saved program: {e}\n");
        let _ = failed.send(Err(CompileFailure { text, files: Vec::new() }));
    }
    rx
}

/// Runs `build`, turning a compiler panic into a failed compile (the panic hook has printed the
/// panic), so a half-saved file that trips a compiler bug never ends the game.
fn guarded(build: Build, job: &Job, started: Instant, saved: Option<SystemTime>) -> Outcome {
    catch_unwind(AssertUnwindSafe(|| build(job, started, saved)))
        .unwrap_or_else(|_| Err(CompileFailure { text: PANICKED.to_string(), files: Vec::new() }))
}

/// Compiles `job` for the reloading host (a reloadable program comes without its generated `main`).
fn build(job: &Job, started: Instant, saved: Option<SystemTime>) -> Outcome {
    let (compiled, warnings) = compile_for_reload(&job.path, job.style, job.release)?;
    let natives = crate::natives();
    match JitProgram::compile_with(&compiled.module, job.opts, &natives) {
        Ok(program) => Ok(Built { program, compiled, warnings, started, saved }),
        Err(e) => Err(CompileFailure { text: format!("kestrel: internal compiler error: {e}\n"), files: compiled.files }),
    }
}

/// Waits, for at most `SETTLE`, until every file in `files` can be opened: an editor that is still
/// saving one can hold it shut for a moment, and reading it then would report an error that the
/// next moment no longer has. A file that doesn't exist (a module the program names but that isn't
/// written yet) isn't waited for.
fn settle(files: &[PathBuf]) {
    let end = Instant::now() + SETTLE;
    for path in files {
        while path.exists() && std::fs::File::open(path).is_err() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Instant, SystemTime};

    use kestrel_codegen::Options;
    use kestrel_driver::DiagStyle;

    use super::{spawn_with, Job, Outcome, PANICKED, THREAD};

    fn panics(_: &Job, _: Instant, _: Option<SystemTime>) -> Outcome {
        assert_eq!(std::thread::current().name(), Some(THREAD));
        panic!("a compiler bug");
    }

    #[test]
    fn a_compiler_panic_is_a_failed_compile() {
        let job = Job { path: PathBuf::from("prog.kes"), style: DiagStyle::Full, release: false, opts: Options::default() };
        match spawn_with(&job, Vec::new(), None, panics).recv() {
            Ok(Err(failure)) => {
                assert_eq!(failure.text, PANICKED);
                assert!(failure.files.is_empty());
            }
            Ok(Ok(_)) => panic!("a panicking compile gave a program"),
            Err(e) => panic!("the compile thread ended without a result: {e}"),
        }
    }
}
