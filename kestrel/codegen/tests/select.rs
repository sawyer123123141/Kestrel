//! Small `if`s lowered as selects (spec decision 2 of the performance round): which ones are, what
//! stays a branch, and that a selected float is bit-for-bit what the branch gives.

mod common;

use common::{jit_with, optimized};
use kestrel_codegen::{dump, JitProgram, Options};

fn release() -> Options {
    Options { checked_arith: false, ..Options::default() }
}

/// The optimised IR of `name` (the dump up to the machine code).
fn ir(src: &str, opts: Options, name: &str) -> String {
    let m = optimized(src);
    let code = dump(&m, opts, name).unwrap();
    code.split("\n}\n").next().unwrap().to_string()
}

fn count(code: &str, op: &str) -> usize {
    code.lines().filter(|l| l.split_whitespace().any(|w| w == op || w.starts_with(&format!("{op}.")))).count()
}

const COLLATZ: &str = "pure collatz_len(start: i64) -> int {\n var n = start\n var steps = 0\n while n != 1 {\n  if n % 2 == 0 { n = n / 2 } else { n = 3 * n + 1 }\n  steps += 1\n }\n steps\n}\nmain() {}\n";

#[test]
fn collatz_picks_the_next_value_without_a_branch_on_the_parity() {
    let code = ir(COLLATZ, release(), "collatz_len");
    // One branch, the loop's own test; the parity is a `select` (a `cmov`).
    assert_eq!(count(&code, "brif"), 1, "{code}");
    assert_eq!(count(&code, "select"), 1, "{code}");
    // `n / 2` after a passed even test is one exact shift, without the rounding bias.
    assert_eq!(count(&code, "ushr"), 0, "{code}");
}

/// `f`'s IR, for a body that sets `x` in an `if`.
fn shape(body: &str, opts: Options) -> String {
    let src = format!(
        "struct P {{\n x: int\n}}\nagain(n: int) -> int {{\n if n > 0 {{\n  return again(n - 1)\n }}\n 0\n}}\nf(var p: P, n: int, a: int, b: int, c: int, d: int) -> int {{\n var x = a\n var y = b\n var z = c\n {body}\n x + y + z + p.x\n}}\nmain() {{}}\n"
    );
    ir(&src, opts, "f")
}

#[test]
fn a_small_if_on_register_locals_is_a_select() {
    let cases = [
        "if n > 0 { x = (a + b) * c } else { y = d }",
        "if n > 0 { x = b\n y = a } else { x = c }",
        "if n > 0 { x = a ^ b }",
    ];
    for body in cases {
        let code = shape(body, release());
        assert!(count(&code, "select") >= 1 && count(&code, "brif") == 0, "{body}\n{code}");
    }
}

#[test]
fn an_if_with_a_call_check_nested_if_or_four_operations_stays_a_branch() {
    let cases = [
        ("a call", "if n > 0 { x = again(a) }", release()),
        ("a check left", "if n > 0 { x = a + b }", Options::default()),
        ("a nested if", "if n > 0 { if a > 0 { x = b } }", release()),
        ("four operations", "if n > 0 { x = ((a + b) * c - d) ^ a }", release()),
        ("three locals", "if n > 0 { x = a\n y = b\n z = c }", release()),
        ("a division by a variable", "if d != 0 { x = a / d }", release()),
    ];
    for (what, body, opts) in cases {
        let code = shape(body, opts);
        // Nothing before the `if` branches, so a branch in the entry block is the `if`'s own.
        let entry = code[code.find("block0").unwrap()..].split("\n\n").next().unwrap();
        assert_eq!(count(entry, "brif"), 1, "{what}: {body}\n{code}");
    }
}

#[test]
fn an_if_that_stores_to_memory_stays_a_branch() {
    // The index is proven in bounds, so the store is all the side does.
    let src = "g(var xs: list[int], n: int, a: int) {\n for i from 0 to xs.len {\n  if n > i { xs[i] = a }\n }\n}\nmain() {}\n";
    let code = ir(src, release(), "g");
    assert!(code.contains("store") && count(&code, "select") == 0, "{code}");
}

/// Float values that a branch and a select must agree on, bit for bit.
const FLOATS: &[f64] = &[0.0, -0.0, 1.5, -2.25, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN, 1e-310, 3.0];

fn f64_fn(p: &JitProgram, name: &str) -> F64Fn {
    unsafe { std::mem::transmute(p.fn_ptr(name).expect(name)) }
}

fn f32_fn(p: &JitProgram, name: &str) -> F32Fn {
    unsafe { std::mem::transmute(p.fn_ptr(name).expect(name)) }
}

