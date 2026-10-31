//! The compile the reloading host asks for (hot reload design, decision 1). Every other path runs a
//! reloadable program through the `main` lowering generates for it (`let s = setup()`, then
//! `while frame(var s) {}`); the host calls `setup` and `frame` through their export thunks
//! itself, so its module has no `main`.

use std::path::Path;

use super::{compile_reading, disk, without_bom, Compiled, DiagStyle};

/// Like `compile` (a program, not a library), for the reloading host: a reloadable program's module
/// comes without the generated `main` (`Module::main` is `None`). Any other program is compiled
/// as `compile` does it.
pub fn compile_as_host(path: &str, src: &str, style: DiagStyle) -> (Option<Compiled>, String) {
    let (compiled, text) = compile_reading(path, src, style, true, &disk);
    let compiled = compiled.map(|mut c| {
        without_generated_main(&mut c.module);
        kestrel_kir::optimize(&mut c.module);
        c
    });
    (compiled, text)
}

/// Reads `path` and compiles it with `compile_as_host`.
pub fn compile_file_as_host(path: &Path, style: DiagStyle) -> (Option<Compiled>, String) {
    match std::fs::read_to_string(path) {
        Ok(src) => compile_as_host(&path.to_string_lossy(), without_bom(&src), style),
        Err(e) => (None, format!("error: can't read {}: {e}\n", path.display())),
    }
}

/// Takes the generated `main` out of a reloadable program's module (one with `Module::reload`),
/// as lowering left it: before `kestrel_kir::optimize`, which may inline into it. Lowering adds it
/// last, so nothing else moves. Any other module is left alone.
pub fn without_generated_main(m: &mut kestrel_kir::Module) {
    if m.reload.is_none() {
        return;
    }
    let Some(main) = m.main.take() else { return };
    assert_eq!(main.0 as usize + 1, m.funcs.len(), "lowering adds the generated `main` last");
    assert!(m.not_inlined.is_empty() && m.analysed.is_none(), "the generated `main` is taken out before optimizing");
    m.funcs.pop();
}
