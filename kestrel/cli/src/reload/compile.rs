//! Compiling a saved program off the game thread (hot reload design, decision 6): the same steps
//! as `kestrel run` (parse, check, prove, lower, JIT) on a thread of its own, so the game keeps
//! running frames while it works.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant, SystemTime};

use kestrel_codegen::{JitProgram, Options};
use kestrel_driver::{compile_for_reload, Compiled, DiagStyle};

/// The compiler recurses over the syntax tree, so it runs on a big stack, as `kestrel run` does.
const STACK_SIZE: usize = 256 * 1024 * 1024;

/// A compiled program, ready to swap in.
pub struct Built {
    pub program: JitProgram,
    pub compiled: Compiled,
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
/// would print for its errors.
pub fn spawn(job: &Job, files: Vec<PathBuf>, saved: Option<SystemTime>) -> Receiver<Result<Built, String>> {
    let (tx, rx) = channel();
    let job = job.clone();
    let started = Instant::now();
    let failed = tx.clone();
    let spawned = std::thread::Builder::new().name("kestrel compile".into()).stack_size(STACK_SIZE).spawn(move || {
        // Cranelift's constant folding and the compiler's casts follow this thread's float mode.
        kestrel_runtime::fpu::pin();
        settle(&files);
        let _ = tx.send(build(&job, started, saved));
    });
    if let Err(e) = spawned {
        let _ = failed.send(Err(format!("kestrel: couldn't start compiling the saved program: {e}\n")));
    }
    rx
}

/// Compiles `job` for the reloading host (a reloadable program comes without its generated `main`).
fn build(job: &Job, started: Instant, saved: Option<SystemTime>) -> Result<Built, String> {
    let compiled = compile_for_reload(&job.path, job.style, job.release)?;
    let natives = crate::natives();
    let program = JitProgram::compile_with(&compiled.module, job.opts, &natives)
        .map_err(|e| format!("kestrel: internal compiler error: {e}\n"))?;
    Ok(Built { program, compiled, started, saved })
}

/// Waits, for at most `SETTLE`, until every file in `files` can be opened: an editor that is still
/// saving one can hold it shut for a moment, and reading it then would report an error that the
/// next moment no longer has.
fn settle(files: &[PathBuf]) {
    let end = Instant::now() + SETTLE;
    for path in files {
        while std::fs::File::open(path).is_err() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
