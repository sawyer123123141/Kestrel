//! The reloading host (hot reload design, decisions 2 to 7). A reloadable program (`pub setup() ->
//! State` and `pub frame(var s: State) -> bool`, no `main`) runs on one game thread for the whole
//! session: `setup` once, then `frame` until it returns false. Between two frames the host looks
//! for a saved file, compiles it on a thread of its own and swaps the new code in, keeping the
//! state when its shape is the same. Without reloading (`--no-reload`, `--record`, `replay`) the
//! same loop runs with no watching.

mod compile;
mod swap;
mod watch;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use kestrel_codegen::{JitProgram, Options};
use kestrel_driver::Compiled;

pub use compile::{Job, THREAD as COMPILE_THREAD};
use compile::Outcome;
use kestrel_driver::reload::CompileFailure;
use swap::{Live, StateBuffer};
use watch::Watcher;

use crate::Args;

/// What the reloading host needs to compile a reloadable program again on a save, or `None` when
/// it runs without reloading: under `--no-reload`, `--record` (which says so) and `replay`.
pub fn job(args: &Args, opts: Options, reloadable: bool) -> Option<Job> {
    if !reloadable || args.cmd != "run" || args.no_reload {
        return None;
    }
    if args.record.is_some() {
        eprintln!("--record runs the program without reloading");
        return None;
    }
    Some(Job { path: PathBuf::from(&args.path), style: args.style, release: args.release, opts })
}

/// Runs the reloadable program `program`, compiled from `compiled`, on the game thread: reloading
/// it on every save when `job` is given (the program's file and build, to compile it again), or as
/// it is otherwise. Gives the exit code.
pub fn run(program: JitProgram, compiled: Compiled, job: Option<Job>) -> Result<i32, String> {
    let info = compiled.module.reload.clone().ok_or("the program has no `setup` and `frame` to run")?;
    let live = Live::new(program, info).map_err(|e| format!("internal compiler error: {e}"))?;
    let files = compiled.files;
    drop(compiled.module);
    Ok(kestrel_runtime::start::start_host(move || session(live, job.map(|job| Reloader::new(job, files)))))
}

/// The session on the game thread: `setup`, then `frame` until it returns false, with a reload
/// between two frames whenever one is ready.
fn session(mut live: Live, mut reloader: Option<Reloader>) -> i32 {
    live.init();
    let mut buffer = StateBuffer::new(&live.info);
    live.setup(&mut buffer);
    kestrel_runtime::print::flush();
    loop {
        if let Some(reloader) = reloader.as_mut() {
            reloader.between_frames(&mut live, &mut buffer);
        }
        let more = live.frame(&mut buffer);
        kestrel_runtime::print::flush();
        if !more {
            return 0;
        }
    }
}

/// What reloading keeps between frames: the files watched, the compile running, if any, and
/// whether a save came in while it ran.
struct Reloader {
    job: Job,
    /// The program file's name, for `reloaded <name> in <t> s`.
    name: String,
    watcher: Watcher,
    compiling: Option<Receiver<Outcome>>,
    /// A save was seen while a compile ran: look again as soon as it ends.
    queued: bool,
}

impl Reloader {
    fn new(job: Job, files: Vec<kestrel_driver::SourceFile>) -> Reloader {
        let name = Path::new(&job.path).file_name().map_or_else(|| job.path.display().to_string(), |n| n.to_string_lossy().into_owned());
        Reloader { job, name, watcher: Watcher::new(files), compiling: None, queued: false }
    }

    /// Swaps in a finished compile, and starts one when a file was saved: one compile at a time,
    /// and a save during it makes the host look again once it ends.
    fn between_frames(&mut self, live: &mut Live, buffer: &mut StateBuffer) {
        if let Some(rx) = &self.compiling {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    let text = "kestrel: the compile stopped without a result; save again to retry\n".to_string();
                    Some(Err(CompileFailure { text, files: Vec::new() }))
                }
            };
            if let Some(result) = result {
                self.compiling = None;
                swap::apply(live, result, buffer, &mut self.watcher, &self.name, &mut std::io::stderr());
            }
        }
        let changed = if self.compiling.is_none() && std::mem::take(&mut self.queued) {
            self.watcher.changed_now()
        } else {
            self.watcher.changed()
        };
        if !changed {
            return;
        }
        if self.compiling.is_some() {
            self.queued = true;
        } else {
            // The save being compiled no longer shows as a change while it compiles; one that
            // lands meanwhile does. Afterwards the watcher takes the times the compile itself read
            // the files at (`swap::apply`), so no save is missed and none is compiled twice.
            self.watcher.refresh();
            self.compiling = Some(compile::spawn(&self.job, self.watcher.paths(), self.watcher.latest_save()));
        }
    }
}
