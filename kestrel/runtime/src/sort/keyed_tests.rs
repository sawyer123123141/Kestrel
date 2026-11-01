//! The keyed sorts against the order they had before the radix sort (the standard library's
//! stable sort by the key), on random, shaped and special inputs.

use std::cmp::Ordering;

use super::*;

/// Items of 16 bytes: an integer key and the bits of a float key.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Item {
    key: i64,
    tag: i64,
}

/// How float key `a` stands to `b`: by value, with a NaN after every number and equal to another
/// NaN (the order the generated compare functions use, and the old keyed sort's).
fn float_order(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or_else(|| a.is_nan().cmp(&b.is_nan()))
}

/// A list or array value as compiled code passes it: the items' address and their count.
#[repr(C)]
struct Header {
    ptr: *const Item,
    len: i64,
}

std::thread_local! {
    /// How many times a fill function ran on this test's thread.
    static FILLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Writes each item's `key` as an integer key.
extern "C" fn fill_int(_env: *const u8, xs: *const u8, keys: *mut Keys) {
    FILLS.with(|c| c.set(c.get() + 1));
    // SAFETY: the tests pass a `Header` over `Item`s and room for a key for each.
    unsafe {
        let (h, k) = (&*xs.cast::<Header>(), &*keys);
        assert_eq!(h.len, k.len);
        for i in 0..h.len as usize {
            *k.ptr.add(i) = (*h.ptr.add(i)).key as u64;
        }
    }
}

/// Writes each item's `tag`, the bits of a float, as a float key.
extern "C" fn fill_float(_env: *const u8, xs: *const u8, keys: *mut Keys) {
    FILLS.with(|c| c.set(c.get() + 1));
    // SAFETY: as in `fill_int`.
    unsafe {
        let (h, k) = (&*xs.cast::<Header>(), &*keys);
        for i in 0..h.len as usize {
            *k.ptr.add(i) = (*h.ptr.add(i)).tag as u64;
        }
    }
}

/// Sorts `v` with the keyed sort, by `key` (integer) or `tag` (float bits).
fn keyed(v: &mut [Item], float: bool) {
    let h = Header { ptr: v.as_ptr(), len: v.len() as i64 };
    let xs = (&h as *const Header).cast::<u8>();
    let (items, len) = (v.as_mut_ptr().cast::<u8>(), v.len() as i64);
    // SAFETY: `v` is `len` items of 16 bytes, and the fill functions write a key for each.
    unsafe {
        if float {
            kestrel_rt_sort_keyed_float(items, len, 16, fill_float, std::ptr::null(), xs);
        } else {
            kestrel_rt_sort_keyed_int(items, len, 16, fill_int, std::ptr::null(), xs);
        }
    }
}

/// The order the keyed sorts had before the radix sort: the standard library's stable sort of
/// `(key, position)` by the key (`float_order` for floats).
fn reference(v: &[Item], float: bool) -> Vec<Item> {
    let mut keyed = v.to_vec();
    if float {
        keyed.sort_by(|a, b| float_order(f64::from_bits(a.tag as u64), f64::from_bits(b.tag as u64)));
    } else {
        keyed.sort_by_key(|a| a.key);
    }
    keyed
}

fn check(v: &[Item]) {
    for float in [false, true] {
        let mut got = v.to_vec();
        keyed(&mut got, float);
        assert_eq!(got, reference(v, float), "float keys: {float}, {} items", v.len());
    }
}

/// Floats that test the order: both zeros, both infinities, NaNs of either sign and other
/// payloads, the least subnormals, the greatest finite numbers, and ordinary ones.
const FLOATS: [u64; 16] = [
    0x0000_0000_0000_0000,
    0x8000_0000_0000_0000,
    0x7ff0_0000_0000_0000,
    0xfff0_0000_0000_0000,
    0x7ff8_0000_0000_0000,
    0xfff8_0000_0000_0000,
    0x7ff0_0000_0000_0001,
    0xffff_ffff_ffff_ffff,
    0x0000_0000_0000_0001,
    0x8000_0000_0000_0001,
    0x7fef_ffff_ffff_ffff,
    0xffef_ffff_ffff_ffff,
    0x3ff0_0000_0000_0000,
    0xbff0_0000_0000_0000,
    0x3ff8_0000_0000_0000,
    0x4004_0000_0000_0000,
];

/// Integers that test the order: the least and greatest, around zero, and around 2^32.
const INTS: [i64; 10] = [i64::MIN, i64::MAX, -1, 0, 1, i64::MIN + 1, i64::MAX - 1, 4_294_967_296, -4_294_967_296, 42];

/// `n` items whose integer key and float bits `pick` gives from a random number and the
/// position. The checks compare whole items, so equal keys must keep their order.
fn items(n: usize, seed: u64, pick: impl Fn(u64, usize) -> (i64, u64)) -> Vec<Item> {
    let mut s = seed | 1;
    (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let (key, tag) = pick(s, i);
            Item { key, tag: tag as i64 }
        })
        .collect()
}

