//! `xs.sort()` and `xs.sort_by(key)`: a stable sort over a list's or array's bytes, ordered by a
//! compare function that compiled code generates (a string key), or by number keys that a
//! generated fill function writes, one per item, in item order (see `kir/src/lower/sort.rs`).
//! Number keys are sorted as order bits by `radix`.

mod radix;
#[cfg(test)]
mod keyed_tests;

use std::cmp::Ordering;

use crate::string::KStr;

/// The compare function: given the environment word and the addresses of two items, it says how
/// the first stands to the second: 0 for before, 1 for equal, anything else for after. It must be
/// a total order, which compiled code makes it (a float NaN sorts after every number).
pub type Compare = extern "C" fn(env: *const u8, a: *const u8, b: *const u8) -> i8;

/// A fill function: given the environment word, the address of the list or array being sorted
/// and the address of a `Keys` array of as many keys as it has items, it writes each item's key
/// (an integer or bool widened to `i64`, or a float widened to `f64`) in item order.
pub type Fill = extern "C" fn(env: *const u8, xs: *const u8, keys: *mut Keys) -> ();

/// The keys a fill function writes, as a Kestrel array value: `[i64]` or `[f64]`.
#[repr(C)]
pub struct Keys {
    pub ptr: *mut u64,
    pub len: i64,
}

/// The number of items to sort, or `None` when there is nothing to do (fewer than two items, or
/// items with no bytes). Fails for more items than a `u32` position can name.
fn count(len: i64, size: i64) -> Option<u32> {
    if len < 2 || size <= 0 {
        return None;
    }
    match u32::try_from(len) {
        Ok(n) => Some(n),
        Err(_) => crate::panic::fail("can't sort a list of more than 4294967295 items"),
    }
}

/// The address of item `i` of `size` bytes at `items`.
///
/// # Safety
/// `i` must be below the number of items at `items`.
unsafe fn at(items: *mut u8, size: i64, i: u32) -> *mut u8 {
    // SAFETY: the caller guarantees the item is there.
    unsafe { items.add(i as usize * size as usize) }
}

/// Moves the items into `order` (the old position of each item, in its new order) through a
/// temporary buffer. Moving by bytes is safe for every Kestrel value: a list or a map keeps its
/// arena pointer, and nothing points into the buffer of the list that holds it.
///
/// # Safety
/// `items` must be valid for reading and writing `order.len()` items of `size` bytes, and every
/// position in `order` below that.
unsafe fn permute(items: *mut u8, size: i64, order: impl ExactSizeIterator<Item = u32>) {
    let (n, width) = (order.len(), size as usize);
    // Uninitialised: every byte is written before it is read.
    let mut sorted: Vec<u8> = Vec::with_capacity(n * width);
    let buffer = sorted.as_mut_ptr();
    for (k, i) in order.enumerate() {
        // SAFETY: as the caller guarantees; `sorted` has room for `n` items and doesn't overlap them.
        unsafe { std::ptr::copy_nonoverlapping(at(items, size, i), buffer.add(k * width), width) };
    }
    // SAFETY: as above; the loop wrote all `n` items.
    unsafe { std::ptr::copy_nonoverlapping(buffer, items, n * width) };
}

/// Sorts the `len` items of `size` bytes at `items`, smallest first by `compare`, keeping the
/// order of equal items. The items move as bytes: it sorts a vector of positions, comparing the
/// items where they are, then permutes the items.
///
/// # Safety
/// `items` must be valid for reading and writing `len * size` bytes, and `compare` must be
/// callable with the items' addresses and `env`.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_sort(items: *mut u8, len: i64, size: i64, compare: Compare, env: *const u8) {
    let Some(n) = count(len, size) else { return };
    let mut order: Vec<u32> = (0..n).collect();
    // `sort_by` is stable. The items don't move until the order is known.
    // SAFETY (`at`): every position is below `len`.
    order.sort_by(|&i, &j| match compare(env, unsafe { at(items, size, i) }, unsafe { at(items, size, j) }) {
        0 => Ordering::Less,
        1 => Ordering::Equal,
        _ => Ordering::Greater,
    });
    // SAFETY: `order` holds each position below `len` once.
    unsafe { permute(items, size, order.into_iter()) };
}

