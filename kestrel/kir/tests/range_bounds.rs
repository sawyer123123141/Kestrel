//! Bounds checks the analysis proves from lengths and facts between values, each with a near miss
//! that keeps its check.

mod range_common;

use kestrel_kir::range::Arith::Checked;
use kestrel_kir::range::CheckKind::{Bounds, Narrowing};
use kestrel_kir::range::{remove_checks, Analysis, Arith};
use kestrel_kir::Module;
use range_common::*;

/// The lines of the bounds checks left in `f`, and how many were proven.
fn bounds(src: &str) -> (Vec<u32>, usize) {
    let (m, a) = analysed(src, Checked);
    bounds_in(&m, &a, "f")
}

fn bounds_in(m: &Module, a: &Analysis, name: &str) -> (Vec<u32>, usize) {
    let left = left_of(m, a, name, Bounds).iter().map(|c| line(c.line)).collect();
    (left, proven_of(m, name, Bounds))
}

#[test]
fn a_counted_loop_over_len_proves_its_index() {
    let src = "pure f(xs: list[int]) -> int {\n var s = 0\n for i from 0 to xs.len {\n  s += xs[i]\n }\n s\n}";
    assert_eq!(bounds(src), (vec![], 1));
}

#[test]
fn the_next_index_is_not() {
    let src = "pure f(xs: list[int]) -> int {\n var s = 0\n for i from 0 to xs.len {\n  s += xs[i + 1]\n }\n s\n}";
    assert_eq!(bounds(src), (vec![4], 0));
}

#[test]
fn off_by_one_is_not() {
    let src = "pure f(xs: list[int]) -> int {\n var s = 0\n for i from 0 to xs.len + 1 {\n  s += xs[i]\n }\n s\n}";
    assert_eq!(bounds(src), (vec![4], 0));
}

/// A `while true` loop that breaks unless `i` is inside `xs`, then indexes with `i`; and `xs[i]`
/// again after the loop, on line 11.
const GUARD_BREAKS: &str = "pure f(xs: list[int], start: int) -> int {
    var s = 0
    var i = start
    while true {
        if i >= xs.len or i < 0 {
            break
        }
        s += xs[i]
        i += 1
    }
    s + xs[i]
}";

#[test]
fn a_guard_that_breaks_proves_the_rest_of_the_body() {
    let (left, proven) = bounds(GUARD_BREAKS);
    assert_eq!(proven, 1, "`xs[i]` on line 8 is proven");
    assert!(!left.contains(&8), "{left:?}");
}

#[test]
fn the_facts_after_a_break_are_the_breaks() {
    // After the loop, `i` is outside `xs` (the only way out is the `break`).
    assert_eq!(bounds(GUARD_BREAKS).0, vec![11]);
}

#[test]
fn a_while_guard_proves_its_index() {
    let src = "pure f(xs: list[int]) -> int {\n var s = 0\n var i = 0\n while i < xs.len {\n  s += xs[i]\n  i += 1\n }\n s\n}";
    assert_eq!(bounds(src), (vec![], 1));
}

#[test]
fn a_guard_with_and_proves_it() {
    let src = "pure f(xs: list[int], i: int) -> int {\n var s = 0\n if i >= 0 and i < xs.len {\n  s = xs[i]\n }\n s\n}";
    assert_eq!(bounds(src), (vec![], 1));
}

#[test]
fn a_guard_without_the_lower_end_does_not() {
    let src = "pure f(xs: list[int], i: int) -> int {\n var s = 0\n if i < xs.len {\n  s = xs[i]\n }\n s\n}";
    assert_eq!(bounds(src), (vec![4], 0));
}

#[test]
fn pop_proves_its_own_check() {
    // `pop` on a list of aggregates moves the last item out: `xs[n - 1]` under `n != 0`.
    let src = "f(var xs: list[list[int]]) {\n xs.pop()\n}";
    assert_eq!(bounds(src), (vec![], 1));
}

#[test]
fn the_draw_copy_is_proven() {
    let kernel = include_str!("../../bench/kernels/copy.kes");
    let src = kernel.split("\nmain()").next().unwrap();
    let (m, a) = analysed(src, Checked);
    let (left, proven) = bounds_in(&m, &a, "bench");
    assert!(left.is_empty(), "`positions[i]` is proven: {left:?}");
    assert!(proven >= 1);
}

