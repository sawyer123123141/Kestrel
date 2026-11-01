//! The code that runs and the state it runs on, and swapping in new code between two frames (hot
//! reload design, decisions 3 and 4): the same state shape keeps the state where it is; another
//! shape drops it and runs the new `setup`.

use std::alloc::Layout;
use std::io::Write;
use std::time::SystemTime;

use kestrel_codegen::jit::Thunk;
use kestrel_codegen::{ExportTy, JitProgram};
use kestrel_kir::ReloadInfo;

use super::compile::{Built, Outcome};
use super::watch::Watcher;

/// The memory the state lives in: `ReloadInfo::state_size` bytes, aligned for the state and to at
/// least 16 (a `vec4` field), zeroed to start with.
pub struct StateBuffer {
    ptr: *mut u8,
    layout: Layout,
}

impl StateBuffer {
    pub fn new(info: &ReloadInfo) -> StateBuffer {
        let align = (info.state_align as usize).max(16).next_power_of_two();
        let layout = Layout::from_size_align(info.state_size.max(1) as usize, align).expect("a state's size fits in memory");
        // SAFETY: the layout's size is at least one byte.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        StateBuffer { ptr, layout }
    }
}

impl Drop for StateBuffer {
    fn drop(&mut self) {
        // SAFETY: allocated in `new` with this layout.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) };
    }
}

/// The program that runs now: its `setup`, `frame` and `reload_drop` thunks and its state's
/// description. The programs it replaced are kept, never dropped, so their code and data (a string
/// literal the state still points at) stay mapped.
pub struct Live {
    program: JitProgram,
    pub info: ReloadInfo,
    /// The state type's name, for messages.
    pub state: String,
    setup: Thunk,
    frame: Thunk,
    drop: Thunk,
    retired: Vec<JitProgram>,
}

impl Live {
    /// The reloadable program `program`, whose module's reload information is `info`. Fails when
    /// an export the host calls is missing or has another shape, which is a compiler bug.
    pub fn new(program: JitProgram, info: ReloadInfo) -> Result<Live, String> {
        let exports = program.exports();
        let find = |name: &str| exports.iter().find(|(e, _)| e.name == name).ok_or_else(|| format!("the reloadable program has no `{name}` export"));
        let (setup, frame, drop) = (find("setup")?, find("frame")?, find("reload_drop")?);
        let Some(ExportTy::Other(state)) = setup.0.ret.clone() else {
            return Err("the reloadable program's `setup` doesn't return a struct".to_string());
        };
        let ok = setup.0.params.is_empty()
            && setup.0.ret_size == info.state_size
            && frame.0.ret == Some(ExportTy::Bool)
            && frame.0.arg_offsets == [0]
            && drop.0.arg_offsets == [0];
        if !ok {
            return Err("the reloadable program's `setup`, `frame` and `reload_drop` don't have the shapes the host calls".to_string());
        }
        let (setup, frame, drop) = (setup.1, frame.1, drop.1);
        Ok(Live { program, info, state, setup, frame, drop, retired: Vec::new() })
    }

    /// Builds the program's constants on this thread.
    pub fn init(&self) {
        self.program.run_init();
    }

    /// Runs `setup`, which writes a fresh state into `buffer`.
    pub fn setup(&self, buffer: &mut StateBuffer) {
        // SAFETY: `setup` takes no arguments and writes its result, `state_size` bytes, at `ret`;
        // the buffer has that many, aligned for the state.
        unsafe { (self.setup)(std::ptr::null(), buffer.ptr) };
    }

    /// Runs one `frame` on the state in `buffer`; false ends the program.
    pub fn frame(&self, buffer: &mut StateBuffer) -> bool {
        let mut more = 0u8;
        // SAFETY: `frame`'s one parameter is `var`, so the thunk passes the argument buffer itself
        // (offset 0) as the state's address: the state is the buffer. It writes a `bool` at `ret`.
        unsafe { (self.frame)(buffer.ptr, &mut more) };
        more != 0
    }

    /// Runs `reload_drop` on the state in `buffer`, which lets go of its GPU objects.
    fn drop_state(&self, buffer: &mut StateBuffer) {
        // SAFETY: as for `frame`; `reload_drop` gives no result.
        unsafe { (self.drop)(buffer.ptr, std::ptr::null_mut()) };
    }

    /// Runs `next` from now on, keeping this program mapped.
    fn replace(&mut self, next: Live) {
        let old = std::mem::replace(&mut self.program, next.program);
        self.retired.push(old);
        self.retired.extend(next.retired);
        (self.info, self.state, self.setup, self.frame, self.drop) = (next.info, next.state, next.setup, next.frame, next.drop);
    }
}

