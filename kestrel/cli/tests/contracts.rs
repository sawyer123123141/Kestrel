//! `@prove` and `@assume` at the command line: `kestrel check` warns about an assumption that removes
//! no check in either build; `run` and `build` don't, and annotations change nothing that runs.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fresh directory holding `files` (name, text).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("contracts_{name}"));
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

const UNUSED: &str = "show(n: int) {
    @assume(n > 0)
    print(n)
}

main() { show(4) }
";

#[test]
fn an_assumption_that_removes_nothing_warns() {
    let dir = project("unused", &[("unused.kes", UNUSED)]);
    let file = dir.join("unused.kes");
    for args in [&["check"][..], &["check", "--release"]] {
        let out = kestrel(args, &file);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains("warning: `@assume(n > 0)` removes no check; delete it"), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains("unused.kes:2:5"), "{}", stderr(&out));
    }
    // `run` doesn't repeat it.
    let out = kestrel(&["run"], &file);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!((stdout(&out).as_str(), stderr(&out).as_str()), ("4\n", ""));
}

#[test]
fn an_assumption_for_overflow_checks_does_not_warn_under_release() {
    // It removes `a * 3`'s overflow check in a checked build; `--release` has none to remove.
    let src = "show(a: int) {\n    @assume(a >= 0 and a <= 1000)\n    print(a * 3)\n}\n\nmain() { show(5) }\n";
    let dir = project("overflow", &[("overflow.kes", src)]);
    for args in [&["check"][..], &["check", "--release"]] {
        let out = kestrel(args, &dir.join("overflow.kes"));
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{args:?}");
    }
}