#[test]
fn a_later_statement_reuses_a_passed_check() {
    // (`i * 2` can overflow before `parts[i]` is reached, so `bind_index` leaves both places indexed.)
    let src = "struct P {\n x: int\n y: int\n}\nf(var parts: list[P], i: int) {\n parts[i].x = i * 2\n parts[i].y = 2\n}";
    assert_eq!(bounds(src), (vec![6], 1));
}

#[test]
fn the_same_statement_does_not() {
    let src = "pure f(xs: list[int], i: int) -> int {\n var s = 0\n s = xs[i] + xs[i]\n s\n}";
    assert_eq!(bounds(src), (vec![3, 3], 0));
}

#[test]
fn the_right_of_and_does_not() {
    let src = "pure f(xs: list[int], i: int, c: bool) -> int {\n var s = 0\n if c and xs[i] > 0 {\n  s = 1\n }\n s + xs[i]\n}";
    assert_eq!(bounds(src), (vec![3, 6], 0));
}

#[test]
fn a_new_array_knows_its_length() {
    let src = "f() -> int {\n let a = array[int](10)\n a[9] + a[10]\n}";
    assert_eq!(bounds(src), (vec![3], 1));
}

#[test]
fn rand_int_inside_the_list_is_proven() {
    let src = "f(xs: list[int]) -> int {\n var s = 0\n if xs.len > 0 {\n  s = xs[randi_range(0, xs.len - 1)]\n }\n s\n}";
    assert_eq!(bounds(src), (vec![], 1));
    // Its near miss: the upper end may be `xs.len`.
    let src = "f(xs: list[int]) -> int {\n var s = 0\n if xs.len > 0 {\n  s = xs[randi_range(0, xs.len)]\n }\n s\n}";
    assert_eq!(bounds(src), (vec![4], 0));
}

/// The `par for` body in `m`.
fn par_body(m: &Module) -> String {
    assert_eq!(m.par_bodies.len(), 1);
    m.funcs[m.par_bodies[0].0 as usize].name.clone()
}

#[test]
fn a_par_for_body_knows_its_chunk() {
    for src in [
        "f(ps: list[int], qs: list[int]) {\n par for i, p in ps {\n  let v = qs[i] + p\n }\n}",
        "pure make() -> list[int] {\n [1, 2, 3]\n}\nf(qs: list[int]) {\n par for i, p in make() {\n  let v = qs[i] + p\n }\n}",
    ] {
        let (m, a) = analysed(src, Checked);
        let body = par_body(&m);
        assert_eq!(bounds_in(&m, &a, &body).0.len(), 1, "{src}: `qs[i]` stays");
        assert!(left_of(&m, &a, &body, Narrowing).is_empty(), "{src}: `int(start)` and `int(end)` are proven");
        assert_eq!(proven_of(&m, &body, Narrowing), 2, "{src}");
    }
}

#[test]
fn a_par_for_body_without_its_chunk_facts_keeps_its_conversions() {
    // The near miss: the same body as an ordinary function knows nothing about `start` and `end`.
    let src = "f(ps: list[int], start: i64, end: i64) -> int {\n var s = 0\n for i from int(start) to int(end) {\n  s += ps[i]\n }\n s\n}";
    let (m, a) = analysed(src, Checked);
    assert_eq!(left_of(&m, &a, "f", Narrowing).len(), 2);
    assert_eq!(bounds_in(&m, &a, "f").0, vec![4]);
}

#[test]
fn an_inner_loop_entered_again_has_a_fresh_end() {
    let src = "f(var xs: list[int], m: int) -> int {
    var s = 0
    for k from 0 to m {
        for i from 0 to xs.len {
            s += xs[i]
        }
        xs.pop()
    }
    s
}";
    assert_eq!(bounds(src), (vec![], 1));
}

/// A `var self` method that guards on its field `n` (promoted to a local) and indexes with it,
/// with `{between}` between the guard and the index.
fn promoted(between: &str) -> String {
    format!(
        "struct C {{
    n: int
    bump(var self, d: int) {{
        if d == 0 {{
            self.n += 1
            return
        }}
        self.bump(d - 1)
    }}
    take(var self, xs: list[int]) -> int {{
        var s = 0
        if self.n > 0 and self.n <= xs.len {{
            {between}
            s = xs[self.n - 1]
        }}
        s
    }}
}}"
    )
}