/// `s` (a statement `if`, a min) and `v` (a value `if`) pick floats on a float compare; `g` picks
/// on an integer condition; `k` computes floats on its sides, so it stays a branch. Each must give
/// what Rust computes with a branch.
fn float_source(ty: &str) -> String {
    format!(
        "pure s(a: {ty}, b: {ty}, c: {ty}) -> {ty} {{
 var x = b
 if a < b {{ x = a }}
 x
}}
pure v(a: {ty}, b: {ty}, c: {ty}) -> {ty} {{
 if a != b {{ c }} else {{ a }}
}}
pure g(a: {ty}, b: {ty}, c: {ty}) -> {ty} {{
 var x = a
 if int(c) > 2 {{ x = b }}
 x
}}
pure k(a: {ty}, b: {ty}, c: {ty}) -> {ty} {{
 var x = c
 if a < b {{ x = -b * 0.5 }} else {{ x = a - c }}
 x
}}
main() {{}}
"
    )
}

type F64Fn = extern "C" fn(f64, f64, f64) -> f64;
type F32Fn = extern "C" fn(f32, f32, f32) -> f32;

#[test]
fn a_selected_float_has_the_branch_bits() {
    let src64 = float_source("f64");
    let src32 = float_source("float");
    for (name, branches) in [("s", 0), ("v", 0), ("g", 0), ("k", 1)] {
        let code = ir(&src64, release(), name);
        assert_eq!(count(&code, "brif"), branches, "{name}: {code}");
    }
    for opts in [release(), Options::default()] {
        let p64 = jit_with(&src64, opts);
        let p32 = jit_with(&src32, opts);
        let f64s: Vec<F64Fn> = ["s", "v", "g", "k"].iter().map(|n| f64_fn(&p64, n)).collect();
        let f32s: Vec<F32Fn> = ["s", "v", "g", "k"].iter().map(|n| f32_fn(&p32, n)).collect();
        for &a in FLOATS {
            for &b in FLOATS {
                for &c in FLOATS {
                    let what = format!("{a} {b} {c}");
                    let want = [
                        if a < b { a } else { b },
                        if a != b { c } else { a },
                        if (c as i32) > 2 { b } else { a },
                        if a < b { -b * 0.5 } else { a - c },
                    ];
                    for (i, w) in want.iter().enumerate() {
                        assert_eq!(f64s[i](a, b, c).to_bits(), w.to_bits(), "f64 #{i} {what}");
                    }
                    let (a, b, c) = (a as f32, b as f32, c as f32);
                    let want = [
                        if a < b { a } else { b },
                        if a != b { c } else { a },
                        if (c as i32) > 2 { b } else { a },
                        if a < b { -b * 0.5 } else { a - c },
                    ];
                    for (i, w) in want.iter().enumerate() {
                        assert_eq!(f32s[i](a, b, c).to_bits(), w.to_bits(), "f32 #{i} {what}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_exact_shift_is_only_used_while_the_tested_value_is_unchanged() {
    let src = "pure h(n: int) -> int {\n var m = 0\n var k = n\n if k % 4 == 0 {\n  m = k / 4 + k / 2\n  k = k + 3\n  m = m * 100 + k / 2\n }\n m\n}\npure e(n: int) -> int {\n var m = 7\n if n % 2 != 0 { m = 1 } else { m = n / 2 }\n m\n}\nmain() {}\n";
    for opts in [release(), Options::default()] {
        let p = jit_with(src, opts);
        let (h, e) = (fn_i32(&p, "h"), fn_i32(&p, "e"));
        for n in [-17, -16, -12, -8, -6, -5, -4, -3, -2, -1, 0, 1, 2, 4, 8, 12, 16, 100, i32::MIN, i32::MAX, i32::MIN + 4] {
            let want_h = if n % 4 == 0 {
                let m = n / 4 + n / 2;
                let k = n.wrapping_add(3);
                m.wrapping_mul(100).wrapping_add(k / 2)
            } else {
                0
            };
            if opts.checked_arith && (n == i32::MIN || n == i32::MIN + 4) {
                continue;
            }
            assert_eq!(h(n), want_h, "h({n})");
            assert_eq!(e(n), if n % 2 != 0 { 1 } else { n / 2 }, "e({n})");
        }
    }
}

fn fn_i32(p: &JitProgram, name: &str) -> extern "C" fn(i32) -> i32 {
    unsafe { std::mem::transmute(p.fn_ptr(name).expect(name)) }
}
