//! The compile step of the reloading host: what `kestrel run` does from reading the entry file to
//! `prove`, as one call that a compile thread can make while the game keeps running.

use std::cell::RefCell;
use std::path::Path;

use crate::host::without_generated_main;
use crate::{compile_reading, prove, without_bom, Compiled, DiagStyle, SourceFile};

/// A compile that failed: the text `kestrel run` would print, and every file it tried to read with
/// the time taken just before reading it (`None` for a file that doesn't exist yet). The host
/// watches these files too, so fixing a module that only the broken edit `use`s (or creating it)
/// is seen.
#[derive(Debug)]
pub struct CompileFailure {
    pub text: String,
    pub files: Vec<SourceFile>,
}

/// Reads the program whose entry file is `path`, compiles it as `host::compile_as_host` does (a
/// reloadable program's module comes without the generated `main`) and proves its contracts for a
/// checked build, or a `--release` one with `release`. On success it gives the program and the
/// warnings `kestrel run` would print for it (empty when there are none), for the host to print
/// before `reloaded`. On failure the error has the text `kestrel run` would print (the
/// diagnostics, or that the file can't be read) and the files it read.
///
/// Each file's time, in `Compiled::files` or in the failure, is read just before the file itself
/// is, so a save that lands while this runs makes `SourceFile::changed` true afterwards and is
/// never missed, and a save that finished before it is never seen twice.
///
/// Nothing here is kept per thread, so any thread may call it. Like `kestrel run`'s own thread,
/// the caller pins the float mode first (`kestrel_runtime::fpu::pin`), since Cranelift's constant
/// folding and the compiler's casts follow it, and gives itself a large stack for deep programs.
pub fn compile_for_reload(path: &Path, style: DiagStyle, release: bool) -> Result<(Compiled, String), CompileFailure> {
    let reads = Reads::default();
    reads.note(path);
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) => {
            let text = format!("kestrel: can't read {}: {e}\n", path.display());
            return Err(CompileFailure { text, files: reads.into_files() });
        }
    };
    let (compiled, text) = compile_reading(&path.to_string_lossy(), without_bom(&src), style, true, &|file: &Path| reads.read(file));
    let Some(mut compiled) = compiled else {
        return Err(CompileFailure { text, files: reads.into_files() });
    };
    without_generated_main(&mut compiled.module);
    kestrel_kir::optimize(&mut compiled.module);
    reads.stamp(&mut compiled.files);
    let proved = prove(&mut compiled, !release, false, style);
    if !proved.ok {
        return Err(CompileFailure { text: text + &proved.text, files: compiled.files });
    }
    Ok((compiled, text + &proved.text))
}

/// The files a compile read, in the order it read them, each with the time taken just before.
#[derive(Default)]
pub(crate) struct Reads(RefCell<Vec<SourceFile>>);

impl Reads {
    /// Reads a module file for the compiler (as the compiler's own reader does), taking its time
    /// first.
    pub(crate) fn read(&self, path: &Path) -> Option<String> {
        self.note(path);
        std::fs::read_to_string(path).ok().map(|text| without_bom(&text).to_string())
    }

    /// Takes the time of `path`, which is about to be read.
    pub(crate) fn note(&self, path: &Path) {
        let file = SourceFile::new(path);
        let mut files = self.0.borrow_mut();
        match files.iter_mut().find(|f| f.path == file.path) {
            Some(known) => known.modified = file.modified,
            None => files.push(file),
        }
    }

    /// Gives each of `files` that was read the time taken before its read.
    pub(crate) fn stamp(&self, files: &mut [SourceFile]) {
        let reads = self.0.borrow();
        for file in files {
            if let Some(read) = reads.iter().find(|r| r.path == file.path) {
                file.modified = read.modified;
            }
        }
    }

    pub(crate) fn into_files(self) -> Vec<SourceFile> {
        self.0.into_inner()
    }
}
