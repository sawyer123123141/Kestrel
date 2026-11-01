//! Accumulator recursion as a loop (performance round, decision 6) changes no program's output:
//! every program test with a function that calls itself prints the same under `--release`, where
//! the recursion may be a loop, as in a checked build, where it never is, whenever the checked run
//! succeeds (it overflowed nowhere, so wrapping can't make the two differ).

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use common::directives::parse;
use common::{hermetic, output_within};

/// Whether `src` has a function, declared at the start of a line, whose body (up to the next `}`
/// at the start of a line) calls it.
fn recursive(src: &str) -> bool {
    let lines: Vec<&str> = src.lines().collect();
    lines.iter().enumerate().any(|(i, line)| {
        let decl = line.trim_start_matches("pub ").trim_start_matches("pure ");
        let Some((name, _)) = decl.split_once('(') else { return false };
        let named = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !line.starts_with(char::is_whitespace);
        let call = format!("{name}(");
        named && (decl[name.len() + 1..].contains(&call) || lines[i + 1..].iter().take_while(|l| !l.starts_with('}')).any(|l| l.contains(&call)))
    })
}

fn run(path: &Path, release: bool, env: &[(String, String)]) -> Option<Output> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    let mode: &[&str] = if release { &["run", "--release"] } else { &["run"] };
    hermetic(cmd.args(mode).arg(path)).envs(env.iter().map(|(k, v)| (k, v)));
    output_within(&mut cmd, Duration::from_secs(30))
}

#[test]
fn recursive_programs_print_the_same_released_as_checked() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/programs");
    let mut files: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "kes")).collect();
    files.sort();
    let (mut failures, mut compared) = (Vec::new(), Vec::new());
    for path in files {
        let src = fs::read_to_string(&path).unwrap();
        if !recursive(&src) {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let exp = parse(&name, &src);
        let Some(checked) = run(&path, false, &exp.env).filter(|o| o.status.success()) else { continue };
        compared.push(name.clone());
        match run(&path, true, &exp.env) {
            Some(r) if r.status.success() && r.stdout == checked.stdout => {}
            Some(r) => failures.push(format!("{name}: checked printed\n{}\nreleased printed\n{}{}", String::from_utf8_lossy(&checked.stdout), String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr))),
            None => failures.push(format!("{name}: the released run took over 30 seconds")),
        }
    }
    assert!(compared.iter().any(|n| n == "recurse_acc.kes"), "the folded recursions were compared: {compared:?}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}
