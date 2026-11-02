//! `kestrel check --report`: what range analysis, the runtime and the inliner leave in a program,
//! as text.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fresh directory holding `files` (name, text).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("report_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        fs::write(dir.join(file), text).unwrap();
    }
    dir
}

fn kestrel(args: &[&str], file: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kestrel")).args(args).arg(file).output().expect("run kestrel")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Lines 8 and 14 are empty.
const FIXTURE: &str = "pub total(xs: list[int], k: int) -> int {
    var s = 0
    for i from 0 to xs.len {
        s += xs[i]
    }
    s + xs[k]
}

pub grow(var ys: list[int], n: int) {
    for i from 0 to n {
        ys.add(i * 2)
    }
}

main() {}
";

#[test]
fn the_report_matches_exactly() {
    let dir = project("exact", &[("report_fixture.kes", FIXTURE)]);
    let out = kestrel(&["check", "--report"], &dir.join("report_fixture.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let want = "report_fixture.kes
  function `total` (line 1): 3 checks left (2 removed), no allocations
    loop at line 3
      line 4: overflow check: `s + xs[i]` may exceed `int` (`s` can be any `int`; `xs[i]` can be any `int`)
    line 6: bounds check: `xs[k]` may be outside `xs` (`k` can be any `int`)
    line 6: overflow check: `s + xs[k]` may exceed `int` (`s` can be any `int`; `xs[k]` can be any `int`)
  function `grow` (line 9): 1 check left (0 removed), 1 allocation, 1 other way to fail
    loop at line 10
      line 11: overflow check: `i * 2` may exceed `int` (`i` is 0 to 2,147,483,646)
      line 11: allocation: `add` may grow `ys`
      line 11: can fail: `add` fails if `ys` would hold more than 2,147,483,647 items
  function `main` (line 15): no checks, no allocations
total: 6 checks, 2 removed, 4 left; 1 allocation; 1 other way to fail
";
    assert_eq!(stdout(&out), want);
    // The diagnostics go to standard error, and there are none.
    assert_eq!(stderr(&out), "");
}

#[test]
fn release_reports_only_release_checks() {
    let dir = project("release", &[("report_fixture.kes", FIXTURE)]);
    let out = kestrel(&["check", "--report", "--release"], &dir.join("report_fixture.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(!text.contains("overflow check"), "{text}");
    assert!(text.contains("    line 6: bounds check: `xs[k]` may be outside `xs` (`k` can be any `int`)\n"), "{text}");
    // `--release` before `--report` is the same.
    let again = kestrel(&["check", "--release", "--report"], &dir.join("report_fixture.kes"));
    assert_eq!(stdout(&again), text);
}

#[test]
fn report_needs_check() {
    let dir = project("needs_check", &[("f.kes", "main() {}\n")]);
    for cmd in ["run", "build"] {
        let out = kestrel(&[cmd, "--report"], &dir.join("f.kes"));
        assert_eq!(out.status.code(), Some(2), "{cmd}");
        assert!(stderr(&out).contains("`--report` goes with `check`: kestrel check --report file.kes"), "{}", stderr(&out));
        assert_eq!(stdout(&out), "", "{cmd} prints nothing else");
    }
    // `check` without it prints no report.
    let out = kestrel(&["check"], &dir.join("f.kes"));
    assert!(out.status.success());
    assert_eq!(stdout(&out), "");
}

#[test]
fn a_program_with_errors_gets_no_report() {
    let dir = project("errors", &[("bad.kes", "main() {\n    let x: int = true\n}\n")]);
    let out = kestrel(&["check", "--report"], &dir.join("bad.kes"));
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("error"), "{}", stderr(&out));
}

#[test]
fn the_limits_are_reported() {
    // 300 nested `if`s: past the 256 the analysis goes down.
    let depth = 300;
    let mut src = String::from("pub f(x: int) -> int {\n    var y = 0\n");
    for d in 0..depth {
        src.push_str(&format!("if x > {d} {{\n"));
    }
    src.push_str("y = x * 2\n");
    for _ in 0..depth {
        src.push_str("}\n");
    }
    src.push_str("y\n}\n\nmain() {}\n");
    let dir = project("limits", &[("deep.kes", &src)]);
    let out = kestrel(&["check", "--report"], &dir.join("deep.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("range analysis stopped early in `f` (its code nests too deeply), so every check in it stays\n"), "{text}");
    assert!(text.contains("overflow check: `x * 2` may exceed `int`"), "{text}");
}

#[test]
fn an_item_in_another_file_names_it() {
    let geom = "pub helper(a: int, b: int) -> int {\n    a * b\n}\n";
    let main = "use { helper from geom }\n\npub density(x: int, y: int) -> int {\n    helper(x, y) + y\n}\n\nmain() {}\n";
    let dir = project("modules", &[("main.kes", main), ("geom.kes", geom)]);
    let out = kestrel(&["check", "--report"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("    geom.kes:2 (in `helper`): overflow check: `a * b` may exceed `int` (`a` can be any `int`; `b` can be any `int`)\n"), "{text}");
    // Files are grouped in the order they were loaded: the program, then `geom.kes`.
    let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("no {s:?} in\n{text}"));
    assert!(at("main.kes\n") < at("geom.kes\n  function `helper` (line 1)"), "{text}");
}

#[test]
fn a_call_back_into_a_module_function_names_it_as_the_report_does() {
    // `walk` and `step` call each other: each inlines the other one level, and the call back stays.
    let geom = "pub walk(n: int) -> int {\n    if n <= 0 { 0 } else { step(n - 1) + 1 }\n}\n\npub step(n: int) -> int {\n    if n <= 0 { 1 } else { walk(n - 1) * 2 }\n}\n";
    let main = "use { walk from geom }\n\nmain() {\n    print(walk(5))\n}\n";
    let dir = project("call_back", &[("main.kes", main), ("geom.kes", geom)]);
    let out = kestrel(&["check", "--report"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("call: `walk` isn't inlined: it calls back into `walk`"), "{text}");
    // (The expression text still writes `geom.walk(...)`: that is the report's own naming, not the inliner's.)
    assert!(!text.contains("into `geom."), "{text}");
}

#[test]
fn an_inlined_item_names_its_function() {
    let src = "density(seed: int) -> int {\n    seed + 1\n}\n\npub solids(seed: int) -> int {\n    density(seed) * 2\n}\n\nmain() {}\n";
    let dir = project("inlined", &[("world.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("world.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("  function `density` (line 1): its own copy is never called; its checks are listed where it is inlined\n"), "{text}");
    assert!(text.contains("    line 2 (in `density`): overflow check: `seed + 1` may exceed `int` (`seed` can be any `int`)\n"), "{text}");
    assert!(text.contains("    line 6: overflow check: `(...) * 2` may exceed `int`"), "{text}");
}

#[test]
fn built_in_modules_are_left_out() {
    // `window.open` is a native: it may allocate and fail. The module's own functions aren't listed.
    let src = "use window\n\nmain() {\n    window.open(64, 64, \"report\")\n}\n";
    let dir = project("builtin", &[("w.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("w.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("w.kes\n  function `main` (line 3): "), "{text}");
    assert!(text.contains("calls the native function `window.open` (line 4); its body isn't known here"), "{text}");
    assert!(!text.contains("<builtin>") && !text.contains("window.kes"), "{text}");
}

#[test]
fn an_assumption_is_a_way_to_fail() {
    // (From Task 3 `kestrel check` also warns that this assumption removes no check, so stderr may
    // hold a warning.)
    let src = "pure g(n: int) -> int { @assume(n > 0)\n    n * 2\n}\n\nmain() {}\n";
    let dir = project("assume", &[("assume.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("assume.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("line 1: can fail: the assumption `n > 0` is checked here\n"), "{text}");
}

#[test]
fn a_fail_is_a_way_to_fail_and_its_message_allocates_nothing() {
    let src = "pure half(n: int) -> int {
    if n % 2 != 0 {
        fail(\"{n} is odd\")
    }
    n / 2
}

main() {}
";
    let dir = project("fail", &[("fail.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("fail.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("line 3: can fail: `fail` stops the program\n"), "{text}");
    assert!(text.contains("no allocations"), "{text}");
}

#[test]
fn a_trusted_assumption_is_no_way_to_fail_under_release() {
    // `--release` trusts `b > 0` (it removes only the division's check), so it isn't evaluated;
    // a checked build evaluates it.
    let src = "pure d(a: int, b: int) -> int {\n    @assume(b > 0)\n    a / b\n}\n\nmain() {}\n";
    let dir = project("assume_trusted", &[("trusted.kes", src)]);
    let item = "line 2: can fail: the assumption `b > 0` is checked here";
    let out = kestrel(&["check", "--report"], &dir.join("trusted.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains(item), "{}", stdout(&out));
    let out = kestrel(&["check", "--report", "--release"], &dir.join("trusted.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("assumption"), "{}", stdout(&out));
}

#[test]
fn a_promised_interface_call_lists_no_allocation() {
    // `Shape.area` promises `no_alloc`, so `s.area()` lists only that it can fail; `Sized.size`
    // promises nothing, so `t.size()` lists both.
    let src = "interface Shape {\n    @prove(no_alloc)\n    area(self) -> int\n}\n\ninterface Sized {\n    size(self) -> int\n}\n\npub f(s: Shape, t: Sized) -> int {\n    s.area() ^ t.size()\n}\n\nmain() {}\n";
    let dir = project("promised", &[("promised.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("promised.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let want = "  function `f` (line 10): no checks, 1 allocation, 2 other ways to fail
    line 11: can fail: calls a method of the interface value `s` (line 11); its body isn't known here
    line 11: allocation: calls a method of the interface value `t` (line 11); its body isn't known here
    line 11: can fail: calls a method of the interface value `t` (line 11); its body isn't known here
";
    assert!(text.contains(want), "{text}");
}

#[test]
fn int_vector_checks_read_like_the_scalar_ones() {
    // Each component of an `ivec2` gets its own `int` check; the values the compiler made for
    // `abs` read `(...)`, as for a number, not by their temporaries' names.
    let src = "pub f(v: ivec2) -> ivec2 {\n    abs(v) + ivec2(1, 0)\n}\n\nmain() {}\n";
    let dir = project("ivec", &[("ivec.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("ivec.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let want = "ivec.kes
  function `f` (line 1): 3 checks left (1 removed), no allocations
    line 2: overflow check: `-(...)` may exceed `int` (`(...)` is at most -1)
    line 2: overflow check: `-(...)` may exceed `int` (`(...)` is at most -1)
    line 2: overflow check: `(...) + (...)` may exceed `int` (`(...)` is 0 or more; `(...)` is exactly 1)
  function `main` (line 5): no checks, no allocations
total: 4 checks, 1 removed, 3 left; no allocations; no other ways to fail
";
    assert_eq!(stdout(&out), want);
}

#[test]
fn ivec4_checks_are_one_item_per_operation() {
    // One check for all four lanes, worded with the lane type; wrapping and bit operators have
    // none, and `--release` has none at all.
    let src = "pub f(a: ivec4, b: ivec4, n: int) -> ivec4 {
    let c = a * 3 + b
    let d = -c << n
    let e = (a +% b) & 255 ^ ~a
    d - e
}

main() {}
";
    let dir = project("ivec4", &[("ivec4.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("ivec4.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let want = "ivec4.kes
  function `f` (line 1): 5 checks left (0 removed), no allocations
    line 2: overflow check: `a * 3` may exceed `int` in a lane (the lanes of `a` can be any `int`)
    line 2: overflow check: `(a * 3) + b` may exceed `int` in a lane (the lanes of `a * 3` can be any `int`; the lanes of `b` can be any `int`)
    line 3: overflow check: `-c` may exceed `int` in a lane (the lanes of `c` can be any `int`)
    line 3: shift check: `-c << n` may shift by less than 0 or more than 31 (the lanes of `-c` are -2,147,483,647 or more; `n` can be any `int`)
    line 5: overflow check: `d - e` may exceed `int` in a lane (the lanes of `d` can be any `int`; the lanes of `e` can be any `int`)
  function `main` (line 8): no checks, no allocations
total: 5 checks, 0 removed, 5 left; no allocations; no other ways to fail
";
    assert_eq!(stdout(&out), want);
    let out = kestrel(&["check", "--report", "--release"], &dir.join("ivec4.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("total: 0 checks, 0 removed, 0 left"), "{}", stdout(&out));
}

#[test]
fn vector_checks_read_as_lanes() {
    // One range stands for all four lanes, written with the lane type.
    let src = "pub f(a: ivec4, h: ivec4) -> ivec4 {\n    a * (h & 0xFFFF)\n}\n\nmain() {}\n";
    let dir = project("lanes", &[("lanes.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("lanes.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let line = text.lines().find(|l| l.contains("line 2: overflow check")).unwrap_or_else(|| panic!("{text}"));
    for want in ["may exceed `int` in a lane", "the lanes of `a` can be any `int`", "are 0 to 65,535"] {
        assert!(line.contains(want), "{want}: {text}");
    }
    // A shift by a known amount and a masked product are removed, and not listed.
    let src = "pub g(h: ivec4) -> ivec4 {\n    let k = h & 0xFFFF\n    (k * 3 + 1) >> 1\n}\n\nmain() {}\n";
    let dir = project("lanes_removed", &[("removed.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("removed.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("function `g` (line 1): no checks left (3 removed), no allocations"), "{}", stdout(&out));
}

#[test]
fn a_proven_ivec4_program_has_no_vector_checks_left() {
    // `-v`, shifts by a known amount and a masked product: all seven removed, `id`'s own check left.
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/programs/ivec4_proven.kes");
    let out = kestrel(&["check", "--report"], &file);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("function `main` (line 10): no checks left (7 removed), no allocations"), "{text}");
    assert!(text.contains("total: 8 checks, 7 removed, 1 left"), "{text}");
}

#[test]
fn a_vector_sum_names_its_lanes() {
    // `h` is a parameter, read where it is used: the report names `h.x`, not a temporary.
    let src = "pub total(h: ivec4) -> int {\n    h.sum()\n}\n\nmain() {}\n";
    let dir = project("vector_sum", &[("sum.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("sum.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("    line 2: overflow check: `h.x + h.y` may exceed `int` (`h.x` can be any `int`; `h.y` can be any `int`)\n"), "{text}");
    assert!(text.contains("`((h.x + h.y) + h.z) + h.w` may exceed `int`"), "{text}");
}

const COPIES: &str = "pure best[T](xs: list[T]) -> int {
    var n = 0
    for x in xs {
        n += 1
    }
    n
}

pure spare[T](x: T) -> T { x }

main() {
    let a: list[int] = [1, 2]
    print(best(a))
}
";

#[test]
fn copies_in_the_report() {
    let dir = project("copies", &[("copies.kes", COPIES)]);
    let out = kestrel(&["check", "--report"], &dir.join("copies.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("function `best[int]` (line 1)"), "{text}");
    assert!(text.contains("`spare`: not checked, no copies"), "{text}");
    assert!(!text.contains("`best`: not checked"), "{text}");
    // `best[int]` is inlined into `main`, whose report names the generic.
    assert!(text.contains("    loop at line 3 (in `best`)\n"), "{text}");
}

#[test]
fn an_item_inlined_from_a_copy_names_the_generic() {
    // `total[Goblin]` is inlined into `main`, whose loop is then listed under `main`; a copy of
    // `total` for another type holds the same lines and must not be named in its place.
    let src = "pure total[T](xs: list[T]) -> int {
    var n = 0
    for x in xs {
        n += 1
    }
    n
}

pub run(xs: list[int], ys: list[float]) -> int {
    total(xs) + total(ys) + xs[0]
}

main() {}
";
    let dir = project("inlined_copy", &[("inlined.kes", src)]);
    let out = kestrel(&["check", "--report"], &dir.join("inlined.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(!text.contains("(in `total["), "{text}");
    assert!(text.contains("    loop at line 3 (in `total`)\n"), "{text}");
    assert!(text.contains("function `total[int]` (line 1)") && text.contains("function `total[float]` (line 1)"), "{text}");
}

/// The report's items ("line 25: overflow check: ..."), without the function they were inlined
/// from, sorted.
fn items(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("line "))
        .map(|l| match (l.find(" (in `"), l.find("`):")) {
            (Some(at), Some(end)) if at < end => format!("{}{}", &l[..at], &l[end + 2..]),
            _ => l.to_string(),
        })
        .collect();
    out.sort();
    out
}

/// The totals line: "total: 4 checks, 1 removed, 3 left; 3 allocations; 1 other way to fail".
fn totals(text: &str) -> &str {
    text.lines().find(|l| l.starts_with("total: ")).expect("a totals line")
}

#[test]
fn a_reloadable_program_reports_like_its_main_twin() {
    // The `main` the compiler makes for a reloadable program is generated code, which the report
    // leaves out; `setup` and `frame` are `pub`, so their own copies list their checks, and the
    // constants, which the host builds before it calls `setup`, are listed under `setup`. The twin
    // can't make them `pub` (a `main` program with both is refused), so the inliner folds them into
    // its `main`, which lists the same items under its own row. Every item and the totals agree;
    // only the line the constants' arena is made at is each program's first line of code to run.
    let programs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/programs");
    let report = |file: &str| {
        let src = fs::read_to_string(programs.join(file)).unwrap();
        // The same file name, so the reports' headers agree.
        let dir = project(file.trim_end_matches(".kes"), &[("game.kes", &src)]);
        let out = kestrel(&["check", "--report"], &dir.join("game.kes"));
        assert!(out.status.success(), "{file}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{file}");
        stdout(&out)
    };
    let (reloadable, twin) = (report("reload_shape.kes"), report("reload_shape_main.kes"));
    assert!(!reloadable.contains("function `main`"), "{reloadable}");
    assert!(twin.contains("  function `main` (line 32)"), "{twin}");
    assert!(reloadable.contains("  function `setup` (line 19): no checks, 2 allocations\n"), "{reloadable}");
    assert!(reloadable.contains("  function `frame` (line 24): 3 checks left (1 removed), 1 allocation, 1 other way to fail\n"), "{reloadable}");
    assert_eq!(totals(&reloadable), totals(&twin));
    assert_eq!(totals(&twin), "total: 4 checks, 1 removed, 3 left; 3 allocations; 1 other way to fail");
    let arena = |line: u32| format!("line {line}: allocation: makes the arena the constants live in");
    let mut reloadable_items: Vec<String> = items(&reloadable).into_iter().map(|i| if i == arena(19) { arena(32) } else { i }).collect();
    reloadable_items.sort();
    assert!(reloadable_items.contains(&arena(32)), "{reloadable}");
    assert_eq!(reloadable_items, items(&twin), "{reloadable}\n{twin}");
}
