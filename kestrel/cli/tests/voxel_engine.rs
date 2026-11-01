//! The voxel engine's tests (`kestrel/voxel`). The engine is a flat folder of modules, and Kestrel
//! finds a module only next to the file that uses it, so each test program in
//! `kestrel/voxel/tests` runs in a scratch folder holding copies of every `kestrel/voxel/*.kes`
//! plus the test, written there as `test_<name>.kes` (a module's name would clash with the engine
//! file of that name, and `main.kes` is the engine's play program).
//!
//! The directives are the program tests' (`common/directives.rs`): `// expect:`,
//! `// expect-exit:`, `// expect-stderr:` and `// env:`, and these for the engine:
//! - `// copy: <repo-relative file>` copies another file into the scratch folder;
//! - `// replace: <file> <old> => <new>` replaces literal text in that scratch copy only, before
//!   the run (`file` is a file of the scratch folder);
//! - `// release` also runs the program with `kestrel run --release`;
//! - `// only-jit` skips the executable run (the file says why it is too slow).
//!
//! A build or check that exits non-zero is compared like a run, so a program that must fail to
//! compile (a contract probe) says `// expect-exit: 1` and `// expect-stderr: <the message>`.
//! `KESTREL_TEST_ONLY=a,b` runs only the tests whose file names start with `a` or `b`
//! (`main_smoke` names the play program's smoke test, which is written here, not in a file).
//!
//! Every run and build is stopped after `RUN_TIMEOUT` and counted as a failure, so a test that
//! hangs fails instead of hanging the suite. The play program under hot reload is tested in
//! `voxel_engine_reload.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

mod common;

use common::directives::{compare, only_prefixed, parse, Directives};
use common::{hermetic, output_within};

/// What a contract's error says; see `release_runs_match_their_expectations`.
const CONTRACT_FAILED: &str = "can't be proven";

/// How long any pass lets one program build or run; a run over it is stopped and fails the test.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

fn voxel_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../voxel")
}

/// The repository's root, which `// copy:` paths are relative to.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `.kes` files directly in `dir`, sorted.
fn kes_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "kes"))
        .collect();
    files.sort();
    files
}

/// Every test in `kestrel/voxel/tests`, or with `KESTREL_TEST_ONLY` only the ones that match.
/// A filter that matches only `main_smoke` (the smoke test below) gives none, not a failure.
fn tests() -> Vec<PathBuf> {
    let dir = voxel_dir().join("tests");
    let files = kes_files(&dir);
    assert!(!files.is_empty(), "no .kes tests in {}", dir.display());
    let only = std::env::var("KESTREL_TEST_ONLY").ok();
    let files = only_prefixed(files, only.as_deref());
    assert!(
        !files.is_empty() || smoke_selected(only.as_deref()),
        "KESTREL_TEST_ONLY={} matches no test in {} (nor main_smoke)",
        only.unwrap_or_default(),
        dir.display()
    );
    files
}

/// Whether `KESTREL_TEST_ONLY` (here `only`) selects the play program's smoke test.
fn smoke_selected(only: Option<&str>) -> bool {
    !only_prefixed(vec![PathBuf::from("main_smoke")], only).is_empty()
}

/// Reads a test's directives. The program tests' `// asset:` and the proof-run expectations
/// don't apply here, and a test that used them would silently check less than it says.
fn expectations(name: &str, src: &str) -> Directives {
    let exp = parse(name, src);
    assert!(exp.assets.is_empty(), "{name}: `// asset:` is not supported in the engine's tests");
    assert!(
        exp.exit_proofs.is_none() && exp.stderr_proofs.is_empty(),
        "{name}: `// expect-exit-proofs:` and `// expect-stderr-proofs:` are not supported in the engine's tests"
    );
    exp
}

/// What the test is called, `grid` for `tests/grid.kes`.
fn stem(path: &Path) -> String {
    path.file_stem().unwrap().to_string_lossy().into_owned()
}

