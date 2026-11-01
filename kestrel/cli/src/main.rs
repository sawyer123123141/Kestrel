mod link;
mod reload;
mod replay_cmd;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kestrel_driver::{without_bom, DiagStyle};
use kestrel_codegen::{emit_object, has_sse41, JitProgram, Options, NO_SSE41, TARGET_CPUS};
use link::Lib;

const USAGE: &str = "usage: kestrel <run|check|build> <file.kes> [-o <exe>] [--release] [--report] [--diagnostics=short]
       kestrel run <file.kes> [--release] [--no-reload] [--record <file.replay>] [<args>...] [-- <args>...]
       kestrel replay <file.kes> <file.replay> [--release]";

/// What `--report` on anything but `check` says.
const REPORT_NEEDS_CHECK: &str = "`--report` goes with `check`: kestrel check --report file.kes";

/// Turns any Rust panic in the compiler into exit 3, keeping 101 for Kestrel runtime panics. A
/// panic while hot reload compiles a save is printed and not fatal: the compile thread catches it
/// and the game keeps running the previous code.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("kestrel: internal compiler error: {info}");
        if !survives_panic(std::thread::current().name()) {
            std::process::exit(3);
        }
    }));
}

/// Whether a panic on the thread named `thread` is caught instead of ending the process: only on
/// hot reload's compile thread.
fn survives_panic(thread: Option<&str>) -> bool {
    thread == Some(reload::COMPILE_THREAD)
}

struct Args {
    cmd: String,
    path: String,
    out: Option<String>,
    release: bool,
    report: bool,
    style: DiagStyle,
    /// The words `run` passes to the program (`args.all()`): every word after the file that is not
    /// one of the compiler's flags, and every word after `--`.
    program_args: Vec<String>,
    /// `run --record <file.replay>`: where to write the recording.
    record: Option<String>,
    /// `replay <file.kes> <file.replay>`: the recording to replay.
    replay: Option<String>,
    /// `run --no-reload`: a reloadable program runs without watching its files.
    no_reload: bool,
}

fn parse_args(raw: Vec<String>) -> Option<Args> {
    let mut it = raw.into_iter();
    let (mut positional, mut out, mut release, mut report, mut style) = (Vec::new(), None, false, false, DiagStyle::Full);
    let (mut program_args, mut record, mut no_reload) = (Vec::new(), None, false);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => program_args.extend(it.by_ref()),
            "-o" => out = Some(it.next()?),
            "--record" => record = Some(it.next()?),
            "--release" => release = true,
            "--no-reload" => no_reload = true,
            "--report" => report = true,
            "--diagnostics=short" => style = DiagStyle::Short,
            s if s.starts_with('-') => return None,
            // The command, then the file (and for `replay` the recording); any later word is the program's.
            _ if positional.len() == 2 && positional[0] != "replay" => program_args.push(a),
            _ => positional.push(a),
        }
    }
    let replay = if positional.first().is_some_and(|c| c == "replay") { Some(positional.pop()?) } else { None };
    let [cmd, path]: [String; 2] = positional.try_into().ok()?;
    // Only `run` starts the program with words of its own; a replay has the recorded ones.
    if cmd != "run" && (!program_args.is_empty() || record.is_some() || no_reload) {
        return None;
    }
    // A replay runs the program, so it takes only `--release` of the compiler's flags.
    if cmd == "replay" && (out.is_some() || report) {
        return None;
    }
    let known = matches!(cmd.as_str(), "run" | "check" | "build" | "replay");
    known.then_some(Args { cmd, path, out, release, report, style, program_args, record, replay, no_reload })
}

/// The compiler recurses over the syntax tree, so it runs on a stack far bigger than the
/// platform default.
const STACK_SIZE: usize = 256 * 1024 * 1024;

fn main() -> ExitCode {
    install_panic_hook();
    match std::thread::Builder::new().stack_size(STACK_SIZE).spawn(run) {
        Ok(worker) => worker.join().unwrap_or(ExitCode::from(3)),
        // Couldn't reserve the stack; the default one still handles ordinary programs.
        Err(_) => run(),
    }
}

