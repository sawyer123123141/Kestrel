//! `hoist_checks` (performance round, spec decision 7): a check whose operands a loop never writes
//! moves before the loop, while nothing before it in the first pass can fail or has an effect.

mod after_common;

use after_common::*;
use kestrel_kir::range::Arith::{Checked, Wrapping};
use kestrel_kir::visit::{walk, Node};
use kestrel_kir::*;

/// The body of the `for` range whose variable is named `var`, anywhere in `f`.
fn loop_of<'a>(f: &'a Func, var: &str) -> &'a [Stmt] {
    let mut found = None;
    walk(&f.body, &mut |n| {
        if let Node::Stmt(Stmt::ForRange { var: v, body, .. }) = n {
            if f.locals[v.0 as usize].name == var {
                found = Some(&body[..]);
            }
        }
    });
    found.unwrap_or_else(|| panic!("no loop over `{var}`"))
}

/// The checks left in the body of the loop over `var` in `name`, built in `arith` with and without
/// the passes after marking, and the outline of the built body.
fn before_after(src: &str, name: &str, var: &str, arith: kestrel_kir::range::Arith) -> (usize, usize, String) {
    let old = marked(src, arith);
    let new = built(src, arith);
    let f = func(&new, name);
    let text = outline(&new, f, &f.body, arith);
    (checks_in(loop_of(func(&old, name), var), arith), checks_in(loop_of(f, var), arith), text)
}

#[test]
fn an_invariant_overflow_leaves_the_loop() {
    let src = "pure f(c: int) -> int {\n var acc = 0\n for x from 0 to 10 {\n  acc = acc ^ (c * 32 + 5)\n }\n acc\n}";
    let (old, new, text) = before_after(src, "f", "x", Checked);
    assert_eq!((old, new), (2, 0), "{text}");
}

#[test]
fn an_invariant_check_leaves_every_loop_it_is_invariant_in() {
    let src = "pure f(c: int, n: int) -> int {\n var acc = 0\n for y from 0 to 4 {\n  for x from 0 to 10 {\n   acc = acc ^ (c * 32) ^ (y * c)\n  }\n }\n acc\n}";
    let m = built(src, Checked);
    let f = func(&m, "f");
    let text = outline(&m, f, &f.body, Checked);
    assert_eq!(checks_in(loop_of(f, "x"), Checked), 0, "{text}");
    // `c * 32` is before the outer loop; `y * c` is in the outer loop's body, before the inner one.
    assert_eq!(checks_in(loop_of(f, "y"), Checked), 1, "{text}");
    assert!(text.starts_with("let acc#") && text.contains("let __checked") && text.find("let __checked").unwrap() < text.find("for y").unwrap(), "{text}");
}

#[test]
fn a_check_after_a_print_stays() {
    let src = "f(c: int) -> int {\n var acc = 0\n for x from 0 to 10 {\n  print(x)\n  acc = acc ^ (c * 32)\n }\n acc\n}";
    let (old, new, text) = before_after(src, "f", "x", Checked);
    assert_eq!((old, new), (1, 1), "{text}");
}

#[test]
fn a_check_after_an_index_that_can_fail_stays() {
    let src = "pure f(c: int, xs: [int]) -> int {\n var acc = 0\n for x from 0 to 10 {\n  acc = acc ^ xs[x]\n  acc = acc ^ (c * 32)\n }\n acc\n}";
    let (old, new, text) = before_after(src, "f", "x", Checked);
    assert_eq!((old, new), (2, 2), "{text}");
}

#[test]
fn a_check_inside_an_if_stays() {
    let src = "pure f(c: int, k: int) -> int {\n var acc = 0\n for x from 0 to 10 {\n  if x == k {\n   acc = acc ^ (c * 32)\n  }\n }\n acc\n}";
    let (old, new, text) = before_after(src, "f", "x", Checked);
    assert_eq!((old, new), (1, 1), "{text}");
}