/// Takes in what a compile gave, between two frames: prints its warnings and swaps the new program
/// in, or prints the compile's errors and keeps the one that runs. `name` is the program file's
/// name. Every message goes to `out` (stderr).
pub fn apply(live: &mut Live, result: Outcome, buffer: &mut StateBuffer, watcher: &mut Watcher, name: &str, out: &mut dyn Write) {
    match result {
        Ok(built) => {
            // As the first compile prints them, before anything else is said about this one.
            let _ = write!(out, "{}", built.warnings);
            watcher.adopt(built.compiled.files.clone());
            swap(live, built, buffer, name, out);
        }
        // The watcher takes the times the compile read the files at, and watches every file it
        // tried (a module only this edit uses among them), so this is not tried again until the
        // next save, and a save to any of them is seen.
        Err(failure) => {
            watcher.merge(failure.files);
            let _ = write!(out, "{}", failure.text);
            let _ = writeln!(out, "still running the previous code; fix the error and save again");
        }
    }
    let _ = out.flush();
}

/// Swaps `built` in for `live`: the same state shape keeps the state in `buffer`; another drops it
/// and runs the new `setup` into a new buffer.
pub fn swap(live: &mut Live, built: Built, buffer: &mut StateBuffer, name: &str, out: &mut dyn Write) {
    let Built { program, compiled, started, saved, .. } = built;
    let Some(info) = compiled.module.reload.clone() else {
        let _ = writeln!(
            out,
            "{name} has no `setup` and `frame` now, so it can't be reloaded; still running the previous code; restart it to run the new one"
        );
        return;
    };
    let next = match Live::new(program, info) {
        Ok(next) => next,
        Err(e) => {
            let _ = writeln!(out, "kestrel: internal compiler error: {e}; still running the previous code");
            return;
        }
    };
    // The new code's first failure names no line of the old code's.
    kestrel_runtime::panic::reset_call_site();
    next.init();
    let same_shape = next.info.state_description == live.info.state_description;
    let setup_changed = next.info.setup_hash != live.info.setup_hash;
    if same_shape {
        live.replace(next);
    } else {
        live.drop_state(buffer);
        *buffer = StateBuffer::new(&next.info);
        live.replace(next);
        live.setup(buffer);
    }
    // From the save to now; from the compile's start when the file's time is in the future.
    let took = saved.and_then(|t| SystemTime::now().duration_since(t).ok()).unwrap_or_else(|| started.elapsed());
    let _ = writeln!(out, "reloaded {name} in {:.2} s", took.as_secs_f64());
    if !same_shape {
        let _ = writeln!(out, "state reset: {} changed shape; setup() ran again", live.state);
    } else if setup_changed {
        let _ = writeln!(out, "setup() changed but the state was kept; restart to apply it");
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use kestrel_codegen::{JitProgram, Options};
    use kestrel_driver::{compile_for_reload, DiagStyle};

    use super::super::compile::Built;
    use super::super::watch::Watcher;
    use super::{apply, Live, StateBuffer};

    const PROGRAM: &str = "struct Game {\n    n: int\n}\npub setup() -> Game { Game(n: 0) }\npub frame(var s: Game) -> bool {\n    s.n += 1\n    s.n < 3\n}\n";

    /// Compiles the program at `path` as the compile thread does, with `warnings` in place of the
    /// compile's own.
    fn built(path: &Path, warnings: &str) -> Built {
        let (compiled, none) = compile_for_reload(path, DiagStyle::Short, false).unwrap();
        assert_eq!(none, "", "the program compiles clean");
        let program = JitProgram::compile_with(&compiled.module, Options::default(), &crate::natives()).unwrap();
        Built { program, compiled, warnings: warnings.to_string(), started: Instant::now(), saved: None }
    }

    #[test]
    fn a_reload_prints_its_warnings_before_reloaded() {
        // Only `kestrel check` warns today (an `@assume` that removes no check), so no program
        // `kestrel run` compiles has a warning: this one stands in for the compile's own.
        let warning = "1:1: warning: a stand-in warning\n";
        let dir = std::env::temp_dir().join(format!("kestrel-swap-warnings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prog.kes");
        std::fs::write(&path, PROGRAM).unwrap();
        let first = built(&path, "");
        let mut live = Live::new(first.program, first.compiled.module.reload.clone().unwrap()).unwrap();
        live.init();
        let mut buffer = StateBuffer::new(&live.info);
        live.setup(&mut buffer);
        assert!(live.frame(&mut buffer));
        let mut watcher = Watcher::new(first.compiled.files.clone());
        let mut out = Vec::new();
        apply(&mut live, Ok(built(&path, warning)), &mut buffer, &mut watcher, "prog.kes", &mut out);
        let out = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.len() == 2 && lines[0] == warning.trim_end() && lines[1].starts_with("reloaded prog.kes in "), "{out}");
        // The state was kept: the second frame of three still runs, the third ends it.
        assert!(live.frame(&mut buffer));
        assert!(!live.frame(&mut buffer));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
