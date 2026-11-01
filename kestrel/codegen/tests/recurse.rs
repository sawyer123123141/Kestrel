//! `fib` after the performance round's decision 6: `--release` makes one call of itself per loop
//! pass (`acc += fib(n - 1); n -= 2`, as Rust's `fib` does); a checked build still makes two.

mod common;

use kestrel_codegen::{dump, Options};

const FIB: &str = "pure fib(n: i64) -> i64 {\n    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }\n}\nmain() {}\n";

/// The calls in `fib`'s optimised IR, compiled with `opts`.
fn calls(opts: Options) -> usize {
    let code = dump(&common::optimized(FIB), opts, "fib").unwrap();
    let ir = code.split("\n}\n").next().unwrap();
    ir.lines().filter(|l| l.contains("= call fn") || l.trim_start().starts_with("call fn")).count()
}

#[test]
fn released_fib_calls_itself_once() {
    assert_eq!(calls(Options { checked_arith: false, ..Options::default() }), 1);
}

#[test]
fn checked_fib_calls_itself_twice() {
    // And the overflow check's panic call.
    assert_eq!(calls(Options::default()), 3);
}
