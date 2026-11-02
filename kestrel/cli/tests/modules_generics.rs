//! Generics declared in one file and used from another.
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

const GEOM: &str = "pub struct Stack[T] {
    items: list[T] = []
    push(var self, v: T) { self.items.add(v) }
}

pub enum Outcome[T, E] {
    Ok { value: T }
    Err { error: E }
}

pub struct Item {
    id: int
}

pub pure best[T](xs: list[T], score: pure (T) -> int) -> T? {
    var found: T? = none
    var top = 0
    for x in xs {
        if score(x) > top {
            found = x
            top = score(x)
        }
    }
    found
}

pub first() -> int {
    let pick: list[int] = [4, 5]
    pick[1]
}
";

fn run_ok(dir: &Path) -> String {
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn check_errors(dir: &Path) -> String {
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn generics_imported_both_ways() {
    let main = "use { Stack, best, Item from geom }
use geom

main() {
    var s = Stack[int]()
    s.push(1)
    var t = geom.Stack[int]()
    t.push(2)
    let items: list[Item] = [Item(id: 4), Item(id: 9)]
    let top = best(items, (i) { i.id }) else { return }
    let low = geom.best[Item](items, (i) { 10 - i.id }) else { return }
    let r = geom.Outcome[int, string].Ok(value: 3)
    print(s, t, top.id, low.id, r)
    print(Stack[Item]())
}
";
    let dir = project("modgen_both_ways", &[("main.kes", main), ("geom.kes", GEOM)]);
    assert_eq!(run_ok(&dir), "Stack[int](items: [1]) Stack[int](items: [2]) 9 4 Outcome[int, string].Ok(value: 3)\nStack[Item](items: [])\n");
}

#[test]
fn one_copy_for_two_modules() {
    let other = "use { Stack from geom }\npub fill() -> int {\n    var s = Stack[int]()\n    s.push(5)\n    s.items.len\n}\n";
    let main = "use { Stack from geom }\nuse { fill from other }\n\nmain() {\n    var s = Stack[int]()\n    s.push(1)\n    print(s, fill())\n}\n";
    let dir = project("modgen_one_copy", &[("main.kes", main), ("geom.kes", GEOM), ("other.kes", other)]);
    assert_eq!(run_ok(&dir), "Stack[int](items: [1]) 1\n");
    let (compiled, text) = kestrel_driver::compile_file(&dir.join("main.kes"), kestrel_driver::DiagStyle::Short, true);
    let compiled = compiled.unwrap_or_else(|| panic!("{text}"));
    // The generic's own stub (`geom.Stack.push`) stays; there is one copy for `int`, not one per module.
    let copies: Vec<&str> = compiled.module.funcs.iter().map(|f| f.name.as_str()).filter(|n| n.contains("push") && n.contains('[')).collect();
    assert_eq!(copies, ["geom.Stack[int].push"], "{copies:?}");
}

#[test]
fn module_names_in_messages() {
    let head = "use { Stack, Item from geom }\n";
    let two = format!("{head}bad(s: Stack[int, int]) {{}}\nmain() {{}}\n");
    let dir = project("modgen_msg_count", &[("main.kes", &two), ("geom.kes", GEOM)]);
    let err = check_errors(&dir);
    assert!(err.contains("`geom.Stack` takes 1 type, found 2"), "{err}");
    let opt = format!("{head}bad(s: Stack[Item?]) {{}}\nmain() {{}}\n");
    let dir = project("modgen_msg_optional", &[("main.kes", &opt), ("geom.kes", GEOM)]);
    let err = check_errors(&dir);
    assert!(err.contains("`geom.Item?` is an optional, so it can't be `T` yet; hold it in a struct field instead"), "{err}");
}

#[test]
fn a_local_named_like_an_item() {
    // `geom` has a local `pick`; `main.kes` has a generic `pick`. The local stays a local.
    let main = "use geom\npure pick[T](xs: list[T]) -> int { xs.len }\nmain() { print(geom.first()) }\n";
    let dir = project("modgen_local_named_like_item", &[("main.kes", main), ("geom.kes", GEOM)]);
    assert_eq!(run_ok(&dir), "5\n");
}

#[test]
fn copies_are_named_with_their_modules() {
    let main = "use { Stack, Item from geom }\nmain() {\n    var s = Stack[Item]()\n    s.push(Item(id: 1))\n    print(s)\n}\n";
    let dir = project("modgen_copy_names", &[("main.kes", main), ("geom.kes", GEOM)]);
    assert_eq!(run_ok(&dir), "Stack[Item](items: [Item(id: 1)])\n");
    let (compiled, text) = kestrel_driver::compile_file(&dir.join("main.kes"), kestrel_driver::DiagStyle::Short, true);
    let compiled = compiled.unwrap_or_else(|| panic!("{text}"));
    assert!(compiled.module.funcs.iter().any(|f| f.name == "geom.Stack[geom.Item].push"));
}

#[test]
fn a_placeholder_named_like_a_name_only_main_defines() {
    // `main.kes` has a constant `M` (G16), and `geom` uses `M` as a placeholder everywhere in its
    // bodies: a let annotation, a closure parameter, a type call, an instance (P37).
    let geom = "pub struct Wrap[M] {
    v: M
}

pub pure apply[M](x: M, f: pure (M) -> int) -> int {
    let y: M = x
    let g = (z: M) { f(z) + 1 }
    let w = Wrap[M](v: y)
    g(w.v)
}
";
    let main = "use { Wrap, apply from geom }\nlet M = 3\nmain() {\n    print(apply(M, (n) { n * 2 }), Wrap[string](v: \"a\"))\n}\n";
    let dir = project("modgen_placeholder_names", &[("main.kes", main), ("geom.kes", geom)]);
    assert_eq!(run_ok(&dir), "7 Wrap[string](v: \"a\")\n");
}

#[test]
fn rules_enums_and_private_helpers_across_modules() {
    let geom = "pub interface Damageable {
    pure hp(self) -> int
}

pub enum Maybe[T] {
    Some { value: T }
    Nothing
}

pub struct Pair[A, B] {
    a: A
    b: B
}

pub pure total[T: Damageable](xs: list[T]) -> int {
    var t = 0
    for x in xs { t += x.hp() }
    t + twice_it(1)
}

pure twice_it(n: int) -> int { n * 2 }

pub pure biggest[T: ordered](a: T, b: T) -> T {
    if a < b { b } else { a }
}

pub pure twice[T](x: T) -> Pair[T, T] { Pair[T, T](a: x, b: x) }

pub pure make[T]() -> Maybe[T] { Maybe[T].Nothing }
";
    let main = "use { Damageable, Maybe, total, biggest, twice from geom }
use geom

struct Orc: Damageable {
    life: int
    pure hp(self) -> int { self.life }
}

pure show(m: Maybe[int]) -> int {
    match m {
        Some s { s.value }
        Nothing { 0 }
    }
}

main() {
    print(total([Orc(life: 3), Orc(life: 4)]), biggest(2, 9), biggest(\"a\", \"b\"), twice(3))
    let m = geom.Maybe[int].Some(value: 7)
    let e: Maybe[string] = geom.make()
    print(show(m), show(Maybe[int].Nothing), e, geom.twice(m))
}
";
    let dir = project("modgen_rules_enums", &[("main.kes", main), ("geom.kes", geom)]);
    assert_eq!(run_ok(&dir), "9 9 b Pair[int, int](a: 3, b: 3)\n7 0 Maybe[string].Nothing Pair[Maybe[int], Maybe[int]](a: Maybe[int].Some(value: 7), b: Maybe[int].Some(value: 7))\n");
}

#[test]
fn more_messages_name_the_module() {
    let geom = "pub struct Stack[T] {\n    items: list[T] = []\n}\npub struct Item { id: int }\npub pure pick[T](a: T, b: T) -> T { a }\npub pure order[T: ordered](a: T) -> T { a }\npub pure nothing[T]() -> int { 0 }\n";
    let main = "use { Stack, Item, pick, order, nothing from geom }\nuse geom

main() {
    let a = pick(1, \"x\")
    let b = order(Item(id: 1))
    let c = nothing()
    let d: Stack = Stack[int]()
    let e = geom.pick
    print(a, b, c, d, e)
}
";
    let dir = project("modgen_more_messages", &[("main.kes", main), ("geom.kes", geom)]);
    let err = check_errors(&dir);
    for want in [
        "`T` in `geom.pick` can't be both `int` (from `a`) and `string` (from `b`)",
        "`geom.Item` isn't `ordered` (numbers, bools and strings are), so it can't be `T` in `geom.order`",
        "can't tell what `T` is in `geom.nothing`; write it, like `geom.nothing[int](...)`",
        "`geom.Stack` needs its types, like `geom.Stack[int]`",
        "`geom.pick` is generic; give its types to use it as a value, like `geom.pick[int]`",
    ] {
        assert!(err.contains(want), "missing {want:?} in:\n{err}");
    }
}

#[test]
fn endless_copies_in_a_library_are_refused() {
    let geom = "pub struct Nest[T] {\n    inner: Nest[list[T]]?\n}\n";
    let dir = project("modgen_endless", &[("main.kes", "use geom\nmain() {}\n"), ("geom.kes", geom)]);
    let err = check_errors(&dir);
    assert!(err.contains("`geom.Nest` would need endless copies: `geom.Nest[T]` holds a `geom.Nest[list[T]]`"), "{err}");
}

#[test]
fn two_modules_with_an_item_of_the_same_name() {
    // `a.Item` and `b.Item` print alike, but `Stack[a.Item]` and `Stack[b.Item]` are two instances.
    let lib = "pub struct Stack[T] {\n    items: list[T] = []\n    push(var self, v: T) { self.items.add(v) }\n}\npub pure pick[T](a: T) -> int { 10 }\n";
    let a = "use { Stack, pick from lib }\npub struct Item { id: int }\npub pure f() -> int { pick(Item(id: 1)) }\npub mk() -> Stack[Item] {\n    var s = Stack[Item]()\n    s.push(Item(id: 1))\n    s\n}\n";
    let b = "use { Stack, pick from lib }\npub struct Item { name: string }\npub pure f() -> int { pick(Item(name: \"q\")) }\npub mk() -> Stack[Item] {\n    var s = Stack[Item]()\n    s.push(Item(name: \"z\"))\n    s\n}\n";
    let main = "use a\nuse b\nuse { Stack from lib }\n\nmain() {\n    print(a.mk(), b.mk(), a.f(), b.f())\n    var s = Stack[int]()\n    s.push(2)\n    print(s)\n}\n";
    let dir = project("modgen_same_item_name", &[("main.kes", main), ("lib.kes", lib), ("a.kes", a), ("b.kes", b)]);
    assert_eq!(run_ok(&dir), "Stack[Item](items: [Item(id: 1)]) Stack[Item](items: [Item(name: \"z\")]) 10 10\nStack[int](items: [2])\n");
}

#[test]
fn the_built_in_result_is_one_type_in_every_module() {
    // Every module sees the prelude's `Result` without a `use`, with its own types inside the
    // brackets (`Item` there is `geom.Item`), and a `Result[int, string]` made in one module is the
    // type another writes.
    let geom = "pub struct Item {\n    id: int\n}\npub load(n: int) -> Result[Item, string] {\n    if n < 0 {\n        return Result.Err(error: \"negative\")\n    }\n    Item(id: n)\n}\npub two() -> Result[Item, string] {\n    Result[Item, string].Ok(value: Item(id: 2))\n}\npub pure label(r: Result[int, string]) -> string {\n    match r {\n        Ok o { \"ok {o.value}\" }\n        Err e { e.error }\n    }\n}\n";
    let main = "use geom\n\nmain() {\n    let r: Result[geom.Item, string] = geom.load(3)\n    print(r, geom.load(-1), geom.two())\n    let n: Result[int, string] = 4\n    print(geom.label(n), geom.label(Result.Err(error: \"no\")))\n}\n";
    let dir = project("modgen_prelude_result", &[("main.kes", main), ("geom.kes", geom)]);
    let want = "Result[Item, string].Ok(value: Item(id: 3)) Result[Item, string].Err(error: \"negative\") Result[Item, string].Ok(value: Item(id: 2))\nok 4 no\n";
    assert_eq!(run_ok(&dir), want);
}

#[test]
fn a_module_can_not_declare_its_own_result() {
    let geom = "pub enum Result {\n    A\n}\n";
    let dir = project("modgen_own_result", &[("main.kes", "use geom\nmain() {}\n"), ("geom.kes", geom)]);
    let err = check_errors(&dir);
    assert!(err.contains("`Result` is a built-in type; pick another name"), "{err}");
}