#[test]
fn a_merged_assumption_warns_at_each_condition() {
    let src = "show(n: int, m: int) {\n    @assume(n > 0) @assume(m > 0)\n    print(n + m)\n}\n\nmain() { show(1, 2) }\n";
    let dir = project("merged", &[("merged.kes", src)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("merged.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "2:5: warning: `@assume(n > 0)` removes no check; delete it\n2:20: warning: `@assume(m > 0)` removes no check; delete it\n"
    );
}

#[test]
fn the_report_reuses_the_analysis_and_still_warns() {
    let dir = project("report", &[("report.kes", UNUSED)]);
    let out = kestrel(&["check", "--report", "--diagnostics=short"], &dir.join("report.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "2:5: warning: `@assume(n > 0)` removes no check; delete it\n");
    assert!(stdout(&out).starts_with("report.kes\n"), "{}", stdout(&out));
}

#[test]
fn annotations_do_not_change_what_runs() {
    let src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/programs/prove_parses.kes")).unwrap();
    let dir = project("same", &[("prove_parses.kes", &src)]);
    let file = dir.join("prove_parses.kes");
    for release in [false, true] {
        let flag: &[&str] = if release { &["--release"] } else { &[] };
        let out = kestrel(&[&["run"][..], flag].concat(), &file);
        assert!(out.status.success(), "run {flag:?}: {}", stderr(&out));
        assert_eq!((stdout(&out).as_str(), stderr(&out).as_str()), ("23\n", ""), "run {flag:?}");
        let exe = dir.join(format!("prove_parses_{release}{}", std::env::consts::EXE_SUFFIX));
        let out = kestrel(&[&["build"][..], flag, &["-o", exe.to_str().unwrap()]].concat(), &file);
        assert!(out.status.success(), "build {flag:?}: {}", stderr(&out));
        let ran = Command::new(&exe).output().expect("run the executable");
        assert!(ran.status.success(), "{}", stderr(&ran));
        assert_eq!(stdout(&ran), "23\n", "build {flag:?}");
    }
}

/// `pub f(x: int) -> int` with `depth` nested `if`s around `y = x * 2`: past the 256 levels range
/// analysis goes down, so it gives the function up. `head` goes before the nesting.
fn deep(name: &str, head: &str, depth: usize) -> String {
    let mut src = format!("pub {name}(x: int) -> int {{\n    var y = 0\n{head}");
    for d in 0..depth {
        src.push_str(&format!("if x > {d} {{\n"));
    }
    src.push_str("y = x * 2\n");
    for _ in 0..depth {
        src.push_str("}\n");
    }
    src.push_str("y\n}\n");
    src
}

#[test]
fn release_contracts_use_the_release_build() {
    // `a * b` keeps its overflow check in a checked build; `--release` has none.
    let src = "@prove(no_checks)\npure m(a: int, b: int) -> int { a * b }\n\nmain() { print(m(6, 7)) }\n";
    let dir = project("release", &[("release.kes", src)]);
    let file = dir.join("release.kes");
    let out = kestrel(&["check", "--diagnostics=short"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).starts_with("1:8: error: `m` can't be proven `no_checks`: it has an overflow check on line 2: `a * b` may exceed `int`"), "{}", stderr(&out));
    let out = kestrel(&["check", "--release"], &file);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let exe = dir.join(format!("release{}", std::env::consts::EXE_SUFFIX));
    let out = kestrel(&["build", "--release", "-o", exe.to_str().unwrap()], &file);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&Command::new(&exe).output().expect("run the executable")), "42\n");
    // A checked run fails the contract and runs nothing.
    let out = kestrel(&["run"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("can't be proven `no_checks`"), "{}", stderr(&out));
}

#[test]
fn the_report_prints_when_contracts_fail() {
    let src = "@prove(no_alloc)\npub grow(var ys: list[int]) {\n    ys.add(1)\n}\n\nmain() {}\n";
    let dir = project("report_fails", &[("grow.kes", src)]);
    let out = kestrel(&["check", "--report", "--diagnostics=short"], &dir.join("grow.kes"));
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "1:8: error: `grow` can't be proven `no_alloc`: it allocates on line 3: `add` may grow `ys`\n");
    assert!(stdout(&out).starts_with("grow.kes\n  function `grow` (line 2): no checks, 1 allocation, 1 other way to fail\n"), "{}", stdout(&out));
}

/// Runs `kestrel check --diagnostics=short` on `src` (in a directory named after `name`).
fn check_short(name: &str, src: &str) -> Output {
    let dir = project(name, &[(&format!("{name}.kes"), src)]);
    kestrel(&["check", "--diagnostics=short"], &dir.join(format!("{name}.kes")))
}

#[test]
fn a_function_the_analysis_gave_up_on_fails_every_contract() {
    let src = format!("@prove(no_alloc, inline)\n{}\nmain() {{}}\n", deep("f", "", 300));
    let out = check_short("gave_up", &src);
    assert_eq!(out.status.code(), Some(1));
    let said = "can't be proven `{}`: range analysis stopped early in it (its code nests too deeply), so nothing can be proven about it; split it into smaller functions";
    for (col, c) in [(8, "no_alloc"), (18, "inline")] {
        let want = format!("1:{col}: error: `f` {}", said.replace("{}", c));
        assert!(stderr(&out).contains(&want), "{want}\n{}", stderr(&out));
    }
}

#[test]
fn a_callee_given_up_on_fails_its_callers() {
    // `g` is far too big to inline, so `f` calls it.
    let src = format!("{}\n@prove(no_checks)\npub f(x: int) -> int {{\n    g(x ^ 1)\n}}\n\nmain() {{}}\n", deep("g", "", 300));
    let line = src.lines().count() - 3;
    let out = check_short("callee_gave_up", &src);
    assert_eq!(out.status.code(), Some(1));
    let want = format!("error: `f` can't be proven `no_checks`: it calls `g` (line {line}), where range analysis stopped early (its code nests too deeply)\n");
    assert!(stderr(&out).ends_with(&want), "{want}\n{}", stderr(&out));
}

#[test]
fn an_inlined_copy_in_a_caller_given_up_on_fails() {
    // `get`'s own copy holds (its assumption removes the bounds check); its copy inside `big`,
    // which the analysis gives up on, keeps every check.
    let get = "@prove(no_checks)\npure get(xs: list[int], i: int) -> int {\n    @assume(i >= 0 and i < xs.len)\n    xs[i]\n}\n";
    let big = deep("big", "    let xs: list[int] = [1, 2, 3]\n    y = get(xs, x)\n", 300);
    let src = format!("{get}\n{big}\nmain() {{}}\n");
    let out = check_short("copy_gave_up", &src);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let want = "1:8: error: `get` can't be proven `no_checks` where it is inlined into `big` (line 10): range analysis stopped early in `big` (its code nests too deeply)";
    assert!(stderr(&out).starts_with(want), "{want}\n{}", stderr(&out));
}

#[test]
fn full_diagnostics_show_the_help() {
    let src = "interface Shape {\n    area(self) -> int\n}\n\n@prove(no_alloc)\npub size(s: Shape) -> int { s.area() }\n\n@prove(no_panic)\npub apply(f: (int) -> int) -> int { f(1) }\n\npub fill(var xs: list[int]) {\n    @prove(no_alloc)\n    par for x in xs {\n        x = 1\n    }\n}\n\nmain() {}\n";
    let dir = project("help", &[("help.kes", src)]);
    let out = kestrel(&["check"], &dir.join("help.kes"));
    assert_eq!(out.status.code(), Some(1));
    let text = stderr(&out).replace("\r\n", "\n");
    for help in [
        "= help: add `@prove(no_alloc)` to the interface method `Shape.area`; then every implementation must keep it\n",
        "= help: contracts can't see into a function value yet; call a named function or an interface method instead\n",
        "= help: a `par for` makes an arena for each thread; put `@prove(no_alloc)` on the function its body calls\n",
    ] {
        assert!(text.contains(help), "{help}\n{text}");
    }
    assert!(text.ends_with("3 errors\n"), "{text}");
}

#[test]
fn a_chain_into_another_file_names_it() {
    let geom = "pub grow(var ys: list[int], n: int) {\n    if n <= 0 {\n        return\n    }\n    ys.add(n)\n}\n";
    let main = "use { grow from prove_geom }\n\n@prove(no_alloc)\npub step(var ys: list[int]) {\n    grow(var ys, 3)\n}\n\nmain() {}\n";
    let dir = project("other_file", &[("prove_other_file.kes", main), ("prove_geom.kes", geom)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("prove_other_file.kes"));
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "3:8: error: `step` can't be proven `no_alloc`: it calls `grow` (line 5), which adds to `ys` (prove_geom.kes:5) and may grow the list\n");
}

#[test]
fn an_assumption_on_an_ivec4_lane_removes_no_check() {
    // One lane says nothing about the other three, so the vector's check stays and it warns.
    let src = "show(v: ivec4) {\n    @assume(v.x >= 0 and v.x < 10)\n    print(v * 1000)\n}\n\nmain() { show(ivec4(1)) }\n";
    let dir = project("lane", &[("lane.kes", src)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("lane.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "2:5: warning: `@assume(v.x >= 0 and v.x < 10)` removes no check; delete it\n");
}

/// `Stack.put` stores an item; the contract on it holds for `int` and fails for `string`, which
/// copying allocates. `b.put` on line 12 is what makes the `string` copy.
const STACK_PROVE: &str = "struct Stack[T] {
    items: list[T] = []

    @prove(no_alloc)
    put(var self, i: int, v: T) { self.items[i] = v }
}

main() {
    var a = Stack[int](items: [1])
    a.put(0, 2)
    var b = Stack[string](items: [\"x\"])
    b.put(0, \"y\")
}
";

#[test]
fn a_contract_is_judged_per_copy() {
    let out = check_short("per_copy", STACK_PROVE);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stderr(&out);
    assert_eq!(text.matches("error:").count(), 1, "{text}");
    let want = "`Stack[string].put` (made by `Stack[string]` on line 12) can't be proven `no_alloc`";
    assert!(text.contains(want), "{want}\n{text}");
    // With only the `int` lines, the one copy holds.
    let ints: String = STACK_PROVE.lines().filter(|l| !l.contains("b.put") && !l.contains("var b")).map(|l| format!("{l}\n")).collect();
    let out = check_short("per_copy_int", &ints);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
}

#[test]
fn a_library_copy_breaks_a_program_contract() {
    let geom = "pub struct Stack[T] {
    items: list[T] = []

    put(var self, i: int, v: T) { self.items[i] = v }
}
";
    let main = "use { Stack from geom }

@prove(no_alloc)
fill(var s: Stack[string]) { s.put(0, \"y\") }

main() {
    var s = Stack[string](items: [\"x\"])
    fill(var s)
}
";
    let dir = project("library_copy", &[("geom.kes", geom), ("main.kes", main)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stderr(&out);
    // Reported at the contract on `fill` (line 3 of `main.kes`), and the item is in the library's
    // copy; whatever the inliner did with `put`, the error names the line in `geom.kes`.
    assert_eq!(text, "3:8: error: `fill` can't be proven `no_alloc`: it allocates on geom.kes:4: makes a copy of `v`\n");
}

#[test]
fn a_generic_with_no_copies_passes() {
    let src = "@prove(no_alloc, no_panic)\npure best[T](xs: list[T]) -> int { xs.len }\n\nmain() {}\n";
    let out = check_short("no_copies", src);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
}

#[test]
fn one_copy_fails_a_loop_contract_another_passes() {
    // `held = x` copies a `string` (an allocation) and a plain `int` copies nothing.
    let src = "pure keeps[T](xs: list[T], v: T) -> int {
    var n = 0
    var held = v
    @prove(no_alloc)
    for x in xs {
        held = x
        n += 1
    }
    n
}

main() {
    let a: list[int] = [1, 2, 3]
    let b: list[string] = [\"a\", \"b\"]
    print(keeps(a, 0), keeps(b, \"z\"))
}
";
    let out = check_short("loop_per_copy", src);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stderr(&out);
    assert_eq!(text.matches("error:").count(), 1, "{text}");
    let want = "the loop at line 5 (made by `keeps[string]` on line 15) can't be proven `no_alloc`";
    assert!(text.contains(want), "{want}\n{text}");
    assert!(text.contains("makes a copy of `x`"), "{text}");
}

#[test]
fn a_copy_of_a_library_contract_names_where_the_program_made_it() {
    // The contract is the library's; the copy is made by `main.kes`, so the clause names its file.
    let geom = "pub struct Stack[T] {\n    items: list[T] = []\n\n    @prove(no_alloc)\n    put(var self, i: int, v: T) { self.items[i] = v }\n}\n";
    let main = "use { Stack from geom }\n\nmain() {\n    var s = Stack[string](items: [\"x\"])\n    s.put(0, \"y\")\n}\n";
    let dir = project("library_contract", &[("geom.kes", geom), ("main.kes", main)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let want = "`Stack[string].put` (made by `Stack[string]` on main.kes:5) can't be proven `no_alloc`: it allocates on line 5: makes a copy of `v`";
    assert!(stderr(&out).contains(want), "{want}\n{}", stderr(&out));
}

const PLAIN_STATE: &str = "use { Rng from random }

struct Game {
    ball: vec2
    vel: vec2
    p1: float
    p2: float
    score1: int
    score2: int
    rng: Rng
}

struct Pair {
    game: Game
    ring: list[Game]
}

@prove(no_alloc)
save(var ring: list[Game], k: int, g: Game) {
    ring[k % 16] = g
}

@prove(no_alloc)
restore(var g: Game, ring: list[Game], k: int) {
    g = ring[k % 16]
}

fresh(seed: int) -> Game {
    Game(ball: vec2(0, 0), vel: vec2(1, 0.5), p1: 0, p2: 0, score1: seed, score2: 0, rng: Rng(seed: 5))
}

main() {
    var ring: list[Game] = []
    for i from 0 to 16 {
        ring.add(fresh(i))
    }
    var g = fresh(99)
    var p = Pair(game: fresh(7), ring: ring)
    @prove(no_alloc)
    for k from 0 to 100 {
        save(var ring, k, g)
        restore(var g, ring, k + 3)
        p.ring[k % 16] = p.game
        p.game = p.ring[(k + 11) % 16]
    }
    print(g.score1, p.game.score1, ring.len)
}
";

#[test]
fn save_and_restore_of_plain_state_prove_no_alloc() {
    // A snapshot with no list, map or string is copied without allocating, by separate variables
    // and by assignments between field-disjoint places of one root.
    let dir = project("plain_state", &[("pong.kes", PLAIN_STATE)]);
    for args in [&["check"][..], &["check", "--release"]] {
        let out = kestrel(args, &dir.join("pong.kes"));
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        assert_eq!((stdout(&out).as_str(), stderr(&out).as_str()), ("", ""), "{args:?}");
    }
}

#[test]
fn comparing_plain_structs_proves_no_alloc_and_no_panic() {
    // Decision 19: `==` on a struct of scalars compares inline; on one holding a string and an
    // enum it calls a generated function. Neither allocates or fails.
    let src = "struct Pad {\n    up: bool\n    down: bool\n}\n\nenum Shape {\n    Dot\n    Circle { r: float }\n}\n\nstruct Named {\n    name: string\n    shape: Shape\n}\n\n@prove(no_alloc, no_panic)\npub same_pad(a: Pad, b: Pad) -> bool { a == b }\n\n@prove(no_alloc, no_panic)\npub differ(a: Named, b: Named) -> bool { a != b }\n\nmain() {}\n";
    let dir = project("equatable", &[("equatable.kes", src)]);
    for args in [&["check"][..], &["check", "--release"]] {
        let out = kestrel(args, &dir.join("equatable.kes"));
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{args:?}");
    }
}