/// A fresh scratch folder for the test in `path`, holding copies of the engine's `.kes` files,
/// the files its `// copy:` lines name, and the test itself as `test_<name>.kes`, with its
/// `// replace:` lines done. `pass` ("" for the JIT pass) keeps the passes, which run as separate
/// tests in parallel, out of each other's folders. Gives the path of the test file.
fn prepare(pass: &str, path: &Path, exp: &Directives) -> PathBuf {
    let name = stem(path);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("voxel_{name}{pass}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for module in kes_files(&voxel_dir()) {
        fs::copy(&module, dir.join(module.file_name().unwrap())).unwrap_or_else(|e| panic!("copy {}: {e}", module.display()));
    }
    for copy in &exp.copies {
        let from = repo_root().join(copy);
        let to = dir.join(from.file_name().unwrap());
        assert!(
            !to.exists() && to != dir.join(format!("test_{name}.kes")),
            "{name}: `// copy: {copy}` would overwrite {} in the scratch folder, an engine module or the test itself; rename the copy's source",
            to.file_name().unwrap().to_string_lossy()
        );
        fs::copy(&from, &to).unwrap_or_else(|e| panic!("{name}: can't copy {}: {e}", from.display()));
    }
    let file = dir.join(format!("test_{name}.kes"));
    fs::copy(path, &file).unwrap();
    for r in &exp.replaces {
        let target = dir.join(&r.file);
        let text = fs::read_to_string(&target).unwrap_or_else(|e| panic!("{name}: can't replace in {}: {e}", target.display()));
        assert!(text.contains(&r.old), "{name}: `{}` is not in {}, so `// replace:` would change nothing", r.old, r.file);
        fs::write(&target, text.replace(&r.old, &r.new)).unwrap();
    }
    file
}

/// `kestrel run`, with the proof check on and the test's `// env:` variables. `None` when it ran
/// over `RUN_TIMEOUT` and was stopped.
fn kestrel_run(file: &Path, exp: &Directives) -> Option<Output> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(cmd.args(["run", file.to_str().unwrap()])).envs(exp.env.iter().map(|(k, v)| (k, v))).env("KESTREL_CHECK_PROOFS", "1");
    output_within(&mut cmd, RUN_TIMEOUT)
}

/// `kestrel build` of the file into `exe`, with the proof check on. `None` when it ran over
/// `RUN_TIMEOUT` and was stopped.
fn kestrel_build(file: &Path, exe: &Path) -> Option<Output> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(cmd.args(["build", file.to_str().unwrap(), "-o", exe.to_str().unwrap()])).env("KESTREL_CHECK_PROOFS", "1");
    output_within(&mut cmd, RUN_TIMEOUT)
}

/// The failure line for a run or build of `name` in pass `mode` that was stopped.
fn timed_out(name: &str, mode: &str) -> String {
    format!("{name} [{mode}]: still running after {} seconds, so it was stopped", RUN_TIMEOUT.as_secs())
}

