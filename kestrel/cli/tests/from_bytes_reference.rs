//! `from_bytes[T](b)` reads what the reference encoder in `kestrel_runtime::bytes` writes, and on
//! damaged bytes it agrees with the reference decoder: an error exactly when `decode` is `None`,
//! and otherwise a value whose own bytes are the input. A length a hostile input claims is never
//! allocated, and the error names it.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

use common::hermetic;
use kestrel_runtime::bytes::{decode, encode, fingerprint, RefTy as T, RefValue as V};

const DECLS: &str = "enum Shape {
    Dot
    Label { text: string, size: u8 }
}

struct Unit {
}

struct Tagged {
    id: int
    k: Unit
}

struct P {
    a: int
    name: string
    xs: list[u8]
    o: i64?
    sh: Shape
    m: map[string, int]
    ok: bool
    os: list[int?]
    ls: list[list[u8]]
    t: list[Tagged]
}
";

const DESCRIPTION: &str = "struct P{a:int,name:string,xs:list[u8],o:i64?,sh:enum Shape{Dot,Label{text:string,size:u8}},m:map[string,int],ok:bool,os:list[int?],ls:list[list[u8]],t:list[struct Tagged{id:int,k:struct Unit{}}]}";

/// Writes `src` as a program in a fresh folder named for `name`.
fn write(name: &str, src: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("from_bytes_reference_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.kes");
    fs::write(&file, src).unwrap();
    file
}

/// Runs the program at `path` with `env` set; its stdout and stderr.
fn run(path: &Path, env: &[(&str, &str)]) -> (String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(cmd.arg("run").arg(path));
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{}: exit {:?}\n{stderr}", path.display(), out.status.code());
    (String::from_utf8_lossy(&out.stdout).to_string(), stderr)
}

/// `bytes` as a list literal.
fn literal(bytes: &[u8]) -> String {
    let items: Vec<String> = bytes.iter().map(u8::to_string).collect();
    format!("[{}]", items.join(", "))
}

fn ty() -> T {
    let shape = T::Enum(vec![vec![], vec![T::Str, T::U8]]);
    let list = |t: T| T::List(Box::new(t));
    // `os` and `ls` are read by growing (their items are far bigger in memory than in bytes), and
    // `t`'s items end in an empty struct.
    let tagged = T::Struct(vec![T::Int, T::Struct(vec![])]);
    let fields = vec![T::Int, T::Str, list(T::U8), T::Opt(Box::new(T::I64)), shape, T::Map(Box::new(T::Str), Box::new(T::Int)), T::Bool];
    T::Struct([fields, vec![list(T::Opt(Box::new(T::Int))), list(list(T::U8)), list(tagged)]].concat())
}

fn value() -> V {
    let s = |t: &str| V::Str(t.to_string());
    V::Struct(vec![
        V::Int(-7),
        s("h\u{e9}"),
        V::List(vec![V::U8(1), V::U8(250)]),
        V::Opt(Some(Box::new(V::I64(1 << 40)))),
        V::Enum(1, vec![s("ok"), V::U8(3)]),
        V::Map(vec![(s("b"), V::Int(2)), (s("a"), V::Int(1))]),
        V::Bool(true),
        V::List(vec![V::Opt(None), V::Opt(Some(Box::new(V::Int(5))))]),
        V::List(vec![V::List(vec![V::U8(9)]), V::List(vec![])]),
        V::List(vec![V::Struct(vec![V::Int(1), V::Struct(vec![])]), V::Struct(vec![V::Int(2), V::Struct(vec![])])]),
    ])
}

#[test]
fn from_bytes_agrees_with_the_reference_decoder() {
    let fp = fingerprint(DESCRIPTION);
    let good = encode(&value(), fp);
    // Every truncation, and 150 single-byte changes from a fixed-seed generator.
    let mut cases: Vec<Vec<u8>> = (0..good.len()).map(|n| good[..n].to_vec()).collect();
    cases.push(good.clone());
    let mut seed: u64 = 0xF00D;
    let mut next = |n: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) % n as u64) as usize
    };
    for _ in 0..150 {
        let mut bad = good.clone();
        let at = next(bad.len());
        bad[at] = next(256) as u8;
        cases.push(bad);
    }
    let calls: Vec<String> = cases.iter().map(|b| format!("    print(r({}))\n", literal(b))).collect();
    let src = format!(
        "{DECLS}
/// 0 when `b` isn't a `P`; 1 when it is and its bytes are `b`'s; 2 when they aren't.
r(b: list[u8]) -> int {{
    let y = from_bytes[P](b) else {{ return 0 }}
    let again = to_bytes(y)
    if again.len != b.len {{ return 2 }}
    for i from 0 to b.len {{
        if again[i] != b[i] {{ return 2 }}
    }}
    1
}}

main() {{
{}}}
",
        calls.concat()
    );
    let (stdout, _) = run(&write("fuzz", &src), &[]);
    let got: Vec<&str> = stdout.lines().collect();
    let want: Vec<&str> = cases.iter().map(|b| if decode(b, &ty(), fp).is_some() { "1" } else { "0" }).collect();
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "case {i}: {:?}", cases[i]);
    }
    // The undamaged bytes read, and enough damaged ones do to test both ways.
    assert_eq!(got[good.len()], "1");
    assert!(want.iter().filter(|w| **w == "1").count() > 10, "{want:?}");
}

