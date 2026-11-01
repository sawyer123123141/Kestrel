//! Modules in folders: `use net.packet` loads `net/packet.kes` beside the using file, `use net`
//! loads `net.kes` or `net/net.kes`, and full names work in code and in types (the rollback
//! library design, decisions 12 to 16).
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

/// A fresh directory holding `files` (a path with `/` from the directory, and its text), with
/// the folders they need.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("folders_{name}"));
    let _ = fs::remove_dir_all(&dir);
    for (file, text) in files {
        let path = file.split('/').fold(dir.clone(), |p, part| p.join(part));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    dir
}

fn kestrel(args: &[&str], file: &Path) -> Output {
    common::hermetic(Command::new(env!("CARGO_BIN_EXE_kestrel")).args(args).arg(file)).output().unwrap()
}

fn run_ok(dir: &Path) -> String {
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `kestrel check --diagnostics=short`, which must fail: its stderr.
fn errors(dir: &Path) -> String {
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    let err = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
    assert_eq!(out.status.code(), Some(1), "{err}");
    err
}

/// `kestrel check` with full diagnostics, which must fail: its stderr.
fn full_errors(dir: &Path) -> String {
    let out = kestrel(&["check"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1));
    String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n")
}

const PACKET: &str = "pub struct Packet {
    n: int
}

pub let K = 3

pub make(n: int) -> Packet {
    Packet(n: n)
}

pub pure at(xs: [int], i: int) -> int {
    xs[i]
}
";

#[test]
fn a_dotted_use_reaches_a_module_in_a_folder() {
    let main = "use net.packet
use { Packet, make from net.packet }

main() {
    let a = packet.make(1)
    let b = net.packet.make(2)
    let c = make(3)
    let d = packet.Packet(n: 4)
    let e: Packet = net.packet.Packet(n: 5)
    print(a, b.n, c.n, d.n, e.n, packet.K, net.packet.K)
}
";
    let dir = project("dotted", &[("main.kes", main), ("net/packet.kes", PACKET)]);
    assert_eq!(run_ok(&dir), "Packet(n: 1) 2 3 4 5 3 3\n");
}

const GEOM: &str = "pub struct Point {
    x: int
    y: int
}

pub struct Stack[T] {
    top: T
}

pub interface Named {
    pure name(self) -> string
}
";

