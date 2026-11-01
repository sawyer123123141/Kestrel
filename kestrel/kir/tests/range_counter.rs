//! The nest counter rule of range analysis (performance round, spec decision 7): a counter set
//! before a nest of constant `for` ranges and bumped once per innermost pass stays below its start
//! plus the product of the trip counts.

mod range_common;

use kestrel_kir::range::Arith::{Checked, Wrapping};
use kestrel_kir::range::CheckKind::{Bounds, Overflow};
use range_common::*;

/// The voxel world's `solids` loop (`bench/voxels3d/voxels3d.kes`), with the density left out:
/// `out[i]` on line 7 and `i += 1` on line 8.
fn nest(inner: &str, bump: &str) -> String {
    format!(
        "pure fill(seed: int) -> [u8] {{
    var out = array[u8](34 * 34 * 34)
    var i = 0
    for y from 0 to 34 {{
        for z from 0 to 34 {{
            for x from 0 to 34 {{
                {inner}out[i] = u8(seed & 1)
                {bump}
            }}
        }}
    }}
    out
}}"
    )
}

#[test]
fn a_nest_counter_proves_its_index_and_its_bump() {
    for arith in [Checked, Wrapping] {
        let (m, a) = analysed(&nest("", "i += 1"), arith);
        assert!(left_of(&m, &a, "fill", Bounds).is_empty(), "{arith:?}: {:#?}", checks(&m, &a, "fill").left);
        assert!(left_of(&m, &a, "fill", Overflow).is_empty(), "{arith:?}: {:#?}", checks(&m, &a, "fill").left);
    }
}

#[test]
fn a_counter_written_anywhere_else_in_the_nest_proves_nothing() {
    let (m, a) = analysed(&nest("if x == 3 { i = 0 }\n", "i += 1"), Checked);
    assert_eq!(left_of(&m, &a, "fill", Bounds).len(), 1, "{:#?}", checks(&m, &a, "fill").left);
}

#[test]
fn a_nest_with_a_break_proves_nothing() {
    let (m, a) = analysed(&nest("if seed == 3 { break }\n", "i += 1"), Checked);
    assert_eq!(left_of(&m, &a, "fill", Bounds).len(), 1, "{:#?}", checks(&m, &a, "fill").left);
}

#[test]
fn a_bump_inside_an_if_proves_nothing() {
    let (m, a) = analysed(&nest("", "if seed > 0 { i += 1 }"), Checked);
    assert_eq!(left_of(&m, &a, "fill", Bounds).len(), 1, "{:#?}", checks(&m, &a, "fill").left);
}

#[test]
fn a_bump_of_two_proves_nothing() {
    let (m, a) = analysed(&nest("", "i += 2"), Checked);
    assert_eq!(left_of(&m, &a, "fill", Bounds).len(), 1, "{:#?}", checks(&m, &a, "fill").left);
}

#[test]
fn a_counter_that_may_wrap_keeps_its_checks() {
    // 50,000 passes of a bump from 2,147,483,000: the last ones wrap past `int`.
    let src = "pure count(n: int) -> int {
    var i = 2147483000
    for y from 0 to 50000 {
        i += 1
    }
    i
}";
    let (m, a) = analysed(src, Checked);
    assert_eq!(left_of(&m, &a, "count", Overflow).len(), 1, "{:#?}", checks(&m, &a, "count").left);
}

#[test]
fn a_counter_starting_anywhere_known_is_bounded_from_there() {
    // `i` starts from 10 to 20; on the 4 * 5 passes it is at most 39, so `xs[i]` of 40 items is safe.
    let src = "pure fill(k: int) -> [int] {
    var xs = array[int](40)
    var i = 10
    if k > 0 { i = 20 }
    for a from 0 to 4 {
        for b from 0 to 5 {
            xs[i] = k
            i += 1
        }
    }
    xs
}";
    let (m, a) = analysed(src, Checked);
    assert!(left_of(&m, &a, "fill", Bounds).is_empty(), "{:#?}", checks(&m, &a, "fill").left);
    // One item fewer is not.
    let (m, a) = analysed(&src.replace("array[int](40)", "array[int](39)"), Checked);
    assert_eq!(left_of(&m, &a, "fill", Bounds).len(), 1, "{:#?}", checks(&m, &a, "fill").left);
}