/// True when both paths name the same file (compared canonically when both exist).
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn run() -> ExitCode {
    // Cranelift's constant folding and the compiler's own `as f32` casts follow this thread's
    // float mode, so it gets the same pinned mode a program runs in.
    kestrel_runtime::fpu::pin();
    let Some(args) = parse_args(std::env::args().skip(1).collect()) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if args.report && args.cmd != "check" {
        eprintln!("{REPORT_NEEDS_CHECK}");
        return ExitCode::from(2);
    }
    // A replay reads its file first: one it can't replay stops it before anything is compiled.
    let replaying = match &args.replay {
        Some(file) => match replay_cmd::read(file, &args) {
            Ok(read) => Some(read),
            Err(e) => {
                eprintln!("kestrel: {e}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    // Taken before the file is read, so hot reload sees a save made while this compiles.
    let entry_time = std::fs::metadata(&args.path).and_then(|m| m.modified()).ok();
    let src = match std::fs::read_to_string(&args.path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("kestrel: can't read {}: {e}", args.path);
            return ExitCode::from(2);
        }
    };
    let src = without_bom(&src);
    let output = match args.cmd.as_str() {
        "build" => check_output(&args),
        _ => check_record(&args),
    };
    if let Err(e) = output {
        eprintln!("kestrel: {e}");
        return ExitCode::from(2);
    }
    // Running a reloadable program calls its `setup` and `frame` itself, so it needs no `main`.
    let (compiled, diagnostics) = match args.cmd.as_str() {
        "run" | "replay" => kestrel_driver::host::compile_as_host(&args.path, src, args.style),
        _ => kestrel_driver::compile(&args.path, src, args.style),
    };
    eprint!("{diagnostics}");
    let Some(mut compiled) = compiled else { return ExitCode::from(1) };
    if let Some(entry) = compiled.files.first_mut() {
        entry.modified = entry_time;
    }
    // An annotated program is analysed here, once, for this build; only `check` warns.
    let checking = args.cmd == "check";
    let proved = kestrel_driver::prove(&mut compiled, !args.release, checking, args.style);
    eprint!("{}", proved.text);
    if checking {
        // The report still prints when a contract fails: it is how to see why.
        if args.report {
            print_report(compiled, !args.release, proved.analysis);
        }
        return if proved.ok { ExitCode::SUCCESS } else { ExitCode::from(1) };
    }
    if !proved.ok {
        return ExitCode::from(1);
    }
    if let Err(e) = has_window_for(&compiled.builtins) {
        eprintln!("kestrel: {e}");
        return ExitCode::from(2);
    }
    let opts = Options {
        checked_arith: !args.release,
        verify_proofs: proofs_checked(std::env::var_os("KESTREL_CHECK_PROOFS").as_deref()),
        ..Options::default()
    };
    let result = match args.cmd.as_str() {
        "run" | "replay" if !has_sse41() => Err(NO_SSE41.to_string()),
        "run" | "replay" => {
            // The program's relative paths (`file`, `window.screenshot`, textures) are read from its own folder.
            kestrel_runtime::natives::paths::set_program_dir(Path::new(&args.path).parent().unwrap_or(Path::new("")));
            // `args.program()` is the `.kes` path, and `args.all()` the words meant for the program.
            kestrel_runtime::start::set_args(args.path.clone(), args.program_args.clone());
            JitProgram::compile_with(&compiled.module, opts, &natives()).map_err(internal_error).and_then(|p| {
                let reloadable = compiled.module.reload.is_some();
                let job = reload::job(&args, opts, reloadable);
                // Recording or replaying starts with the program, once it is compiled.
                replay_cmd::start(&args, replaying);
                let code = if reloadable { reload::run(p, compiled, job)? } else { p.run_main() };
                replay_cmd::finish(code)
            })
        }
        _ => {
            build(&compiled.module, opts, &args, lib_for(&compiled.builtins)).map(|()| 0)
        }
    };
    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("kestrel: {e}");
            ExitCode::from(3)
        }
    }
}

/// The natives a program is compiled with: the window crate's, then the runtime's own (`file`,
/// `time`, `args`, `env`).
#[cfg(windows)]
fn natives() -> Vec<(&'static str, *const u8)> {
    kestrel_window::symbols().into_iter().chain(kestrel_runtime::natives::native_symbols()).collect()
}

/// The natives a program is compiled with: the runtime's own (`file`, `time`, `args`, `env`).
/// Only Windows builds have the window crate's.
#[cfg(not(windows))]
fn natives() -> Vec<(&'static str, *const u8)> {
    kestrel_runtime::natives::native_symbols()
}

/// Whether this build of `kestrel` has the window, drawing and input natives (raylib). Only the
/// Windows build has them for now.
const HAS_WINDOW: bool = cfg!(windows);

/// What `run`, `replay` and `build` say about a window program when this build has no window.
pub(crate) const NO_WINDOW: &str = "this build of kestrel has no window, so it can't run or build a program that uses `window`, `draw`, `draw3d` or `input`; only the Windows build has them for now";

/// Refuses a program that uses `window`, `draw`, `draw3d` or `input` (its built-in modules are
/// `builtins`) when this build of `kestrel` has no window.
fn has_window_for(builtins: &[&str]) -> Result<(), String> {
    window_check(HAS_WINDOW, builtins)
}

fn window_check(has_window: bool, builtins: &[&str]) -> Result<(), String> {
    if !has_window && builtins.iter().any(|name| kestrel_driver::needs_window(name)) {
        Err(NO_WINDOW.to_string())
    } else {
        Ok(())
    }
}

/// Whether `KESTREL_CHECK_PROOFS=1` is set: codegen then keeps every check range analysis proved,
/// and a failing one stops the program with a message that names the compiler bug.
fn proofs_checked(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v == "1")
}