#[test]
fn full_names_work_in_type_positions() {
    let main = "use net.packet
use geom

struct Holder: geom.Named {
    p: net.packet.Packet
    q: geom.Point?
    pure name(self) -> string { \"holder\" }
}

pure first(ps: list[geom.Point]) -> geom.Point {
    ps[0]
}

pure called[T: geom.Named](t: T) -> string {
    t.name()
}

main() {
    let p: net.packet.Packet = packet.make(7)
    let pts: list[geom.Point] = [geom.Point(x: 1, y: 2)]
    let h = Holder(p: p, q: none)
    let s: geom.Stack[int] = geom.Stack[int](top: 9)
    let o: geom.Point? = first(pts)
    let got = o else { return }
    let all: [net.packet.Packet] = [p]
    let back = from_bytes[net.packet.Packet](to_bytes(p)) else { return }
    print(h.p.n, got.y, s.top, called(h), all[0].n, back)
}
";
    let dir = project("types", &[("main.kes", main), ("net/packet.kes", PACKET), ("geom.kes", GEOM)]);
    assert_eq!(run_ok(&dir), "7 2 9 holder 7 Packet(n: 7)\n");
}

#[test]
fn a_folders_own_file_is_the_module_named_like_the_folder() {
    // `use net` finds `net/net.kes` when there's no `net.kes`, and `net/net.kes` finds `packet`
    // beside it; the entry reaches the same file as `net.packet`.
    let net = "use packet\n\npub total() -> int {\n    packet.K * 2\n}\n";
    let main = "use net\nuse net.packet\n\nmain() {\n    print(net.total(), packet.K)\n}\n";
    let dir = project("own_file", &[("main.kes", main), ("net/net.kes", net), ("net/packet.kes", PACKET)]);
    assert_eq!(run_ok(&dir), "6 3\n");
}

#[test]
fn one_file_reached_by_two_names_is_one_module() {
    // Review Focus 1: `wire` is `net.wire` from the entry and `wire` from `net/packet.kes`. Were it
    // loaded twice, its two `Signal`s would be two types and `relay` couldn't take the one
    // `packet.send` gives.
    let wire = "pub struct Signal {\n    v: int\n}\n\npub make(v: int) -> Signal {\n    Signal(v: v)\n}\n";
    let packet = "use wire\n\npub send(v: int) -> wire.Signal {\n    wire.make(v + 1)\n}\n";
    let main = "use net.wire\nuse net.packet\n\nrelay(s: net.wire.Signal) -> int {\n    s.v\n}\n\nmain() {\n    print(relay(packet.send(1)), relay(wire.make(5)))\n}\n";
    let dir = project("one_file", &[("main.kes", main), ("net/wire.kes", wire), ("net/packet.kes", packet)]);
    assert_eq!(run_ok(&dir), "2 5\n");
    // On a disk that ignores case, `Net.packet` and `net.packet` are one file too.
    if cfg!(windows) {
        let main = "use net.packet\nuse { Packet from Net.packet }\n\nmain() {\n    let p: Packet = packet.make(4)\n    print(p.n)\n}\n";
        let dir = project("one_file_case", &[("main.kes", main), ("net/packet.kes", PACKET)]);
        assert_eq!(run_ok(&dir), "4\n");
    }
}

#[test]
fn finding_a_module_says_what_is_wrong() {
    let both = project("could_be", &[("main.kes", "use net\nmain() {}\n"), ("net.kes", "pub f() {}\n"), ("net/net.kes", "pub f() {}\n")]);
    assert_eq!(errors(&both), "1:5: error: `net` could be net.kes or net/net.kes; remove or rename one of them\n");
    let empty = project("empty_folder", &[("main.kes", "use net\nmain() {}\n"), ("net/packet.kes", PACKET)]);
    let want = "1:5: error: can't find module `net`: the folder net/ has no net.kes; add one, or use a module inside it, like `use net.packet`\n";
    assert_eq!(errors(&empty), want);
    let missing = project("missing_dotted", &[("main.kes", "use net.packet\nmain() {}\n")]);
    assert_eq!(errors(&missing), "1:5: error: can't find module `net.packet`: there's no net/packet.kes next to this file\n");
    let flat = project("missing_flat", &[("main.kes", "use geom\nmain() {}\n")]);
    assert_eq!(errors(&flat), "1:5: error: can't find module `geom`: there's no `geom.kes` next to this file\n");
    // A module in a folder never finds one above it.
    let above = project("above", &[("main.kes", "use net.packet\nmain() {}\n"), ("game.kes", "pub f() {}\n"), ("net/packet.kes", "use game\n")]);
    let err = full_errors(&above);
    assert!(err.contains("error: can't find module `game`: there's no `game.kes` next to this file\n"), "{err}");
    assert!(err.contains("a module finds modules next to it or in folders below it, never above"), "{err}");
    // Only when there is one above: a misspelt name gets no such help.
    let typo = project("typo_below", &[("main.kes", "use net.packet\nmain() {}\n"), ("game.kes", "pub f() {}\n"), ("net/packet.kes", "use gmae\n")]);
    let err = full_errors(&typo);
    assert!(err.contains("error: can't find module `gmae`: there's no `gmae.kes` next to this file\n"), "{err}");
    assert!(!err.contains("never above"), "{err}");
    // A dotted name is never a built-in module.
    let folder = project("builtin_folder", &[("main.kes", "use draw.shapes\nmain() {}\n"), ("draw/shapes.kes", "pub f() {}\n")]);
    assert_eq!(errors(&folder), "1:5: error: `draw` is a built-in module, so a folder can't be called draw; rename the folder\n");
}

#[test]
fn names_from_folders_say_what_is_wrong() {
    // The first name of a full name is taken in the file that uses it.
    let main = "use net.packet\n\nnet() -> int { 1 }\n\nmain() {}\n";
    let dir = project("first_name_item", &[("main.kes", main), ("net/packet.kes", PACKET)]);
    assert_eq!(errors(&dir), "1:5: error: `net` is already defined here\n");
    let main = "use net.packet\n\nmain() {\n    let net = 2\n}\n";
    let dir = project("first_name_local", &[("main.kes", main), ("net/packet.kes", PACKET)]);
    assert_eq!(errors(&dir), "4:9: error: `net` is already defined: it's imported; pick another name\n");
    // Two modules with one last name: neither short name, both full names.
    let ui = "pub make(n: int) -> int {\n    n * 10\n}\n";
    let main = "use net.packet\nuse ui.packet\n\nmain() {\n    print(packet.make(1))\n}\n";
    let dir = project("two_packets", &[("main.kes", main), ("net/packet.kes", PACKET), ("ui/packet.kes", ui)]);
    let want = "5:11: error: `packet` could be `net.packet` or `ui.packet`; write the full name, like `net.packet.make`\n";
    assert_eq!(errors(&dir), want);
    let main = "use net.packet\nuse ui.packet\n\nmain() {\n    print(net.packet.make(1).n, ui.packet.make(1))\n}\n";
    let dir = project("two_packets_full", &[("main.kes", main), ("net/packet.kes", PACKET), ("ui/packet.kes", ui)]);
    assert_eq!(run_ok(&dir), "1 10\n");
    // A single name keeps its short name; the dotted one is reached by its full name.
    let main = "use packet\nuse net.packet\n\nmain() {\n    print(packet.make(2), net.packet.make(2).n)\n}\n";
    let dir = project("single_keeps", &[("main.kes", main), ("packet.kes", ui), ("net/packet.kes", PACKET)]);
    assert_eq!(run_ok(&dir), "20 2\n");
    // A module and an item with one full name.
    let net = "pub packet() -> int {\n    1\n}\n";
    let main = "use net\nuse net.packet\n\nmain() {}\n";
    let dir = project("module_and_item", &[("main.kes", main), ("net.kes", net), ("net/packet.kes", PACKET)]);
    let want = "1:5: error: `net.packet` is both the module net/packet.kes and `packet` in net.kes; rename one of them\n";
    assert_eq!(errors(&dir), want);
}

#[test]
fn a_module_and_an_item_with_one_full_name_are_refused_across_files() {
    // No file uses both: `main` uses `net`, `other` uses `net.packet`. The method `make` of the
    // struct `net.packet` and the function `make` of module `net.packet` would link to one name.
    let net = "struct packet {\n    n: int\n\n    make(self) -> int { self.n }\n}\n\npub mk() -> int {\n    packet(n: 1).make()\n}\n";
    let other = "use net.packet\n\npub f() -> int {\n    packet.make(2).n\n}\n";
    let main = "use net\nuse other\n\nmain() {\n    print(net.mk(), other.f())\n}\n";
    let files = [("main.kes", main), ("other.kes", other), ("net.kes", net), ("net/packet.kes", PACKET)];
    let dir = project("module_and_item_apart", &files);
    let want = "1:8: error: `net.packet` is both the module net/packet.kes and `packet` in net.kes; rename one of them\n";
    assert_eq!(errors(&dir), want);
}

#[test]
fn a_folders_own_file_has_one_full_name_however_it_is_reached() {
    // `net/y.kes` reaches `net/net.kes` as `net`, beside it: its full name is still `net`.
    let y = "use net\n\npub f() {\n    net.nothing()\n}\n";
    let main = "use net.y\n\nmain() {\n    y.f()\n}\n";
    let dir = project("own_file_name", &[("main.kes", main), ("net/y.kes", y), ("net/net.kes", "pub g() {}\n")]);
    assert_eq!(errors(&dir), "4:9: error: `net` has no `nothing`\n");
}

#[test]
fn a_type_below_the_entry_may_name_a_module_by_its_full_name() {
    let x = "use packet\n\npub f() -> int {\n    let p: net.packet.Packet = packet.make(6)\n    p.n\n}\n";
    let main = "use net.x\n\nmain() {\n    print(x.f())\n}\n";
    let dir = project("full_type_below", &[("main.kes", main), ("net/x.kes", x), ("net/packet.kes", PACKET)]);
    assert_eq!(run_ok(&dir), "6\n");
}

#[test]
fn the_report_keeps_another_modules_name_that_starts_like_this_ones() {
    // In `net.kes`, `net.packet.P` is module `net.packet`'s, not `packet.P` of `net`.
    let net = "pub struct Stack[T] {\n    items: list[T] = []\n\n    put(var self, v: T) { self.items.add(v) }\n}\n";
    let main = "use net\nuse net.packet\n\nmain() {\n    var s = net.Stack[net.packet.P]()\n    s.put(packet.P(n: 1))\n    print(s.items.len)\n}\n";
    let dir = project("report_longer", &[("main.kes", main), ("net.kes", net), ("net/packet.kes", "pub struct P {\n    n: int\n}\n")]);
    let out = kestrel(&["check", "--report"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("`Stack[net.packet.P].put`"), "{text}");
    assert!(!text.contains("Stack[packet.P]"), "{text}");
}

#[test]
fn a_type_from_a_module_the_file_doesnt_use_says_how_to_reach_it() {
    let main = "main() {\n    let p: geo.Point? = none\n}\n";
    let dir = project("unknown_prefix", &[("main.kes", main)]);
    let want = "2:12: error: `geo` isn't a module this file uses; add `use geo`, or import the type: `use { Point from geo }`\n";
    assert_eq!(errors(&dir), want);
}

#[test]
fn a_folder_module_cant_see_the_programs_names() {
    let packet = "pub peek() -> int {\n    secret\n}\n";
    let main = "use net.packet\nlet secret = 41\nmain() {\n    print(packet.peek())\n}\n";
    let dir = project("unseen", &[("main.kes", main), ("net/packet.kes", packet)]);
    assert_eq!(errors(&dir), "2:5: error: unknown name `secret`: it's in main.kes, which `net.packet` can't see\n");
}

#[test]
fn a_runtime_error_in_a_folder_names_its_path_from_the_program() {
    let main = "use net.packet\nmain() {\n    print(packet.at([1, 2], 5))\n}\n";
    let dir = project("runtime", &[("main.kes", main), ("net/packet.kes", PACKET)]);
    let want = "index 5 out of bounds for length 2 on line 12 of net/packet.kes";
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(101));
    assert!(String::from_utf8_lossy(&out.stderr).contains(want), "{}", String::from_utf8_lossy(&out.stderr));
    // A built program prints the same.
    let exe = dir.join("folders_runtime.exe");
    let built = kestrel(&["build", "-o", exe.to_str().unwrap()], &dir.join("main.kes"));
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = Command::new(&exe).output().expect("run the built program");
    assert_eq!(run.status.code(), Some(101));
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains(want), "{err}");
    assert!(!err.contains(dir.to_str().unwrap()), "the built program mustn't carry the folder: {err}");
}

