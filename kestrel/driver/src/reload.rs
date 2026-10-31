//! The compile step of the reloading host: what `kestrel run` does from reading the entry file to
//! `prove`, as one call that a compile thread can make while the game keeps running.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::host::without_generated_main;
use crate::{compile_reading, prove, without_bom, Compiled, DiagStyle};

/// Reads the program whose entry file is `path`, compiles it as `host::compile_as_host` does (a
/// reloadable program's module comes without the generated `main`) and proves its contracts for a
/// checked build, or a `--release` one with `release`. On failure the error is the text `kestrel run`
/// would print: the diagnostics, or that the file can't be read.
///
/// Each file's time in `Compiled::files` is read just before the file itself is, so a save that
/// lands while this runs makes `SourceFile::changed` true afterwards and is never missed.
///
/// Nothing here is kept per thread, so any thread may call it. Like `kestrel run`'s own thread,
/// the caller pins the float mode first (`kestrel_runtime::fpu::pin`), since Cranelift's constant
/// folding and the compiler's casts follow it, and gives itself a large stack for deep programs.
pub fn compile_for_reload(path: &Path, style: DiagStyle, release: bool) -> Result<Compiled, String> {
    let entry_time = modified(path);
    let src = std::fs::read_to_string(path).map_err(|e| format!("kestrel: can't read {}: {e}\n", path.display()))?;
    let times = RefCell::new(HashMap::<PathBuf, Option<SystemTime>>::new());
    let read = |file: &Path| {
        times.borrow_mut().insert(file.to_path_buf(), modified(file));
        std::fs::read_to_string(file).ok().map(|text| without_bom(&text).to_string())
    };
    let (compiled, text) = compile_reading(&path.to_string_lossy(), without_bom(&src), style, true, &read);
    let Some(mut compiled) = compiled else { return Err(text) };
    without_generated_main(&mut compiled.module);
    kestrel_kir::optimize(&mut compiled.module);
    let times = times.into_inner();
    for (i, file) in compiled.files.iter_mut().enumerate() {
        if i == 0 {
            file.modified = entry_time;
        } else if let Some(&time) = times.get(&file.path) {
            file.modified = time;
        }
    }
    let proved = prove(&mut compiled, !release, false, style);
    if !proved.ok {
        return Err(text + &proved.text);
    }
    Ok(compiled)
}

/// When `path` last changed, or `None` when it can't be read.
fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