/// The keys `fill` writes for the `n` items of the list or array at `xs`, as raw bits.
///
/// # Safety
/// `fill` must write all `n` keys, given `env` and `xs`.
unsafe fn keys(n: u32, fill: Fill, env: *const u8, xs: *const u8) -> Vec<u64> {
    let mut bits: Vec<u64> = Vec::with_capacity(n as usize);
    let mut keys = Keys { ptr: bits.as_mut_ptr(), len: i64::from(n) };
    fill(env, xs, &mut keys);
    // SAFETY: the caller guarantees `fill` wrote every key.
    unsafe { bits.set_len(n as usize) };
    bits
}

/// The order bits of an `i64` key: unsigned order is the signed order.
fn int_bits(k: u64) -> u64 {
    k ^ (1 << 63)
}

/// The order bits of an `f64` key, in `float_order`: `-0.0` as `0.0`, every NaN after every number
/// and equal to another (`u64::MAX`), a negative number's bits all flipped and a positive one's
/// sign bit set.
fn float_bits(k: f64) -> u64 {
    if k.is_nan() {
        return u64::MAX;
    }
    let bits = if k == 0.0 { 0 } else { k.to_bits() };
    if bits >> 63 == 1 {
        !bits
    } else {
        bits | (1 << 63)
    }
}

/// Sorts the `len` items of `size` bytes at `items`, the items of the list or array at `xs`,
/// stably, smallest first, by integer keys: `fill` writes each item's key, once and in item
/// order, unless there are fewer than two items. Then the items move once, into a buffer and back.
///
/// # Safety
/// `items` must be valid for reading and writing `len * size` bytes, and `fill` must write `len`
/// keys, given `env` and `xs`.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_sort_keyed_int(items: *mut u8, len: i64, size: i64, fill: Fill, env: *const u8, xs: *const u8) {
    let Some(n) = count(len, size) else { return };
    // SAFETY: as the caller guarantees.
    let mut bits = unsafe { keys(n, fill, env, xs) };
    bits.iter_mut().for_each(|b| *b = int_bits(*b));
    // SAFETY: the order holds each position below `len` once.
    unsafe { permute(items, size, radix::sorted_positions(&bits).into_iter()) };
}

/// As `kestrel_rt_sort_keyed_int`, for float keys in `float_order`.
///
/// # Safety
/// As for `kestrel_rt_sort_keyed_int`.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_sort_keyed_float(items: *mut u8, len: i64, size: i64, fill: Fill, env: *const u8, xs: *const u8) {
    let Some(n) = count(len, size) else { return };
    // SAFETY: as the caller guarantees.
    let mut bits = unsafe { keys(n, fill, env, xs) };
    bits.iter_mut().for_each(|b| *b = float_bits(f64::from_bits(*b)));
    // SAFETY: the order holds each position below `len` once.
    unsafe { permute(items, size, radix::sorted_positions(&bits).into_iter()) };
}