#[test]
fn a_built_folder_program_prints_what_run_prints() {
    let main = "use net.packet\nuse geom\n\nmain() {\n    let p: geom.Point = geom.Point(x: 1, y: packet.K)\n    print(p, packet.make(8))\n}\n";
    let dir = project("built", &[("main.kes", main), ("net/packet.kes", PACKET), ("geom.kes", GEOM)]);
    let ran = run_ok(&dir);
    assert_eq!(ran, "Point(x: 1, y: 3) Packet(n: 8)\n");
    let exe = dir.join("folders_built.exe");
    let built = kestrel(&["build", "-o", exe.to_str().unwrap()], &dir.join("main.kes"));
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = Command::new(&exe).output().expect("run the built program");
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), ran);
}

#[test]
fn the_report_and_contracts_name_folder_files_from_the_program() {
    let packet = "pub helper(a: int, b: int) -> int {\n    a * b\n}\n";
    let main = "use { helper from net.packet }\n\npub density(x: int, y: int) -> int {\n    helper(x, y) + y\n}\n\nmain() {}\n";
    let dir = project("report", &[("main.kes", main), ("net/packet.kes", packet)]);
    let out = kestrel(&["check", "--report"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("    net/packet.kes:2 (in `helper`): overflow check: `a * b` may exceed `int`"), "{text}");
    // The function's plain name, without its module's full name.
    assert!(text.contains("net/packet.kes\n  function `helper` (line 1)"), "{text}");
    let stack = "pub struct Stack[T] {\n    items: list[T] = []\n\n    put(var self, i: int, v: T) { self.items[i] = v }\n}\n";
    let main = "use { Stack from net.stack }\n\n@prove(no_alloc)\nfill(var s: Stack[string]) { s.put(0, \"y\") }\n\nmain() {\n    var s = Stack[string](items: [\"x\"])\n    fill(var s)\n}\n";
    let dir = project("contract", &[("main.kes", main), ("net/stack.kes", stack)]);
    let out = kestrel(&["check", "--diagnostics=short"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1));
    let want = "3:8: error: `fill` can't be proven `no_alloc`: it allocates on net/stack.kes:4: makes a copy of `v`\n";
    assert_eq!(String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"), want);
}
