mod dump;
pub mod export;
mod func;
mod globals;
mod isa;
mod loop_exits;
mod native;
mod par_thunk;
mod phis;
pub mod jit;
pub mod object;
mod rt;
mod tables;

use cranelift_codegen::ir::Function;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_module::{FuncId, Linkage, Module};
use kestrel_kir as kir;

#[doc(hidden)]
pub use dump::{dump, dump_lowered};
pub use export::{ExportInfo, ExportTy};
pub use isa::TARGET_CPUS;
pub use kestrel_runtime::fpu::{has_sse41, NO_SSE41};
pub use jit::JitProgram;
pub use object::emit_object;
#[doc(hidden)]
pub use object::emit_object_for;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Panic on integer overflow (exit 101). Off for `--release`.
    pub checked_arith: bool,
    /// Run range analysis (`kestrel_kir::range`) on a copy of the module and leave out the checks
    /// it proves can't fail. Off, every check stays (the analysis's own tests compare the two),
    /// apart from any a caller marked proven itself. A module analysed already (`Module::analysed`,
    /// set by the driver for an annotated program) is compiled as it is: it must have been
    /// analysed for this build, and this must be on.
    pub remove_checks: bool,
    /// Keep every proven check, but make its failure stop the program with "range analysis
    /// removed the ... check ..., but it fails; this is a compiler bug" (`KESTREL_CHECK_PROOFS=1`).
    pub verify_proofs: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { checked_arith: true, remove_checks: true, verify_proofs: false }
    }
}

/// Symbol name of Kestrel function `index`, named `name`, kept apart from C names like `main`. A
/// copy's name (`Pair[int, bool].swap`) has characters a symbol can't: each outside
/// `[A-Za-z0-9_.]` becomes `_`, and `.<index>` is added so two copies can't collide (P31). No
/// plain name ends in `.` and digits (a name never starts with a digit), so neither can a copy
/// and a plain function. Every other name keeps `k_<name>`.
pub(crate) fn symbol(name: &str, index: usize) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.';
    if name.chars().all(plain) {
        return format!("k_{name}");
    }
    let sanitised: String = name.chars().map(|c| if plain(c) { c } else { '_' }).collect();
    format!("k_{sanitised}.{index}")
}

/// Declares every function of `m` in `module` and defines the ones with bodies
/// (natives are imports under their own symbols), and defines its globals. Returns
/// the functions' ids, indexed by `FuncIdx`. With `dump`, the named function's code is
/// recorded in it as it is compiled.
pub(crate) fn define_module<M: Module>(
    module: &mut M,
    m: &kir::Module,
    opts: Options,
    mut dump: Option<&mut dump::Dump>,
) -> Result<Vec<FuncId>, String> {
    isa::check_simd(module.isa(), m)?;
    // Range analysis runs here, on a copy, with this build's arithmetic: so every way of compiling
    // gets it, and the proofs always assume the mode of the code emitted. A module the driver
    // analysed already (an annotated program) is copied as it is, for its own build only. The
    // passes that need the marks (`kir::after_marking`) then run on that copy, so the caller's
    // module, which `kestrel check --report` and contracts read, never sees them.
    let mode = kir::range::Arith::of(opts.checked_arith);
    let analysed;
    let m = match (m.analysed, opts.remove_checks) {
        (Some(_), false) => return Err("the module was analysed, so it can't be compiled with every check kept".to_string()),
        (Some(built), true) if built != mode => return Err("the module was analysed for the other build".to_string()),
        (None, false) => m,
        (Some(_), true) => {
            let mut copy = m.clone();
            kir::after_marking(&mut copy, mode);
            analysed = copy;
            &analysed
        }
        (None, true) => {
            let mut copy = m.clone();
            // What is left isn't needed here: `kestrel check --report` describes it.
            kir::range::mark_proven(&mut copy, mode);
            kir::after_marking(&mut copy, mode);
            analysed = copy;
            &analysed
        }
    };
    let cc = module.isa().default_call_conv();
    let ptr = module.target_config().pointer_type();
    let rt = rt::RtFuncs::declare(module)?;
    let mut ids = Vec::with_capacity(m.funcs.len());
    for (i, f) in m.funcs.iter().enumerate() {
        let id = match &f.native {
            Some(import) => module.declare_function(import, Linkage::Import, &native::signature(f, cc, ptr)),
            None => module.declare_function(&symbol(&f.name, i), Linkage::Local, &func::signature(f, cc, ptr)),
        };
        ids.push(id.map_err(|e| e.to_string())?);
    }
    let par_thunks = par_thunk::define_par_thunks(module, m, &ids)?;
    let globals = globals::define_globals(module, m)?;
    let files = globals::define_file_names(module, m)?;
    // The tables hold the functions' addresses, so they come after the functions are declared.
    let tables = tables::define_tables(module, m, &ids)?;
    let shared = func::Shared { funcs: &ids, par_thunks: &par_thunks, rt: &rt, globals: &globals, files: &files, tables: &tables, opts, kir: m };
    let mut strings = func::Strings::default();
    let mut ctx = module.make_context();
    let mut fb_ctx = FunctionBuilderContext::new();
    for (f, &id) in m.funcs.iter().zip(&ids).filter(|(f, _)| f.native.is_none()) {
        ctx.func = Function::with_name_signature(Default::default(), func::signature(f, cc, ptr));
        func::lower_func(module, &mut strings, &shared, &mut ctx.func, &mut fb_ctx, f)?;
        let dumping = dump.as_ref().is_some_and(|d| d.name == f.name);
        ctx.set_disasm(dumping);
        if let Some(d) = dump.as_deref_mut().filter(|_| dumping) {
            d.record_input(&ctx.func);
        }
        module
            .define_function(id, &mut ctx)
            .map_err(|e| format!("codegen failed for `{}`: {e:?}", f.name))?;
        if let Some(d) = dump.as_deref_mut().filter(|_| dumping) {
            d.record(&ctx);
        }
        module.clear_context(&mut ctx);
    }
    Ok(ids)
}