/// How the first string stands to the second by code point: 0 for before, 1 for equal, 2 for
/// after. UTF-8 keeps the order of code points, so this compares bytes.
///
/// # Safety
/// Both strings must be valid for reading their bytes unless the pointer is null.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_str_cmp(a: *const u8, a_len: i64, b: *const u8, b_len: i64) -> i8 {
    // SAFETY: the caller guarantees both strings.
    let (a, b) = unsafe { (KStr::bytes(a, a_len), KStr::bytes(b, b_len)) };
    match a.cmp(b) {
        Ordering::Less => 0,
        Ordering::Equal => 1,
        Ordering::Greater => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(a: &str, b: &str) -> i8 {
        // SAFETY: valid strings.
        unsafe { kestrel_rt_str_cmp(a.as_ptr(), a.len() as i64, b.as_ptr(), b.len() as i64) }
    }

    #[test]
    fn strings_compare_by_code_point() {
        assert_eq!((cmp("a", "z"), cmp("z", "a"), cmp("ab", "ab")), (0, 2, 1));
        // A prefix comes first, uppercase before lowercase, 'é' (233) after 'z' (122), and a
        // character outside the BMP after one inside it.
        assert_eq!((cmp("ab", "abc"), cmp("B", "a"), cmp("z", "é"), cmp("\u{ffff}", "\u{10000}")), (0, 0, 0, 0));
        // The empty string, whether null or not, is first.
        // SAFETY: a null pointer with length 0 is the empty string.
        assert_eq!(unsafe { kestrel_rt_str_cmp(std::ptr::null(), 0, "a".as_ptr(), 1) }, 0);
        assert_eq!(cmp("", ""), 1);
    }

    /// Items of 16 bytes: a key and a tag that says where the item started.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Item {
        key: i64,
        tag: i64,
    }

    extern "C" fn by_key(_env: *const u8, a: *const u8, b: *const u8) -> i8 {
        // SAFETY: the sort passes addresses of `Item`s.
        let (a, b) = unsafe { (*a.cast::<Item>(), *b.cast::<Item>()) };
        match a.key.cmp(&b.key) {
            Ordering::Less => 0,
            Ordering::Equal => 1,
            Ordering::Greater => 2,
        }
    }

    /// Sorts `v` with the runtime.
    fn sort(v: &mut [Item]) {
        // SAFETY: `v` is `v.len()` items of `size_of::<Item>()` bytes.
        unsafe { kestrel_rt_sort(v.as_mut_ptr().cast(), v.len() as i64, std::mem::size_of::<Item>() as i64, by_key, std::ptr::null()) };
    }

    #[test]
    fn equal_keys_keep_their_order() {
        let mut v: Vec<Item> = (0..40).map(|i| Item { key: i % 3, tag: i }).collect();
        sort(&mut v);
        assert!(v.windows(2).all(|w| w[0].key < w[1].key || (w[0].key == w[1].key && w[0].tag < w[1].tag)), "{v:?}");
    }

    #[test]
    fn no_items_and_one_item() {
        let mut v: Vec<Item> = Vec::new();
        sort(&mut v);
        assert!(v.is_empty());
        let mut v = vec![Item { key: 7, tag: 1 }];
        sort(&mut v);
        assert_eq!(v, [Item { key: 7, tag: 1 }]);
        // A null list (a list nothing was added to) has no items either.
        // SAFETY: with no items, nothing is read.
        unsafe { kestrel_rt_sort(std::ptr::null_mut(), 0, 16, by_key, std::ptr::null()) };
    }

    #[test]
    fn matches_the_standard_library_on_random_items() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut v: Vec<Item> = (0..10_000).map(|i| Item { key: (next() % 500) as i64 - 250, tag: i }).collect();
        let mut expected = v.clone();
        // (`sort_by_key` is stable too.)
        expected.sort_by_key(|i| i.key);
        sort(&mut v);
        assert_eq!(v, expected);
    }

    #[test]
    fn the_environment_reaches_the_compare_function() {
        extern "C" fn backwards(env: *const u8, a: *const u8, b: *const u8) -> i8 {
            // SAFETY: the test passes the address of a bool.
            assert!(unsafe { *env } == 1);
            by_key(std::ptr::null(), b, a)
        }
        let mut v: Vec<Item> = (0..5).map(|i| Item { key: i, tag: i }).collect();
        let flag = 1u8;
        // SAFETY: as in `sort`.
        unsafe { kestrel_rt_sort(v.as_mut_ptr().cast(), 5, 16, backwards, &flag) };
        assert_eq!(v.iter().map(|i| i.key).collect::<Vec<_>>(), [4, 3, 2, 1, 0]);
    }

    #[test]
    fn items_without_size_are_left_alone() {
        extern "C" fn never(_: *const u8, _: *const u8, _: *const u8) -> i8 {
            panic!("nothing to compare")
        }
        let mut bytes = [0u8; 4];
        // SAFETY: a size of 0 reads nothing.
        unsafe { kestrel_rt_sort(bytes.as_mut_ptr(), 3, 0, never, std::ptr::null()) };
    }
}
