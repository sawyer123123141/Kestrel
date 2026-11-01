//! Programs split across files with `use`.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

/// A fresh directory holding `files` (name, text).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        fs::write(dir.join(file), text).unwrap();
    }
    dir
}

fn kestrel(args: &[&str], file: &Path) -> Output {
    common::hermetic(Command::new(env!("CARGO_BIN_EXE_kestrel")).args(args).arg(file)).output().unwrap()
}

const GEOM: &str = "pub struct Point {
    x: int
    y: int

    pure sum(self) -> int { self.x + self.y }
}

pub let ORIGIN_X = 10

pub pure dist2(a: Point, b: Point) -> int {
    let dx = a.x - b.x
    let dy = a.y - b.y
    dx * dx + dy * dy
}

helper() -> int { 1 }

pub enum Shape {
    Dot
    Box { w: int, h: int }
}

pub pure at(xs: [int], i: int) -> int {
    xs[i]
}
";

#[test]
fn imported_names_and_qualified_access() {
    let main = "use { Point, dist2 from geom }
use geom

main() {
    let a = Point(1, 2)
    let b: Point = Point(4, 6)
    print(dist2(a, b), a.sum(), geom.ORIGIN_X)
    let s = geom.Shape.Box(w: 2, h: 3)
    print(s, geom.dist2(b, a), a)
}
";
    let dir = project("modules_ok", &[("main.kes", main), ("geom.kes", GEOM)]);
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "25 3 10\nShape.Box(w: 2, h: 3) 25 Point(x: 1, y: 2)\n");
}

#[test]
fn module_errors_point_at_the_right_file() {
    let main = "use { helper, nope from geom }
use missing
use geom

main() {
    let dist2 = 1
    print(geom)
}
";
    let dir = project("modules_errors", &[("main.kes", main), ("geom.kes", GEOM)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    let err = String::from_utf8_lossy(&out.stderr);
    for expected in [
        "1:7: error: `helper` isn't `pub` in `geom`",
        "1:15: error: `geom` has no `nope`",
        "2:5: error: can't find module `missing`: there's no `missing.kes` next to this file",
        "7:11: error: `geom` is a module; use one of its items, like `geom.something`",
    ] {
        assert!(err.contains(expected), "missing {expected:?} in:\n{err}");
    }
    // Errors inside an imported file name that file.
    let bad = project("modules_bad_import", &[("main.kes", "use geom\nmain() {}\n"), ("geom.kes", "pub f() -> int { true }\n")]);
    let out = kestrel(&["check"], &bad.join("main.kes"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("geom.kes:1:18"), "{err}");
}

#[test]
fn a_module_may_start_with_a_byte_order_mark() {
    let geom = format!("\u{feff}{GEOM}");
    let dir = project("modules_bom", &[("main.kes", "use { dist2, Point from geom }\nmain() {\n    print(dist2(Point(0, 0), Point(3, 4)))\n}\n"), ("geom.kes", &geom)]);
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "25\n");
}

#[test]
fn runtime_errors_report_the_line_and_their_own_file() {
    let main = "use { at from geom }\nmain() {\n    print(at([1, 2], 5))\n}\n";
    let dir = project("modules_runtime", &[("main.kes", main), ("geom.kes", GEOM)]);
    // `xs[i]` is on line 24 of geom.kes; the check is inlined into `main`, and still names geom.kes.
    let want = "index 5 out of bounds for length 2 on line 24 of geom.kes";
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(101));
    assert!(String::from_utf8_lossy(&out.stderr).contains(want), "{}", String::from_utf8_lossy(&out.stderr));
    // A built program carries the name too (only the file's name, not its folder).
    let exe = dir.join("main_runtime.exe");
    let built = kestrel(&["build", "-o", exe.to_str().unwrap()], &dir.join("main.kes"));
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = std::process::Command::new(&exe).env("KESTREL_POISON", "1").output().expect("run the built program");
    assert_eq!(run.status.code(), Some(101));
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains(want), "{err}");
    assert!(!err.contains(dir.to_str().unwrap()), "the built program mustn't carry the folder: {err}");
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_native_error_names_the_module_file_that_called_it() {
    let shapes = "use draw\nuse { WHITE from draw }\n\npub dot() {\n    draw.circle(vec2(1, 1), 2, WHITE)\n}\n";
    let main = "use { dot from shapes }\nmain() {\n    dot()\n}\n";
    let dir = project("modules_native_error", &[("main.kes", main), ("shapes.kes", shapes)]);
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(101));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("on line 5 of shapes.kes"), "{err}");
}