/// Prints what range analysis leaves in the program for `kestrel check --report`, in the checks
/// of a checked build or of a `--release` build (`checked`). `analysis` is the driver's, of the
/// module as it is now, when it analysed it already (for that build).
fn print_report(mut compiled: kestrel_driver::Compiled, checked: bool, analysis: Option<kestrel_kir::range::Analysis>) {
    use kestrel_kir::range::{remove_checks, render::render, report::build, Arith};
    let analysis = analysis.unwrap_or_else(|| remove_checks(&mut compiled.module, Arith::of(checked)));
    let report = build(&compiled.module, &analysis);
    let files = &compiled.files;
    let text = render(&report, &compiled.module, &|i| files.get(i as usize).is_none_or(|f| !f.is_builtin()));
    print!("{text}");
}

fn internal_error(e: String) -> String {
    format!("internal compiler error: {e}")
}

/// Output path: `-o`, else `<source stem><EXE_SUFFIX>` in the current directory.
fn output_path(args: &Args) -> Result<PathBuf, String> {
    if let Some(o) = &args.out {
        return Ok(PathBuf::from(o));
    }
    let stem = Path::new(&args.path).file_stem().ok_or("source path has no file name")?;
    Ok(PathBuf::from(format!("{}{}", stem.to_string_lossy(), std::env::consts::EXE_SUFFIX)))
}

/// Refuses a `build` whose executable would overwrite the source file.
fn check_output(args: &Args) -> Result<(), String> {
    let exe = output_path(args)?;
    if same_file(&exe, Path::new(&args.path)) {
        return Err(format!("the output {} is the source file; pick another with -o", exe.display()));
    }
    Ok(())
}

/// Refuses a `run --record` whose recording would overwrite the program.
fn check_record(args: &Args) -> Result<(), String> {
    match &args.record {
        Some(record) if same_file(Path::new(record), Path::new(&args.path)) => {
            Err("`--record` would overwrite the program; give it another file name".to_string())
        }
        _ => Ok(()),
    }
}

/// The library a program is linked with: programs that use `window`, `draw`, `draw3d` or `input`
/// need the window library, which carries the runtime as well; the rest (`random` is plain
/// Kestrel) need the runtime alone.
fn lib_for(builtins: &[&str]) -> Lib {
    if builtins.iter().any(|name| kestrel_driver::needs_window(name)) {
        Lib::Window
    } else {
        Lib::Runtime
    }
}

fn build(module: &kestrel_kir::Module, opts: Options, args: &Args, lib: Lib) -> Result<(), String> {
    let exe = output_path(args)?;
    let bytes = emit_object(module, opts).map_err(internal_error)?;
    let obj = std::env::temp_dir().join(format!("kestrel-{}.o", std::process::id()));
    std::fs::write(&obj, bytes).map_err(|e| format!("can't write {}: {e}", obj.display()))?;
    let linked = link::link(&obj, &exe, lib);
    let _ = std::fs::remove_file(&obj);
    linked?;
    println!("built {} for {TARGET_CPUS}", exe.display());
    Ok(())
}

#[cfg(test)]
#[path = "main_test.rs"]
mod tests;
