//! `recurse` (performance round, spec decision 6): in `--release`, a recursion that folds its calls
//! with `+ * ^ & |` becomes a loop with one call per pass; checked builds keep the recursion.

mod after_common;

use after_common::*;
use kestrel_kir::range::Arith::{Checked, Wrapping};
use kestrel_kir::visit::{walk, Node};
use kestrel_kir::*;

const FIB: &str = "pure fib(n: i64) -> i64 {\n    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }\n}";

/// How many calls of `f` itself `f` makes, and how many of those are inside a loop.
fn self_calls(m: &Module, name: &str) -> (usize, usize) {
    let me = FuncIdx(m.funcs.iter().position(|f| f.name == name).unwrap() as u32);
    let count = |stmts: &[Stmt]| {
        let mut n = 0;
        walk(stmts, &mut |node| n += usize::from(matches!(node, Node::Expr(Expr { kind: ExprKind::Call(fi, _), .. }) if *fi == me)));
        n
    };
    let f = func(m, name);
    let in_loops: usize = f.body.iter().map(|s| if let Stmt::While { body, .. } = s { count(body) } else { 0 }).sum();
    (count(&f.body), in_loops)
}

/// Whether the passes after marking leave `name` as range analysis marked it, in `arith`.
fn unchanged(src: &str, name: &str, arith: kestrel_kir::range::Arith) -> bool {
    func(&marked(src, arith), name).body == func(&built(src, arith), name).body
}

#[test]
fn fib_in_release_is_a_loop_with_one_call() {
    let m = built(FIB, Wrapping);
    assert_eq!(self_calls(&m, "fib"), (1, 1), "{}", outline(&m, func(&m, "fib"), &func(&m, "fib").body, Wrapping));
}

#[test]
fn checked_builds_keep_the_recursion() {
    assert!(unchanged(FIB, "fib", Checked));
    assert_eq!(self_calls(&built(FIB, Checked), "fib"), (2, 0));
}

#[test]
fn every_operator_folds() {
    for op in ["+", "*", "^", "&", "|", "+%", "*%"] {
        let src = format!("pure f(n: int) -> int {{\n    if n < 2 {{ n + 3 }} else {{ f(n - 1) {op} f(n - 2) }}\n}}");
        assert_eq!(self_calls(&built(&src, Wrapping), "f"), (1, 1), "{op}");
    }
    for op in ["-", "/", "<<"] {
        let src = format!("pure f(n: int) -> int {{\n    if n < 2 {{ n + 3 }} else {{ f(n - 1) {op} f(n - 2) }}\n}}");
        assert!(unchanged(&src, "f", Wrapping), "{op}");
    }
}

#[test]
fn a_float_sum_is_not_folded() {
    let src = "pure f(n: int) -> float {\n    if n < 2 { float(n) } else { f(n - 1) + f(n - 2) }\n}";
    assert!(unchanged(src, "f", Wrapping));
}

#[test]
fn calls_of_another_function_are_not_folded() {
    let src = format!("{FIB}\npure g(n: i64) -> i64 {{\n    if n < 2 {{ n }} else {{ fib(n - 1) + fib(n - 2) }}\n}}");
    assert!(unchanged(&src, "g", Wrapping));
}

#[test]
fn a_value_before_the_call_may_be_anything() {
    // `100 / n` runs before the call either way: as a loop, it is folded in first.
    let src = "pure f(n: int) -> int {\n    if n == 0 { 0 } else { 100 / n + f(n - 1) }\n}";
    assert_eq!(self_calls(&built(src, Wrapping), "f"), (0, 0));
}

#[test]
fn a_value_after_the_call_that_can_fail_keeps_the_recursion() {
    // As a loop, `100 / n` would run before the call it follows.
    let src = "pure f(n: int) -> int {\n    if n == 0 { 0 } else { f(n - 1) + 100 / n }\n}";
    assert!(unchanged(src, "f", Wrapping));
    let pure_after = "pure f(n: int) -> int {\n    if n == 0 { 0 } else { f(n - 1) + n * 3 }\n}";
    assert_eq!(self_calls(&built(pure_after, Wrapping), "f"), (0, 0));
}

#[test]
fn a_body_written_with_returns_folds() {
    let src = "pure f(n: int) -> int {\n    if n <= 0 {\n        return 7\n    }\n    return f(n - 1) + f(n - 3)\n}";
    assert_eq!(self_calls(&built(src, Wrapping), "f"), (1, 1));
}

#[test]
fn a_var_parameter_keeps_the_recursion() {
    let src = "f(n: int, var k: int) -> int {\n    if n < 2 { n } else { f(n - 1, var k) + f(n - 2, var k) }\n}";
    assert!(unchanged(src, "f", Wrapping));
}
