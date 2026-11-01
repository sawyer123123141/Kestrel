//! The `file` module (`driver/std/file.kes`), run through `kestrel run` (the JIT) and through a
//! built executable. Each test has scratch folders of its own under the target folder's scratch
//! space: the JIT run's program sits in one, so its relative paths land beside it, and the
//! executable sits in another, so its relative paths land beside the executable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

use common::hermetic;

/// A fresh, empty folder `name` under the scratch space. Tests run in parallel, so each one uses
/// its own names; the folder is cleared first.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("file_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A `kestrel` command with a clean environment (and freed memory poisoned, see `hermetic`).
fn kestrel() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(&mut cmd);
    cmd
}

fn text(out: &Output) -> (String, String) {
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Runs `cmd`, which must exit 0, and gives its standard output.
fn ok(cmd: &mut Command, what: &str) -> String {
    let out = cmd.output().unwrap();
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{what}: exit {:?}\n{stdout}\n{stderr}", out.status.code());
    stdout
}

/// The two folders a test's program runs in: one beside the `.kes` file (`kestrel run`) and one
/// beside the built executable.
struct Runs {
    jit: PathBuf,
    exe: PathBuf,
}

impl Runs {
    fn new(name: &str) -> Runs {
        Runs { jit: scratch(&format!("{name}_jit")), exe: scratch(&format!("{name}_exe")) }
    }

    /// Does `setup` in both folders, so each run starts from the same files.
    fn each(&self, setup: impl Fn(&Path)) -> &Self {
        setup(&self.jit);
        setup(&self.exe);
        self
    }

    /// Runs `src` with `kestrel run` and as a built executable, from the test's own working
    /// directory (so a relative path that resolved against it would miss), and gives both outputs.
    fn run(&self, src: &str) -> (String, String) {
        let program = self.jit.join("prog.kes");
        fs::write(&program, src).unwrap();
        let jit = ok(kestrel().arg("run").arg(&program), "kestrel run");
        let built = self.exe.join(format!("prog{}", std::env::consts::EXE_SUFFIX));
        // The source goes in a folder of its own, so the executable's folder holds only what it writes.
        let source = scratch(&format!("{}_src", self.exe.file_name().unwrap().to_string_lossy()));
        fs::write(source.join("prog.kes"), src).unwrap();
        ok(kestrel().arg("build").arg(source.join("prog.kes")).arg("-o").arg(&built), "kestrel build");
        let mut exe = Command::new(&built);
        hermetic(&mut exe);
        let exe = ok(&mut exe, "the built program");
        (jit, exe)
    }

    /// `run`, with both outputs equal to `expected`.
    fn expect(&self, src: &str, expected: &str) {
        let (jit, exe) = self.run(src);
        assert_eq!(jit, expected, "kestrel run");
        assert_eq!(exe, expected, "the built program");
    }

    /// The bytes of file `name` in both folders, which must be equal.
    fn file(&self, name: &str) -> Vec<u8> {
        let (jit, exe) = (fs::read(self.jit.join(name)).unwrap(), fs::read(self.exe.join(name)).unwrap());
        assert_eq!(jit, exe, "{name} differs between the runs");
        jit
    }
}

#[test]
fn write_then_read_round_trips() {
    let runs = Runs::new("round_trip");
    // The read's path is interpolated, and its result is used after the call and after more
    // strings are made: the result lives in the caller's arena, never in one freed at the call.
    let src = r#"use file
main() {
    print(file.write("data0.bin", [1, 2, 3]), file.error() == "")
    let n = 0
    let bytes = file.read("data{n}.bin") else {
        print("none: {file.error()}")
        return
    }
    var noise = list[string]()
    for i from 0 to 100 {
        noise.add("noise {i}")
    }
    print(bytes.len, noise.len)
    for b in bytes {
        print(b)
    }
    print(file.write_text("note.txt", "héllo {n}"))
    let note = file.read_text("note.txt") else { return }
    print(note, note.len)
}
"#;
    runs.expect(src, "true true\n3 100\n1\n2\n3\ntrue\nhéllo 0 7\n");
    assert_eq!(runs.file("data0.bin"), [1, 2, 3]);
    assert_eq!(runs.file("note.txt"), "héllo 0".as_bytes());
}

#[test]
fn interpolated_paths_and_a_list_built_by_add() {
    let runs = Runs::new("interpolated");
    // Every path is interpolated, and the text and names are used after more strings are made:
    // like `read`'s, their results live in the caller's arena. `write` takes a `list[u8]` built
    // by `add`, not only an array literal.
    let src = r#"use file
main() {
    let n = 1
    var bytes: list[u8] = []
    for i from 0 to 5 {
        bytes.add(u8(i * 10))
    }
    print(file.write("data{n}.bin", bytes))
    print(file.write_text("note{n}.txt", "hi {n}"), file.make_dir("sub{n}"), file.write_text("sub{n}/x{n}.txt", ""))
    let note = file.read_text("note{n}.txt") else { return }
    let names = file.list_dir("sub{n}") else { return }
    var noise = list[string]()
    for i from 0 to 100 {
        noise.add("noise {i}")
    }
    print(note, note.len, names.join(" "), noise.len)
    let back = file.read("data{n}.bin") else { return }
    print(back)
}
"#;
    runs.expect(src, "true
true true true
hi 1 4 x1.txt 100
[0, 10, 20, 30, 40]
");
    assert_eq!(runs.file("data1.bin"), [0, 10, 20, 30, 40]);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_window_program_that_uses_file_builds() {
    // An exe that uses `window` links the window library, which must carry the runtime's natives
    // too (`file` here): the link fails on a missing symbol. It is built, not run.
    let dir = scratch("window_build");
    let src = "use window
use file
main() {
    window.open(64, 64, \"file\")
    while window.frame() {
        print(file.exists(\"x.txt\"))
    }
}
";
    fs::write(dir.join("prog.kes"), src).unwrap();
    let built = dir.join(format!("prog{}", std::env::consts::EXE_SUFFIX));
    ok(kestrel().arg("build").arg(dir.join("prog.kes")).arg("-o").arg(&built), "kestrel build");
    assert!(built.is_file(), "no executable at {}", built.display());
}

#[test]
fn a_write_replaces_the_whole_file() {
    let runs = Runs::new("replace");
    runs.each(|dir| fs::write(dir.join("old.bin"), [9; 100]).unwrap());
    runs.expect("use file\nmain() {\n    print(file.write(\"old.bin\", [7]))\n}\n", "true\n");
    assert_eq!(runs.file("old.bin"), [7]);
}

#[test]
fn read_text_rejects_invalid_utf8() {
    let runs = Runs::new("utf8");
    runs.each(|dir| {
        fs::write(dir.join("bad.txt"), [b'a', 0xFF, 0xFE]).unwrap();
        fs::write(dir.join("good.txt"), "ok ✓").unwrap();
    });
    let src = r#"use file
show(path: string) {
    let t = file.read_text(path) else {
        print("none: {file.error()}")
        return
    }
    print("text: {t} error: [{file.error()}]")
}
main() {
    show("good.txt")
    show("bad.txt")
    let bytes = file.read("bad.txt") else { return }
    print(bytes.len, file.error() == "")
}
"#;
    runs.expect(src, "text: ok ✓ error: []\nnone: could not read `bad.txt`: it is not UTF-8 text\n3 true\n");
}

#[test]
fn exists_and_list_sorted() {
    let runs = Runs::new("list");
    let src = r#"use file
main() {
    print(file.exists("a.txt"))
    print(file.write_text("b.txt", "b"), file.write_text("a.txt", "a"), file.make_dir("sub"))
    print(file.exists("a.txt"), file.exists("sub"), file.exists("nope.txt"))
    let names = file.list_dir("sub") else { return }
    print(names.len)
    print(file.write_text("sub/Z.txt", ""), file.write_text("sub/a.txt", ""), file.write_text("sub/b b.txt", ""))
    let filled = file.list_dir("sub") else { return }
    print(filled.join(" | "))
}
"#;
    runs.expect(src, "false\ntrue true true\ntrue true false\n0\ntrue true true\nZ.txt | a.txt | b b.txt\n");
}

#[test]
fn list_of_the_programs_own_folder() {
    // `.` is the folder the program's relative paths resolve in: the program's or the executable's.
    let runs = Runs::new("list_dot");
    let src = r#"use file
main() {
    print(file.write_text("b.txt", "b"), file.write_text("a.txt", "a"))
    let names = file.list_dir(".") else { return }
    for name in names {
        if name.ends_with(".txt") {
            print(name)
        }
    }
}
"#;
    runs.expect(src, "true true\na.txt\nb.txt\n");
}

#[test]
fn make_dir_creates_parents() {
    let runs = Runs::new("make_dir");
    let src = r#"use file
main() {
    print(file.make_dir("a/b/c"), file.make_dir("a/b/c"), file.exists("a/b"))
    print(file.write("a/b/c/d.bin", [5]))
}
"#;
    runs.expect(src, "true true true\ntrue\n");
    assert_eq!(runs.file("a/b/c/d.bin"), [5]);
}

#[test]
fn remove_a_file_not_a_directory() {
    let runs = Runs::new("remove");
    let src = r#"use file
main() {
    print(file.write_text("gone.txt", "x"), file.make_dir("keep"))
    print(file.remove("gone.txt"), file.exists("gone.txt"), file.error() == "")
    print(file.remove("keep"))
    // `exists` never fails, so it empties the error: read the error first.
    print(file.error())
    print(file.exists("keep"), file.error() == "")
    print(file.remove("gone.txt"), file.error().starts_with("could not remove `gone.txt`: "))
}
"#;
    runs.expect(
        src,
        "true true\ntrue false true\nfalse\ncould not remove `keep`: it is a folder, and `file.remove` removes only files\ntrue true\nfalse true\n",
    );
}

#[test]
fn errors_name_the_path() {
    let runs = Runs::new("errors");
    runs.each(|dir| {
        fs::create_dir_all(dir.join("folder")).unwrap();
        fs::write(dir.join("plain.txt"), "x").unwrap();
    });
    // Each failure gives `false` or `none`, and a success afterwards clears the error.
    let src = r#"use file
say(what: string) {
    let e = file.error()
    print(what, e.starts_with("could not "), e.has("`missing/x.bin`") or e.has("`folder`"), e.ends_with("."), e == "")
}
main() {
    print(file.write("missing/x.bin", [1]))
    say("write")
    let a = file.read("folder") else {
        say("read")
        return
    }
}
"#;
    runs.expect(src, "false\nwrite true true false false\nread true true false false\n");
    let src = r#"use file
show(what: string, ok: bool) {
    print(what, ok, file.error())
}
main() {
    let names = file.list_dir("plain.txt") else {
        show("list", false)
        print(file.exists("plain.txt"), file.error() == "")
        let missing = file.read_text("nope.bin") else {
            show("read_text", false)
            print(file.make_dir("plain.txt"))
            print(file.error().starts_with("could not create `plain.txt`: "))
            return
        }
        return
    }
}
"#;
    let (jit, exe) = runs.run(src);
    assert_eq!(jit, exe);
    let lines: Vec<&str> = jit.lines().collect();
    assert_eq!(lines.len(), 5, "{jit}");
    assert!(lines[0].starts_with("list false could not list `plain.txt`: "), "{jit}");
    assert_eq!(lines[1], "true true");
    assert!(lines[2].starts_with("read_text false could not open `nope.bin`: "), "{jit}");
    assert_eq!(lines[3], "false");
    assert_eq!(lines[4], "true");
    for line in [lines[0], lines[2]] {
        let os = line.split(": ").nth(1).unwrap();
        assert!(os.starts_with(|c: char| !c.is_uppercase()), "the OS message starts in lower case: {line}");
        assert!(!os.ends_with('.') && !os.contains("os error"), "no trailing period or error number: {line}");
    }
}