#[test]
fn engine_tests_run_via_jit() {
    let mut failures = Vec::new();
    for path in tests() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let exp = expectations(&name, &fs::read_to_string(&path).unwrap());
        let file = prepare("", &path, &exp);
        match kestrel_run(&file, &exp) {
            Some(out) => failures.extend(compare(&name, "jit", &out, &exp)),
            None => failures.push(timed_out(&name, "jit")),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn engine_tests_run_as_executables() {
    let mut failures = Vec::new();
    for path in tests() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let exp = expectations(&name, &fs::read_to_string(&path).unwrap());
        if exp.only_jit {
            continue;
        }
        let file = prepare("_exe", &path, &exp);
        let exe = file.with_extension(std::env::consts::EXE_EXTENSION);
        let Some(built) = kestrel_build(&file, &exe) else {
            failures.push(timed_out(&name, "exe build"));
            continue;
        };
        // A build that fails (a type error, a contract that can't be proven) is compared with
        // the expected exit code and messages, as a run is; it isn't a harness failure.
        if !built.status.success() {
            failures.extend(compare(&name, "exe build", &built, &exp));
            continue;
        }
        let mut cmd = Command::new(&exe);
        hermetic(&mut cmd).envs(exp.env.iter().map(|(k, v)| (k, v))).env("KESTREL_CHECK_PROOFS", "1");
        let Some(out) = output_within(&mut cmd, RUN_TIMEOUT) else {
            failures.push(timed_out(&name, "exe"));
            continue;
        };
        failures.extend(compare(&name, "exe", &out, &exp));
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The release pass: the tests that say `// release`, run with `kestrel run --release` and the
/// proof check on, compared like the other passes (standard output, exit code and messages
/// against the test's directives; a failed proof always fails). A test that says `// release`
/// therefore has output that is the same in the checked and the release build (no overflow that
/// only stops a checked run). A contract's error the test doesn't expect also fails, and so does
/// a run over `RUN_TIMEOUT`, which is killed.
#[test]
fn release_runs_match_their_expectations() {
    let mut failures = Vec::new();
    for path in tests() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let exp = expectations(&name, &fs::read_to_string(&path).unwrap());
        if !exp.release {
            continue;
        }
        let file = prepare("_release", &path, &exp);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
        hermetic(cmd.args(["run", "--release", file.to_str().unwrap()]))
            .envs(exp.env.iter().map(|(k, v)| (k, v)))
            .env("KESTREL_CHECK_PROOFS", "1");
        let Some(out) = output_within(&mut cmd, RUN_TIMEOUT) else {
            failures.push(timed_out(&name, "release"));
            continue;
        };
        failures.extend(compare(&name, "release", &out, &exp));
        let stderr = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
        if stderr.contains(CONTRACT_FAILED) && !exp.stderr.iter().any(|s| s.contains(CONTRACT_FAILED)) {
            failures.push(format!("{name} [release]: a contract doesn't hold\n--- stderr ---\n{stderr}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// How many headless frames the play program's smoke run lasts: 2 seconds of the headless
/// clock. Kept low because `hermetic`'s `KESTREL_POISON=1` never reuses freed memory, so every
/// frame's meshing arena stays allocated.
const SMOKE_FRAMES: &str = "120";

/// The play program, `main.kes`, run in a quiet headless window for `SMOKE_FRAMES` frames with
/// `kestrel run` and `kestrel run --release`: it exits 0 (the headless window closes after its
/// frames; `main.kes` has no exit of its own) and prints a stats line once a second, in the
/// documented shape, with columns and chunks above 0, and no contract fails. A test file can't run another program's
/// `main`, so this one is written here. `KESTREL_TEST_ONLY` runs it when a prefix matches
/// `main_smoke`.
#[test]
fn main_smoke_runs_the_play_program_headless() {
    if !smoke_selected(std::env::var("KESTREL_TEST_ONLY").ok().as_deref()) {
        return;
    }
    let mut failures = Vec::new();
    for (pass, flags) in [("", &[][..]), ("_release", &["--release"][..])] {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("voxel_main_smoke{pass}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for module in kes_files(&voxel_dir()) {
            fs::copy(&module, dir.join(module.file_name().unwrap())).unwrap();
        }
        let main = dir.join("main.kes");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
        hermetic(cmd.arg("run").args(flags).arg(&main))
            .env("KESTREL_HEADLESS", SMOKE_FRAMES)
            .env("KESTREL_HEADLESS_QUIET", "1")
            .env("KESTREL_CHECK_PROOFS", "1");
        let Some(out) = output_within(&mut cmd, RUN_TIMEOUT) else {
            failures.push(timed_out("main.kes", pass));
            continue;
        };
        let stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        let stderr = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
        let lines: Vec<&str> = stdout.lines().filter(|l| l.starts_with("voxel fps ")).collect();
        let loaded = lines.iter().any(|l| stat(l, "columns").is_some_and(|n| n > 0.0) && stat(l, "chunks").is_some_and(|n| n > 0.0));
        let shaped = lines.iter().all(|l| stats_shaped(l));
        // The proof check is on, so a contract that doesn't hold in this build says so on
        // standard error, as in the release pass.
        let contracts_hold = !stderr.contains(CONTRACT_FAILED);
        if !out.status.success() || lines.is_empty() || !loaded || !shaped || !contracts_hold {
            failures.push(format!(
                "main.kes [{pass}]: exit {:?}, {} stats lines, loaded {loaded}, shaped {shaped}, contracts hold {contracts_hold}\n--- stdout ---\n{stdout}--- stderr ---\n{stderr}",
                out.status.code(),
                lines.len()
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The number after `key` in a stats line, if it is there.
fn stat(line: &str, key: &str) -> Option<f64> {
    let words: Vec<&str> = line.split_whitespace().collect();
    words.windows(2).find(|w| w[0] == key).and_then(|w| w[1].parse().ok())
}

/// Whether a stats line is `voxel fps <f> work_ms <f> columns <n> chunks <n> faces <n> queue <n>
/// mode <walk|fly>`.
fn stats_shaped(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    let keys = ["fps", "work_ms", "columns", "chunks", "faces", "queue"];
    words.len() == 15
        && words[0] == "voxel"
        && keys.iter().enumerate().all(|(k, key)| words[1 + 2 * k] == *key && words[2 + 2 * k].parse::<f64>().is_ok())
        && words[13] == "mode"
        && (words[14] == "walk" || words[14] == "fly")
}

#[test]
fn the_engine_directives_are_read() {
    let src = "// copy: kestrel/bench/voxels3d/voxels3d.kes\n// replace: voxels3d.kes fn density => pub fn density\n\
               // replace: gen.kes SIZE = 32 => SIZE = 4\n// replace: gen.kes  gone =>\n// release\n// only-jit\n// expect: hi\nmain() {}\n";
    let exp = expectations("d.kes", src);
    assert_eq!(exp.copies, ["kestrel/bench/voxels3d/voxels3d.kes"]);
    let replaces: Vec<(&str, &str, &str)> = exp.replaces.iter().map(|r| (&*r.file, &*r.old, &*r.new)).collect();
    let expected = [("voxels3d.kes", "fn density", "pub fn density"), ("gen.kes", "SIZE = 32", "SIZE = 4"), ("gen.kes", "gone", "")];
    assert_eq!(replaces, expected);
    assert!(exp.release && exp.only_jit);
    assert_eq!(exp.stdout, "hi\n");
    // Comments that merely start with those words are not directives.
    let plain = expectations("c.kes", "// release pass runs this\n// only-jit-ish\n// copying files\nmain() {}\n");
    assert!(!plain.release && !plain.only_jit && plain.copies.is_empty() && plain.replaces.is_empty());
}

#[test]
#[should_panic(expected = "e.kes: bad replace directive (expected <file> <old text> => <new text>): // replace: a.kes only")]
fn a_replace_without_an_arrow_panics() {
    expectations("e.kes", "// replace: a.kes only old text\nmain() {}\n");
}

#[test]
#[should_panic(expected = "e.kes: bad copy directive (expected a file inside the repo): // copy: ../outside.kes")]
fn a_copy_outside_the_repo_panics() {
    expectations("e.kes", "// copy: ../outside.kes\nmain() {}\n");
}

#[test]
#[should_panic(expected = "e.kes: `// asset:` is not supported in the engine's tests")]
fn the_program_tests_asset_directive_is_refused_here() {
    expectations("e.kes", "// asset: a.png\nmain() {}\n");
}

#[test]
#[should_panic(expected = "is not in test_t.kes, so `// replace:` would change nothing")]
fn a_replace_that_matches_nothing_panics() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("voxel_replace_unit");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let test = dir.join("t.kes");
    fs::write(&test, "main() {}\n").unwrap();
    let exp = expectations("t.kes", "// replace: test_t.kes no such text in the file => x\nmain() {}\n");
    prepare("_unit", &test, &exp);
}

#[test]
#[should_panic(expected = "`// copy: kestrel/voxel/grid.kes` would overwrite grid.kes in the scratch folder")]
fn a_copy_over_an_engine_module_panics() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("voxel_copy_unit");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let test = dir.join("t.kes");
    fs::write(&test, "main() {}\n").unwrap();
    let exp = expectations("t.kes", "// copy: kestrel/voxel/grid.kes\nmain() {}\n");
    prepare("_unit2", &test, &exp);
}