/// The program arena's size when `main` returns (`KESTREL_ARENA_REPORT=1`), in bytes.
fn arena_bytes(path: &Path) -> u64 {
    let (_, stderr) = run(path, &[("KESTREL_ARENA_REPORT", "1")]);
    let line = stderr.lines().find_map(|l| l.strip_prefix("kestrel: program arena ")).unwrap_or_else(|| panic!("no arena report in stderr:\n{stderr}"));
    line.strip_suffix(" bytes").and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("can't read the arena report {line:?}"))
}

#[test]
fn a_length_the_bytes_cant_hold_is_never_allocated() {
    // 20 bytes: a header, then a list length of 2,147,483,647 (or 4,294,967,295) and 3 bytes.
    let header = |desc: &str| encode(&V::List(Vec::new()), fingerprint(desc))[..13].to_vec();
    let claim = |desc: &str, count: [u8; 4]| [header(desc), count.to_vec(), vec![0, 0, 0]].concat();
    let reads = [
        ("list[u8]", "list[u8]", [0xFF, 0xFF, 0xFF, 0x7F]),
        ("list[u8]", "list[u8]", [0xFF, 0xFF, 0xFF, 0xFF]),
        ("list[list[int]]", "list[list[int]]", [0xFF, 0xFF, 0xFF, 0x7F]),
        ("[f64]", "array[f64]", [0xFF, 0xFF, 0xFF, 0x7F]),
        ("map[int, string]", "map[int,string]", [0xFF, 0xFF, 0xFF, 0x7F]),
        ("string", "string", [0xFF, 0xFF, 0xFF, 0x7F]),
    ];
    let program = |read: bool| {
        let mut body = String::new();
        for (i, (ty, desc, count)) in reads.iter().enumerate() {
            body.push_str(&format!("    let b{i}: list[u8] = {}\n", literal(&claim(desc, *count))));
            if read {
                body.push_str(&format!("    match from_bytes[{ty}](b{i}) {{\n        Ok {{ print(\"read\") }}\n        Err e {{ print(e.error) }}\n    }}\n"));
            } else {
                body.push_str(&format!("    print(b{i}.len)\n"));
            }
        }
        format!("main() {{\n{body}}}\n")
    };
    let read = write("claims", &program(true));
    let (stdout, _) = run(&read, &[]);
    assert_eq!(stdout.lines().collect::<Vec<_>>(), vec!["the length at byte 13 is more than the bytes left can hold"; reads.len()]);
    let with = arena_bytes(&read);
    let without = arena_bytes(&write("claims_control", &program(false)));
    assert!(with <= without + 4096, "reading the claims grew the arena from {without} to {with} bytes");
}

#[test]
fn items_far_bigger_in_memory_than_in_bytes_are_not_made_up_front() {
    // A `vec4?` takes 32 bytes in memory and can be written in 1, so room for every item a length
    // claims would be 32 times the input. Here a 40-byte input claims 4,294,967,295 items, and
    // 2,000-byte ones claim as many items as their bytes could hold, each item a byte that is not
    // 0 or 1 or a length past `i32::MAX` (so the first item already fails), for lists and an array.
    let header = |desc: &str| encode(&V::List(Vec::new()), fingerprint(desc))[..13].to_vec();
    let claim = |desc: &str, count: u32, len: usize| {
        let mut b = [header(desc), count.to_le_bytes().to_vec()].concat();
        b.resize(len, 0xFF);
        b
    };
    let reads = [
        ("list[vec4?]", claim("list[vec4?]", u32::MAX, 40)),
        ("list[vec4?]", claim("list[vec4?]", 2000 - 17, 2000)),
        ("[vec4?]", claim("array[vec4?]", 2000 - 17, 2000)),
        ("list[list[u8]]", claim("list[list[u8]]", (2000 - 17) / 4, 2000)),
    ];
    let program = |read: bool| {
        let mut body = String::new();
        for (i, (ty, bytes)) in reads.iter().enumerate() {
            body.push_str(&format!("    let b{i}: list[u8] = {}\n", literal(bytes)));
            if read {
                body.push_str(&format!("    match from_bytes[{ty}](b{i}) {{\n        Ok {{ print(\"read\") }}\n        Err e {{ print(e.error) }}\n    }}\n"));
            } else {
                body.push_str(&format!("    print(b{i}.len)\n"));
            }
        }
        format!("main() {{\n{body}}}\n")
    };
    let read = write("big_items", &program(true));
    let (stdout, _) = run(&read, &[]);
    // The first item fails: its byte isn't 0 or 1, or its length is past `i32::MAX`.
    let want = [
        "the length at byte 13 is more than the bytes left can hold",
        "byte 17 should be 0 or 1",
        "byte 17 should be 0 or 1",
        "the length at byte 17 is more than the bytes left can hold",
    ];
    assert_eq!(stdout.lines().collect::<Vec<_>>(), want);
    let with = arena_bytes(&read);
    let without = arena_bytes(&write("big_items_control", &program(false)));
    assert!(with <= without + 4096, "reading the claims grew the arena from {without} to {with} bytes");
}