#[test]
fn keyed_sorts_match_the_old_order_at_every_size_up_to_1000() {
    for n in 0..=1_000 {
        let seed = 0x2545_f491_4f6c_dd1d ^ n as u64;
        // Random keys from the special values, and random keys anywhere.
        check(&items(n, seed, |r, _| (INTS[(r % 10) as usize], FLOATS[(r >> 8) as usize % 16])));
        check(&items(n, seed, |r, _| (r as i64, r.rotate_left(17))));
    }
}

#[test]
fn keyed_sorts_match_the_old_order_on_shaped_inputs() {
    for n in [2, 3, 63, 64, 65, 1_000, 100_000] {
        // All equal, sorted, reversed, few distinct keys, a NaN in every other item, zeros of
        // both signs, the integer extremes, and fractions.
        check(&items(n, 1, |r, _| (7, FLOATS[(r % 2) as usize])));
        check(&items(n, 2, |_, i| (i as i64, (i as f64).to_bits())));
        check(&items(n, 3, |_, i| (-(i as i64), (-(i as f64)).to_bits())));
        check(&items(n, 4, |r, _| ((r % 3) as i64 - 1, ((r % 3) as f64 - 1.0).to_bits())));
        check(&items(n, 5, |r, i| (r as i64, if i % 2 == 0 { FLOATS[4 + (r % 4) as usize] } else { (r as f64).to_bits() })));
        check(&items(n, 6, |r, _| ((r % 2) as i64, FLOATS[(r % 2) as usize])));
        check(&items(n, 7, |r, _| (INTS[(r % 2) as usize], FLOATS[2 + (r % 2) as usize])));
        check(&items(n, 8, |r, _| (r as i64, (r as i64 as f64 / 3.0).to_bits())));
    }
}

#[test]
fn the_fill_runs_once_and_not_for_fewer_than_two_items() {
    let mut v = items(500, 9, |r, _| (r as i64, r));
    FILLS.with(|c| c.set(0));
    keyed(&mut v, false);
    keyed(&mut v, true);
    assert_eq!(FILLS.with(|c| c.get()), 2);
    let mut one = items(1, 9, |r, _| (r as i64, r));
    keyed(&mut one, false);
    keyed(&mut one[..0], true);
    // SAFETY: items with no bytes are never keyed or read.
    unsafe { kestrel_rt_sort_keyed_int(v.as_mut_ptr().cast(), 500, 0, fill_int, std::ptr::null(), std::ptr::null()) };
    assert_eq!(FILLS.with(|c| c.get()), 2);
}

#[test]
fn order_bits_follow_the_old_orders() {
    for a in FLOATS {
        for b in FLOATS {
            let (x, y) = (f64::from_bits(a), f64::from_bits(b));
            assert_eq!(float_bits(x).cmp(&float_bits(y)), float_order(x, y), "{x} {y}");
        }
    }
    for a in INTS {
        for b in INTS {
            assert_eq!(int_bits(a as u64).cmp(&int_bits(b as u64)), a.cmp(&b));
        }
    }
}
