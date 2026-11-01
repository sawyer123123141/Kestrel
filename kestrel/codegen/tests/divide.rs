//! Division and remainder by a literal: every divisor against Rust's `wrapping_div` and
//! `wrapping_rem` (edge dividends and 100,000 seeded ones), and no division instruction left in
//! the generated code for any literal but 0 and -1.

mod common;

use common::{jit_with, optimized};
use kestrel_codegen::{dump, JitProgram, Options};

/// The divisors of spec decision 1 for `i64`.
fn divisors_i64() -> Vec<i64> {
    let mut ds: Vec<i64> = (3..=13).flat_map(|d| [d, -d]).collect();
    ds.extend([1000003, -1000003, 1000000007, 65537, i64::MAX, i64::MIN, i64::MIN + 1, 2147483647]);
    ds.extend((4..=62).map(|k| -(1i64 << k)));
    ds.push(-2);
    ds
}

/// The same set at `i32` width (`-2^k` up to `k = 30`, the `i32` limits).
fn divisors_i32() -> Vec<i64> {
    let mut ds: Vec<i64> = (3..=13).flat_map(|d| [d, -d]).collect();
    ds.extend([1000003, -1000003, 1000000007, 65537, i32::MAX as i64, i32::MIN as i64, i32::MIN as i64 + 1]);
    ds.extend((4..=30).map(|k| -(1i64 << k)));
    ds.push(-2);
    ds
}

const U8_DIVISORS: &[i64] = &[3, 5, 6, 7, 9, 10, 11, 13, 100, 127, 129, 254, 255];

/// Kestrel function-name suffix for a divisor (`-2` -> `m2`).
fn tag(n: i64) -> String {
    if n < 0 {
        format!("m{}", n.unsigned_abs())
    } else {
        n.to_string()
    }
}

/// `n` as Kestrel source: the lowest value of a type has no literal (its magnitude doesn't fit),
/// so it is written as `-MAX - 1`.
fn literal(n: i64) -> String {
    match n {
        i64::MIN => "(-9223372036854775807 - 1)".into(),
        n if n == i32::MIN as i64 => "(-2147483647 - 1)".into(),
        n => n.to_string(),
    }
}

/// `d_N(x) = x / N` and `r_N(x) = x % N` for every divisor, at type `ty`.
fn div_source(ty: &str, divisors: &[i64]) -> String {
    let mut src = String::new();
    for &n in divisors {
        let (t, l) = (tag(n), literal(n));
        src += &format!("d_{t}(x: {ty}) -> {ty} {{\n x / {l}\n}}\nr_{t}(x: {ty}) -> {ty} {{\n x % {l}\n}}\n");
    }
    src + "main() {}\n"
}

fn fn_of<A, R>(p: &JitProgram, name: &str) -> extern "C" fn(A) -> R {
    unsafe { std::mem::transmute(p.fn_ptr(name).expect(name)) }
}

/// A fixed sequence of 64-bit values (splitmix64).
fn seeded(count: usize) -> Vec<u64> {
    let mut s: u64 = 0x5EED_0FD1_7155;
    (0..count)
        .map(|_| {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        })
        .collect()
}

/// Edge dividends for divisor `d` in `[lo, hi]`: 0, Â±1, the limits, and multiples of `d`
/// with their neighbours.
fn edges(d: i64, lo: i64, hi: i64) -> Vec<i64> {
    let mut xs = vec![0, 1, -1, lo, hi, lo + 1, hi - 1, 2, -2];
    for m in [1i64, 2, 3, 7, 1000, -1, -2, -3, -7, -1000] {
        if let Some(x) = d.checked_mul(m) {
            for x in [x.checked_sub(1), Some(x), x.checked_add(1)].into_iter().flatten() {
                if (lo..=hi).contains(&x) {
                    xs.push(x);
                }
            }
        }
    }
    xs
}

#[test]
fn every_literal_divisor_matches_rust_i64() {
    let ds = divisors_i64();
    let random = seeded(100_000);
    for checked_arith in [true, false] {
        let p = jit_with(&div_source("i64", &ds), Options { checked_arith, ..Options::default() });
        for &d in &ds {
            let div = fn_of::<i64, i64>(&p, &format!("d_{}", tag(d)));
            let rem = fn_of::<i64, i64>(&p, &format!("r_{}", tag(d)));
            let xs = edges(d, i64::MIN, i64::MAX).into_iter().chain(random.iter().map(|&r| r as i64));
            for x in xs {
                assert_eq!(div(x), x.wrapping_div(d), "{x} / {d} (checked={checked_arith})");
                assert_eq!(rem(x), x.wrapping_rem(d), "{x} % {d} (checked={checked_arith})");
            }
        }
    }
}

#[test]
fn every_literal_divisor_matches_rust_i32() {
    let ds = divisors_i32();
    let random = seeded(100_000);
    for checked_arith in [true, false] {
        let p = jit_with(&div_source("int", &ds), Options { checked_arith, ..Options::default() });
        for &d in &ds {
            let d32 = d as i32;
            let div = fn_of::<i32, i32>(&p, &format!("d_{}", tag(d)));
            let rem = fn_of::<i32, i32>(&p, &format!("r_{}", tag(d)));
            let xs = edges(d, i32::MIN as i64, i32::MAX as i64).into_iter().chain(random.iter().map(|&r| r as i64));
            for x in xs.map(|x| x as i32) {
                assert_eq!(div(x), x.wrapping_div(d32), "{x} / {d} (checked={checked_arith})");
                assert_eq!(rem(x), x.wrapping_rem(d32), "{x} % {d} (checked={checked_arith})");
            }
        }
    }
}

#[test]
fn every_literal_divisor_matches_rust_u8() {
    for checked_arith in [true, false] {
        let p = jit_with(&div_source("u8", U8_DIVISORS), Options { checked_arith, ..Options::default() });
        for &d in U8_DIVISORS {
            let div = fn_of::<u8, u8>(&p, &format!("d_{}", tag(d)));
            let rem = fn_of::<u8, u8>(&p, &format!("r_{}", tag(d)));
            for x in 0..=255u8 {
                assert_eq!(div(x), x / d as u8, "{x} / {d} (checked={checked_arith})");
                assert_eq!(rem(x), x % d as u8, "{x} % {d} (checked={checked_arith})");
            }
        }
    }
}

/// One function `all(x)` xoring `x / d` and `x % d` for every divisor, so a single dump shows them.
fn all_source(ty: &str, divisors: &[i64]) -> String {
    let terms: Vec<String> = divisors.iter().map(|d| {
        let l = literal(*d);
        format!("(x / {l}) ^ (x % {l})")
    }).collect();
    format!("pure all(x: {ty}) -> {ty} {{\n {}\n}}\nmain() {{}}\n", terms.join(" ^ "))
}

#[test]
fn no_literal_divisor_but_zero_and_minus_one_divides() {
    let mut u8s = U8_DIVISORS.to_vec();
    u8s.extend([1, 2, 16, 128]);
    let cases = [("i64", divisors_i64()), ("int", divisors_i32()), ("u8", u8s)];
    for (ty, ds) in cases {
        let m = optimized(&all_source(ty, &ds));
        for checked_arith in [true, false] {
            let code = dump(&m, Options { checked_arith, ..Options::default() }, "all").unwrap();
            for op in ["sdiv", "srem", "udiv", "urem"] {
                assert!(!code.contains(op), "{ty} (checked={checked_arith}) has `{op}`:\n{code}");
            }
        }
    }
}
