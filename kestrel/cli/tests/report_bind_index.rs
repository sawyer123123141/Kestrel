//! `kestrel check --report` after `bind_index` binds a run of `parts[i]` places once: the same
//! text as with every place indexed, with fewer checks counted.
use std::fs;
use std::path::Path;
use std::process::Command;

fn report(file: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_kestrel")).args(["check", "--report"]).arg(file).output().expect("run kestrel");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

#[test]
fn step_index_reports_the_same_text_with_fewer_checks() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/kernels/step_index.kes");
    // Before binding: 31 removed in each function, 62 checks in all.
    let expected = "step_index.kes
  function `Particle.step` (line 15): its own copy is never called; its checks are listed where it is inlined
  function `spawn` (line 36): its own copy is never called; its checks are listed where it is inlined
  function `bench` (line 44): no checks left (3 removed), 2 allocations, 1 other way to fail
    line 46: allocation: `arena { }` makes a new arena
    loop at line 37 (in `spawn`)
      line 40 (in `spawn`): allocation: `add` may grow `parts`
      line 40 (in `spawn`): can fail: `add` fails if `parts` would hold more than 2,147,483,647 items
  function `main` (line 64): no checks left (3 removed), 2 allocations, 1 other way to fail
    line 46 (in `bench`): allocation: `arena { }` makes a new arena
    loop at line 37 (in `spawn`)
      line 40 (in `spawn`): allocation: `add` may grow `parts`
      line 40 (in `spawn`): can fail: `add` fails if `parts` would hold more than 2,147,483,647 items
total: 6 checks, 6 removed, 0 left; 4 allocations; 2 other ways to fail
";
    assert_eq!(report(&file), expected);
}

#[test]
fn a_bound_index_left_checked_reads_as_before() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("report_bind_index");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bound.kes");
    fs::write(&file, "struct P {\n    x: float\n    y: float\n}\n\npub f(var ps: list[P], i: int) {\n    ps[i].x += 1\n    ps[i].y = ps[i].x * 2\n}\n\nmain() {}\n").unwrap();
    // Before binding, line 7's two places were each a check left, and line 8's two were removed:
    // now the one `Bind` holds the check.
    let expected = "bound.kes
  function `f` (line 6): 1 check left (0 removed), no allocations
    line 7: bounds check: `ps[(...)]` may be outside `ps` (`(...)` can be any `int`)
  function `main` (line 11): no checks, no allocations
total: 1 check, 0 removed, 1 left; no allocations; no other ways to fail
";
    assert_eq!(report(&file), expected);
}