#[test]
fn a_check_before_it_is_peeled_for_the_first_pass() {
    // `d + x` can fail on the first pass, before `c * 32`: it is copied before the loop with `x`
    // = 0, in front of `c * 32`, and stays in the loop too.
    let src = "pure f(c: int, d: int) -> int {\n var acc = 0\n for x from 0 to 10 {\n  acc = acc ^ (d + x) ^ (c * 32)\n }\n acc\n}";
    let m = built(src, Checked);
    let f = func(&m, "f");
    let text = outline(&m, f, &f.body, Checked);
    assert_eq!(checks_in(loop_of(f, "x"), Checked), 1, "{text}");
    let peel = text.find("(d#1 Add! 0)").unwrap_or_else(|| panic!("{text}"));
    let moved = text.find("(c#0 Mul! 32)").unwrap_or_else(|| panic!("{text}"));
    assert!(peel < moved && moved < text.find("for x").unwrap(), "{text}");
}

#[test]
fn a_peeled_check_with_nothing_moved_after_it_is_dropped() {
    let src = "pure f(c: int, d: int) -> int {\n var acc = 0\n for x from 0 to 10 {\n  acc = acc ^ (c * 32) ^ (d + x)\n }\n acc\n}";
    let m = built(src, Checked);
    let f = func(&m, "f");
    let text = outline(&m, f, &f.body, Checked);
    assert!(!text.contains("(d#1 Add! 0)"), "{text}");
    assert_eq!(text.matches("__checked").count(), 2, "one let, one load: {text}");
}

#[test]
fn a_loop_that_may_not_run_moves_its_checks_behind_a_guard() {
    let src = "pure f(c: int, n: int) -> int {\n var acc = 0\n for x from 0 to n {\n  acc = acc ^ (c * 32)\n }\n acc\n}";
    let m = built(src, Checked);
    let f = func(&m, "f");
    let text = outline(&m, f, &f.body, Checked);
    assert_eq!(checks_in(loop_of(f, "x"), Checked), 0, "{text}");
    assert!(text.contains("if (0 Lt n#1) {"), "{text}");
}

#[test]
fn a_while_loop_with_a_plain_condition_moves_its_checks_behind_it() {
    let src = "pure f(c: int, n: int) -> int {
 var acc = 0
 var i = 0
 while i < n {
  acc = acc ^ (c * 32)
  i += 1
 }
 acc
}";
    let while_body = |m: &Module| {
        let f = func(m, "f");
        let body = f.body.iter().find_map(|s| if let Stmt::While { body, .. } = s { Some(body.clone()) } else { None });
        checks_in(&body.unwrap(), Checked)
    };
    let (old, new) = (marked(src, Checked), built(src, Checked));
    let f = func(&new, "f");
    let text = outline(&new, f, &f.body, Checked);
    assert!(text.contains("if (i#") && text.contains(" Lt n#1) {"), "{text}");
    assert_eq!(while_body(&new), while_body(&old) - 1, "{text}");
}

#[test]
fn release_builds_move_divisions() {
    let src = "pure f(a: int, b: int) -> int {\n var acc = 0\n for x from 0 to 10 {\n  acc = acc ^ (a / b)\n }\n acc\n}";
    let (old, new, text) = before_after(src, "f", "x", Wrapping);
    assert_eq!((old, new), (1, 0), "{text}");
}

#[test]
fn the_frozen_voxel_worlds_sample_loop_keeps_two_checks_checked_and_none_released() {
    // Before the round: 14 checks per sample checked, 1 (`out[i]`) released.
    let src = include_str!("../../bench/voxels3d/voxels3d.kes");
    for (arith, left) in [(Checked, 2), (Wrapping, 0)] {
        let m = built(src, arith);
        let f = func(&m, "mesh");
        // Checked, the chunk offset `cx * 32 + x - 1` is all that is left: it depends on `x`.
        assert_eq!(checks_in(loop_of(f, "solids.x"), arith), left, "{arith:?}: {}", outline(&m, f, &f.body, arith));
    }
}