#[test]
fn a_promoted_field_forgets_after_its_change() {
    let (m, a) = analysed(&promoted("self.bump(1)"), Checked);
    let name = method(&m, "take");
    assert_eq!(bounds_in(&m, &a, &name), (vec![14], 0));
}

#[test]
fn a_promoted_field_proves_without_the_change() {
    // The near miss, showing the field's guard is a fact at all.
    let (m, a) = analysed(&promoted("s = 1"), Checked);
    let name = method(&m, "take");
    assert_eq!(bounds_in(&m, &a, &name), (vec![], 1));
}

/// The name KIR gives a method whose name ends in `name`.
fn method(m: &Module, name: &str) -> String {
    let found: Vec<_> = m.funcs.iter().filter(|f| f.name.ends_with(name)).map(|f| f.name.clone()).collect();
    assert_eq!(found.len(), 1, "{found:?}");
    found[0].clone()
}

#[test]
fn len_conversions_are_proven() {
    let src = "pure f(xs: list[int], s: string) -> int {\n let a = xs.len\n let b = s.len\n a ^ b\n}";
    let (m, a) = analysed(src, Checked);
    let left = left_of(&m, &a, "f", Narrowing);
    assert_eq!(left.len(), 1, "only the string's `s.len` keeps its check: {left:#?}");
    assert_eq!(line(left[0].line), 3);
}

#[test]
fn release_proves_bounds_too() {
    // Bounds checks stay under `--release`, and the same facts prove them.
    let src = "pure f(xs: list[int]) -> int {\n var s = 0\n for i from 0 to xs.len {\n  s = s ^ xs[i]\n }\n s\n}";
    let mut m = optimized(src);
    let a = remove_checks(&mut m, Arith::Wrapping);
    assert_eq!(bounds_in(&m, &a, "f"), (vec![], 1));
}

/// The bounds checks left in `f` under `--release`, and how many were proven.
fn release_bounds(src: &str) -> (Vec<u32>, usize) {
    let mut m = optimized(src);
    let a = remove_checks(&mut m, Arith::Wrapping);
    bounds_in(&m, &a, "f")
}

#[test]
fn release_a_wrapped_index_is_not_proven() {
    // `i + 1` wraps for the largest `i`: with checks on, the add's check makes it exact.
    let src = "f(xs: list[int], i: int) -> int {\n var s = 0\n let j = i + 1\n if i >= 0 and j < xs.len {\n  s = xs[j]\n }\n s\n}";
    assert_eq!(release_bounds(src), (vec![5], 0));
    assert_eq!(bounds(src), (vec![], 1), "checked, `j == i + 1` holds after the add's check");
}

#[test]
fn release_a_wrapped_bound_is_not_proven() {
    // `n - 1` wraps for the smallest `n`, so `i < n - 1` doesn't put `i` below `n`.
    let src = "f(xs: list[int], n: int, i: int) -> int {\n var s = 0\n if n <= xs.len and i >= 0 and i < n - 1 {\n  s = xs[i]\n }\n s\n}";
    assert_eq!(release_bounds(src), (vec![4], 0));
    assert_eq!(bounds(src), (vec![], 1));
}

#[test]
fn release_a_shift_teaches_nothing_about_its_amount() {
    let src = "f(xs: list[int], n: int) -> int {\n var s = 0\n let y = 1 << n\n if n < xs.len {\n  s = xs[n] + y\n }\n s\n}";
    assert_eq!(release_bounds(src), (vec![5], 0));
    assert_eq!(bounds(src), (vec![], 1), "checked, the shift's check says `n` is 0 to 31");
}

#[test]
fn a_changed_end_is_not_the_old_one() {
    // The inner loop's end is `n`, which grows after each run of it: each entry has its own end.
    let src = "f(xs: list[int], n0: int, m: int) -> int {
    var s = 0
    var n = n0
    if n >= 0 and n <= xs.len {
        for k from 0 to m {
            for i from 0 to n {
                s += xs[i]
            }
            n += 1
        }
    }
    s
}";
    assert_eq!(bounds(src), (vec![7], 0));
}
