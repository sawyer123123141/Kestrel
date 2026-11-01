//! `reassoc` (performance round, spec decision 9): in a `^`, `&` or `|` chain inside a loop, the
//! operands the loop never changes are grouped after the others, and nothing else changes.

mod after_common;

use after_common::*;
use kestrel_kir::range::Arith::{Checked, Wrapping};
use kestrel_kir::visit::{walk, walk_expr, Node};
use kestrel_kir::*;

/// The value assigned to `acc` inside a loop of `f`.
fn assigned(f: &Func) -> &Expr {
    let mut found = None;
    walk(&f.body, &mut |n| {
        if let Node::Stmt(Stmt::ForRange { body, .. }) = n {
            for s in body {
                if let Stmt::Assign(p, e) = s {
                    if f.locals[p.root.0 as usize].name == "acc" {
                        found = Some(e);
                    }
                }
            }
        }
    });
    found.expect("an assignment to `acc` in a loop")
}

/// The names of the locals `e` reads, sorted.
fn reads(f: &Func, e: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    walk_expr(e, &mut |n| {
        if let Node::Place(p, _) = n {
            out.push(f.locals[p.root.0 as usize].name.clone());
        }
    });
    out.sort();
    out.dedup();
    out
}

/// `acc`'s new value in `name`, built in `arith`, before and after the passes after marking.
fn before_after(src: &str, arith: kestrel_kir::range::Arith) -> (Module, Module) {
    (marked(src, arith), built(src, arith))
}

const HASH: &str = "pure h(n: int, y: int, z: int, s: int) -> i64 {
    var acc: i64 = 0
    for x from 0 to n {
        acc = acc ^ (i64(x) * 73856093 ^ i64(y) * 19349663 ^ i64(z) * 83492791 ^ i64(s) * 1610612741)
    }
    acc
}";

#[test]
fn the_invariant_parts_of_a_hash_are_grouped_after_the_rest() {
    for arith in [Checked, Wrapping] {
        let m = built(HASH, arith);
        let f = func(&m, "h");
        let e = assigned(f);
        let ExprKind::Binary(BinOp::BitXor, changing, same) = &e.kind else { panic!("{arith:?}: {}", expr(&m, f, e, arith)) };
        assert_eq!(reads(f, changing), ["acc", "x"], "{arith:?}: {}", expr(&m, f, e, arith));
        assert_eq!(reads(f, same), ["s", "y", "z"], "{arith:?}: {}", expr(&m, f, e, arith));
        // In written order inside the group: `(y * B ^ z * C) ^ s * D`.
        let ExprKind::Binary(BinOp::BitXor, yz, s) = &same.kind else { panic!("{}", expr(&m, f, e, arith)) };
        assert_eq!((reads(f, yz), reads(f, s)), (vec!["y".to_string(), "z".to_string()], vec!["s".to_string()]), "{}", expr(&m, f, e, arith));
    }
}

#[test]
fn sums_and_floats_are_left_alone() {
    let sum = HASH.replace(" ^ ", " +% ");
    let float = "pure h(n: int, y: float, z: float, s: float) -> float {
    var acc = 0.0
    for x from 0 to n {
        acc = acc + (float(x) * 0.5 + y * 0.25 + z * 0.125 + s)
    }
    acc
}";
    for src in [sum.as_str(), float] {
        for arith in [Checked, Wrapping] {
            let (old, new) = before_after(src, arith);
            assert_eq!(assigned(func(&old, "h")), assigned(func(&new, "h")), "{src}");
        }
    }
}

#[test]
fn a_chain_of_another_operator_is_one_operand() {
    // `y & s` is one operand of the `^` chain, which has one invariant operand: nothing moves.
    let src = "pure h(n: int, y: int, s: int) -> int {
    var acc = 0
    for x from 0 to n {
        acc = acc ^ x ^ (y & s)
    }
    acc
}";
    for arith in [Checked, Wrapping] {
        let (old, new) = before_after(src, arith);
        assert_eq!(assigned(func(&old, "h")), assigned(func(&new, "h")));
    }
}

#[test]
fn a_chain_with_a_check_left_is_left_alone() {
    // `y * c` and `z * c` may overflow in a checked build: moving them would move their checks.
    // (The `print` keeps them in the loop: they can't run before it.)
    let src = "h(n: int, y: int, z: int, c: int) -> int {
    var acc = 0
    for x from 0 to n {
        print(x)
        acc = acc ^ x ^ (y * c) ^ (z * c)
    }
    acc
}";
    let (old, new) = before_after(src, Checked);
    assert_eq!(assigned(func(&old, "h")), assigned(func(&new, "h")));
    let m = built(src, Wrapping);
    let f = func(&m, "h");
    let ExprKind::Binary(BinOp::BitXor, _, same) = &assigned(f).kind else { panic!() };
    assert_eq!(reads(f, same), ["c", "y", "z"]);
}

#[test]
fn a_chain_outside_any_loop_is_left_alone() {
    let src = "pure h(x: int, y: int, z: int) -> int {\n    x ^ y ^ z\n}";
    let (old, new) = before_after(src, Wrapping);
    assert_eq!(func(&old, "h").body, func(&new, "h").body);
}