const SHADE: &str = "pub struct Tint {
    r: float
    g: float
    a: float = 1
}

pub let DARK = Tint(0.25, 0.5)
pub let CLEAR = Tint(0, 0, 0)
pub let ORIGIN = vec2(0, 0)
";

#[test]
fn constants_can_hold_structs_from_modules() {
    let main = "use shade
use { Tint, DARK from shade }
let BG = shade.DARK
let SKY = Tint(0.5, 1, a: 0.25)
let HOME = shade.ORIGIN
main() {
    print(BG, DARK, SKY, HOME, shade.CLEAR)
}
";
    let dir = project("modules_struct_consts", &[("main.kes", main), ("shade.kes", SHADE)]);
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let want = "Tint(r: 0.25, g: 0.5, a: 1) Tint(r: 0.25, g: 0.5, a: 1) Tint(r: 0.5, g: 1, a: 0.25) vec2(x: 0, y: 0) Tint(r: 0, g: 0, a: 0)\n";
    assert_eq!(String::from_utf8_lossy(&out.stdout), want);
}

#[test]
fn a_file_named_like_a_built_in_module_is_refused() {
    // `rollback` is a built-in module of several files.
    for m in ["draw", "rollback"] {
        let (main, file) = (format!("use {m}\nmain() {{}}\n"), format!("{m}.kes"));
        let dir = project(&format!("modules_shadow_{m}"), &[("main.kes", &main), (&file, "pub f() -> int { 1 }\n")]);
        let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
        assert_eq!(out.status.code(), Some(1));
        let err = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
        assert_eq!(err, format!("1:5: error: `{m}` is a built-in module, so {m}.kes can't be used as a module; rename the file\n"));
    }
}

#[test]
fn a_program_named_like_a_built_in_module_is_told_to_rename_the_program() {
    // The clashing file is the program itself, whichever file uses the module.
    for (name, files) in [
        ("modules_shadow_entry", vec![("window.kes", "use window\nmain() {}\n")]),
        ("modules_shadow_entry_via_module", vec![("window.kes", "use geom\nmain() {}\n"), ("geom.kes", "use window\npub f() -> int { 1 }\n")]),
    ] {
        let dir = project(name, &files);
        let out = kestrel(&["check", "--diagnostics=short"], &dir.join("window.kes"));
        assert_eq!(out.status.code(), Some(1), "{name}");
        let err = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
        assert!(err.contains("`window` is a built-in module, so a program can't be called window.kes; rename the program\n"), "{name}: {err}");
        assert!(!err.contains("rename the file"), "{name}: {err}");
    }
}

#[test]
fn natives_in_programs_are_errors_not_crashes() {
    let main = "use geom\nnative beep()\nmain() {\n    beep()\n}\n";
    let dir = project("modules_natives", &[("main.kes", main), ("geom.kes", "pub native twice(x: int) -> int\n")]);
    for command in ["check", "run", "build"] {
        let out = kestrel(&[command, "--diagnostics=short"], &dir.join("main.kes"));
        let err = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
        assert_eq!(out.status.code(), Some(1), "{command}: {err}");
        assert_eq!(err.matches("only Kestrel's built-in modules can declare native functions").count(), 2, "{command}: {err}");
    }
}

#[test]
fn the_built_in_modules_check() {
    let main = "use window
use draw
use input
use rollback
use { Color from draw }
let BG = draw.BLACK
let ACCENT = Color(1, 0.5, 0)
main() {
    window.open(800, 600, \"demo\")
    while window.frame() {
        draw.clear(BG)
        draw.rect(vec2(10, 10), window.size() * 0.5, ACCENT)
        draw.text(\"fps {window.fps()}\", vec2(4, 4), 16, draw.WHITE)
        if input.pressed(input.Key.Escape) {
            window.close()
        }
        let dx = input.axis(input.Key.Left, input.Key.Right)
        draw.circle(input.mouse() + vec2(dx, 0), 5, draw.TRANSPARENT)
    }
    print(rollback.Input(buttons: rollback.SENT))
}
";
    let dir = project("modules_builtin_check", &[("main.kes", main)]);
    let out = kestrel(&["check"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_module_constant_can_use_a_constant_of_another_module() {
    // `palette` loads before `draw` here, and the order of the `use` lines doesn't matter.
    let palette = "use draw\npub let SKY = draw.BLUE\npub let FADED = draw.Color(0.5, 0.5, 1, a: 0.5)\n";
    let want = "Color(r: 0, g: 0, b: 1, a: 1) Color(r: 0.5, g: 0.5, b: 1, a: 0.5) Color(r: 0, g: 0, b: 1, a: 1)\n";
    for (name, uses) in [("modules_consts_palette_first", "use palette\nuse draw"), ("modules_consts_draw_first", "use draw\nuse palette")] {
        let main = format!("{uses}\nlet MINE = palette.SKY\nmain() {{\n    print(palette.SKY, palette.FADED, MINE)\n}}\n");
        let dir = project(name, &[("main.kes", &main), ("palette.kes", palette)]);
        let out = kestrel(&["run"], &dir.join("main.kes"));
        assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "{name}");
    }
}

#[test]
fn constants_of_plain_modules_may_use_each_other_in_any_order() {
    // Load order is main, a, b, c; `a.TOTAL` needs `b.WIDTH`, which needs `c.UNIT`.
    let a = "use b\npub let TOTAL = b.WIDTH * 3\n";
    let b = "use c\npub let WIDTH = c.UNIT + 1\n";
    let c = "pub let UNIT = 4\n";
    let main = "use a\nuse b\nuse c\nlet ALL = a.TOTAL + c.UNIT\nmain() {\n    print(a.TOTAL, b.WIDTH, c.UNIT, ALL)\n}\n";
    let dir = project("modules_consts_chain", &[("main.kes", main), ("a.kes", a), ("b.kes", b), ("c.kes", c)]);
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "15 5 4 19\n");
}

#[test]
fn constants_that_use_each_other_across_modules_are_a_named_error() {
    let a = "use b\npub let X = b.Y + 1\n";
    let b = "use a\npub let Y = a.X\n";
    let dir = project("modules_consts_cycle", &[("main.kes", "use a\nmain() {\n    print(a.X)\n}\n"), ("a.kes", a), ("b.kes", b)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1));
    // The error is where the loop closes: `a.X` in `b.kes`, line 2.
    let err = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
    assert_eq!(err, "2:13: error: constants can't use each other in a loop: a.X -> b.Y -> a.X; give one of them a value that doesn't use the others\n");
}

#[test]
fn a_module_may_reuse_the_names_of_the_programs_globals_for_its_locals() {
    // The module's own names are its items and its imports: the entry file's `r` and `k` are
    // not among them, so a parameter `r` and a local `k` hide nothing.
    let util = "pub pure helper(r: int) -> int {\n    let k = r + 1\n    k\n}\n";
    let main = "use { helper from util }\npure r(x: int) -> int { x }\nlet k = 5\nmain() {\n    print(helper(2), r(1), k)\n}\n";
    let dir = project("modules_local_names", &[("main.kes", main), ("util.kes", util)]);
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "3 1 5\n");
}

#[test]
fn a_local_named_like_an_item_of_its_own_file_is_still_refused() {
    let util = "pub pure helper(x: int) -> int {\n    let helper2 = x\n    helper2\n}\n\npub pure helper2() -> int { 1 }\n";
    let main = "use { helper from util }\npure r(x: int) -> int { x }\nmain() {\n    let r = 2\n    print(helper(r))\n}\n";
    let dir = project("modules_local_clash", &[("main.kes", main), ("util.kes", util)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
    let want = [
        "4:9: error: `r` is already defined in this file; Kestrel doesn't allow shadowing\n",
        "2:9: error: `helper2` is already defined in this file; Kestrel doesn't allow shadowing\n",
    ];
    for w in want {
        assert!(err.contains(w), "missing {w:?} in:\n{err}");
    }
    assert_eq!(err.lines().count(), 2, "{err}");
}

#[test]
fn a_module_cant_see_the_programs_own_names() {
    // `bump` and `secret` are the entry file's, not imported by `util`: they don't exist there.
    let util = "pub pure helper() -> int {\n    bump(secret)\n}\n";
    let main = "use { helper from util }\nlet secret = 41\npure bump(x: int) -> int { x + 1 }\nmain() {\n    print(helper())\n}\n";
    let dir = project("modules_entry_private", &[("main.kes", main), ("util.kes", util)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
    let want = [
        "2:5: error: unknown function `bump`: it's in main.kes, which `util` can't see\n",
        "2:10: error: unknown name `secret`: it's in main.kes, which `util` can't see\n",
    ];
    for w in want {
        assert!(err.contains(w), "missing {w:?} in:\n{err}");
    }
    assert_eq!(err.lines().count(), 2, "{err}");
}

#[test]
fn a_modules_function_name_is_a_value_in_the_importing_file() {
    let geom = "pub pure sq(x: int) -> int { x * x }
pub shout(x: int) { print(x) }
hidden(x: int) -> int { x }
";
    // The one error is the last line's, which shows what `geom.shout` is a value of.
    let main = "use geom
use { sq from geom }
                struct S {
    f: pure (int) -> int
}
                main() {
    let a: pure (int) -> int = geom.sq
    let b: (int) -> int = sq
    let s = S(geom.sq)
    let c: int = geom.shout
}
";
    let dir = project("modules_fn_values", &[("main.kes", main), ("geom.kes", geom)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    let err = String::from_utf8_lossy(&out.stderr);
    let errors: Vec<&str> = err.lines().filter(|l| l.contains(": error: ")).collect();
    assert_eq!(errors, ["10:18: error: expected int, found (int)"], "{err}");
    // The rules for names still hold: a function that isn't `pub` can't be named, as a value either.
    let bad = "use geom
main() {
    let f = geom.hidden
}
";
    let dir = project("modules_fn_values_hidden", &[("main.kes", bad), ("geom.kes", geom)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("3:18: error: `hidden` isn't `pub` in `geom`"), "{err}");
}

#[test]
fn a_modules_function_value_runs() {
    let util = "pub triple(x: int) -> int { scale(x) }
scale(x: int) -> int { x * 3 }
pub pure plus(a: int, b: int) -> int { a + b }
";
    let main = "use util
use { plus from util }

main() {
    let h = util.triple
    print(h(2))
    let fs = [plus, plus]
    print(fs[1](4, 5))
}
";
    let dir = project("modules_fn_values_run", &[("main.kes", main), ("util.kes", util)]);
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "6\n9\n");
}

#[test]
fn a_type_can_list_an_interface_from_another_module() {
    let shapes = "pub interface Shape {
    pure area(self) -> int
}

pub struct Square: Shape {
    side: int
    pure area(self) -> int { self.side * self.side }
}
";
    let main = "use { Shape, Square from shapes }
struct Rect: Shape {
    w: int
    h: int
    pure area(self) -> int { self.w * self.h }
}
main() {
    print(Square(3).area(), Rect(2, 5).area())
}
";
    let dir = project("modules_iface", &[("main.kes", main), ("shapes.kes", shapes)]);
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "9 10\n");
    // A module that doesn't import the interface can't list it.
    let main = "use shapes\nstruct Rect: Shape {\n    w: int\n    pure area(self) -> int { self.w }\n}\nmain() { }\n";
    let dir = project("modules_iface_unseen", &[("main.kes", main), ("shapes.kes", shapes)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("`Shape` isn't an interface"));
}

#[test]
fn an_interface_from_another_module_is_called_through_its_tables() {
    let shapes = "pub interface Shape {
    pure area(self) -> int
}

pub struct Square: Shape {
    side: int
    pure area(self) -> int { self.side * self.side }
}

pub pure total(shapes: list[Shape]) -> int {
    var n = 0
    for s in shapes {
        n += s.area()
    }
    n
}
";
    let main = "use { Shape, Square, total from shapes }
struct Rect: Shape {
    w: int
    h: int
    pure area(self) -> int { self.w * self.h }
}
pure measure(s: Shape) -> int { s.area() }
main() {
    var all: list[Shape] = [Square(3), Rect(2, 5)]
    all.add(Square(1))
    let r = Rect(1, 4)
    print(total(all), measure(r), measure(Square(2)))
    print(all[1])
}
";
    let dir = project("modules_iface_values", &[("main.kes", main), ("shapes.kes", shapes)]);
    let expected = "20 4 4\nRect(w: 2, h: 5)\n";
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
    // The tables hold function addresses, which the object file relocates too.
    let exe = dir.join("main").with_extension(std::env::consts::EXE_EXTENSION);
    let built = common::hermetic(Command::new(env!("CARGO_BIN_EXE_kestrel")).arg("build").arg(dir.join("main.kes")).arg("-o").arg(&exe)).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = common::hermetic(&mut Command::new(&exe)).output().unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    assert_eq!(String::from_utf8_lossy(&ran.stdout), expected);
}
