//! `xs.sort()` and `xs.sort_by(key)`: a stable sort over a list's or array's bytes, ordered by a
//! compare function that compiled code generates, or for a number or bool key by the keys a
//! generated key function gives, each worked out once (see `kir/src/lower/sort.rs`).

use std::cmp::Ordering;

use crate::string::KStr;

/// The compare function: given the environment word and the addresses of two items, it says how
/// the first stands to the second: 0 for before, 1 for equal, anything else for after. It must be
/// a total order, which compiled code makes it (a float NaN sorts after every number).
pub type Compare = extern "C" fn(env: *const u8, a: *const u8, b: *const u8) -> i8;

/// A key function for an integer or bool key, widened to `i64`: given the environment word and
/// the address of an item, its key.
pub type KeyInt = extern "C" fn(env: *const u8, item: *const u8) -> i64;

/// A key function for a float key, widened to `f64`.
pub type KeyFloat = extern "C" fn(env: *const u8, item: *const u8) -> f64;

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
    let mut sorted = vec![0u8; n * width];
    for (k, i) in order.enumerate() {
        // SAFETY: as the caller guarantees; `sorted` holds `n` items and doesn't overlap them.
        unsafe { std::ptr::copy_nonoverlapping(at(items, size, i), sorted.as_mut_ptr().add(k * width), width) };
    }
    // SAFETY: as above.
    unsafe { std::ptr::copy_nonoverlapping(sorted.as_ptr(), items, n * width) };
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

/// Sorts the items as `kestrel_rt_sort` does, by the integer key `key` gives each, called once per
/// item, in order.
///
/// # Safety
/// As `kestrel_rt_sort`, with `key` callable with an item's address and `env`.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_sort_by_int(items: *mut u8, len: i64, size: i64, key: KeyInt, env: *const u8) {
    let Some(n) = count(len, size) else { return };
    // SAFETY (`at`): every position is below `len`.
    let mut keyed: Vec<(i64, u32)> = (0..n).map(|i| (key(env, unsafe { at(items, size, i) }), i)).collect();
    // (`sort_by_key` is stable.)
    keyed.sort_by_key(|&(k, _)| k);
    // SAFETY: `keyed` holds each position below `len` once.
    unsafe { permute(items, size, keyed.into_iter().map(|(_, i)| i)) };
}

/// Sorts the items as `kestrel_rt_sort_by_int` does, by a float key: numbers by value (`-0.0`
/// equal to `0.0`), a NaN after every number and equal to another NaN.
///
/// # Safety
/// As `kestrel_rt_sort_by_int`.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_sort_by_float(items: *mut u8, len: i64, size: i64, key: KeyFloat, env: *const u8) {
    let Some(n) = count(len, size) else { return };
    // SAFETY (`at`): every position is below `len`.
    let mut keyed: Vec<(f64, u32)> = (0..n).map(|i| (key(env, unsafe { at(items, size, i) }), i)).collect();
    keyed.sort_by(|a, b| float_order(a.0, b.0));
    // SAFETY: `keyed` holds each position below `len` once.
    unsafe { permute(items, size, keyed.into_iter().map(|(_, i)| i)) };
}

/// How float key `a` stands to `b`: by value, with a NaN after every number and equal to another
/// NaN (the order the generated compare functions use).
fn float_order(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or_else(|| a.is_nan().cmp(&b.is_nan()))
}

/// Sorts the `len` items of `size` bytes at `items` stably, smallest first, by the integer key of
/// each item, which compiled code has written to `keys` in item order (spec decision 8 of the
/// performance round builds it; until then it fails).
///
/// # Safety
/// `items` must be valid for reading and writing `len * size` bytes, and `keys` for reading `len`
/// keys.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_sort_keyed_int(_items: *mut u8, _len: i64, _size: i64, _keys: *const i64) {
    crate::panic::fail("the keyed integer sort is not built yet, so this compiler can't sort by it")
}

/// As `kestrel_rt_sort_keyed_int`, for float keys in `float_order`.
///
/// # Safety
/// As for `kestrel_rt_sort_keyed_int`.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_sort_keyed_float(_items: *mut u8, _len: i64, _size: i64, _keys: *const f64) {
    crate::panic::fail("the keyed float sort is not built yet, so this compiler can't sort by it")
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

    extern "C" fn int_key(_env: *const u8, item: *const u8) -> i64 {
        // SAFETY: the sort passes addresses of `Item`s.
        unsafe { (*item.cast::<Item>()).key }
    }

    /// The `tag` of an item as a float key: the test stores a float's bits in it.
    extern "C" fn float_key(_env: *const u8, item: *const u8) -> f64 {
        // SAFETY: the sort passes addresses of `Item`s.
        f64::from_bits(unsafe { (*item.cast::<Item>()).tag } as u64)
    }

    #[test]
    fn an_int_key_sorts_like_the_compare_function_and_runs_once_per_item() {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        extern "C" fn counted(env: *const u8, item: *const u8) -> i64 {
            CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            int_key(env, item)
        }
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut v: Vec<Item> = (0..5_000)
            .map(|i| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                Item { key: (seed >> 40) as i64 % 300 - 150, tag: i }
            })
            .collect();
        let mut expected = v.clone();
        sort(&mut expected);
        // SAFETY: as in `sort`.
        unsafe { kestrel_rt_sort_by_int(v.as_mut_ptr().cast(), v.len() as i64, 16, counted, std::ptr::null()) };
        assert_eq!(v, expected);
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::Relaxed), 5_000);
    }

    #[test]
    fn a_float_key_puts_nan_last_and_keeps_equal_keys_in_order() {
        let keys = [2.5, f64::NAN, -0.0, 1.0, 0.0, f64::NEG_INFINITY, -f64::NAN, 1.0];
        let mut v: Vec<Item> = keys.iter().enumerate().map(|(i, k)| Item { key: i as i64, tag: k.to_bits() as i64 }).collect();
        // SAFETY: as in `sort`.
        unsafe { kestrel_rt_sort_by_float(v.as_mut_ptr().cast(), v.len() as i64, 16, float_key, std::ptr::null()) };
        // -inf, then -0.0 and 0.0 (equal: kept in order), the two 1.0s in order, 2.5, then both NaNs
        // (equal: kept in order).
        assert_eq!(v.iter().map(|i| i.key).collect::<Vec<_>>(), [5, 2, 4, 3, 7, 0, 1, 6]);
        assert_eq!(float_order(f64::NAN, f64::INFINITY), Ordering::Greater);
        assert_eq!(float_order(-0.0, 0.0), Ordering::Equal);
    }

    #[test]
    fn keyed_sorts_leave_short_lists_alone() {
        extern "C" fn never_int(_: *const u8, _: *const u8) -> i64 {
            panic!("nothing to key")
        }
        extern "C" fn never_float(_: *const u8, _: *const u8) -> f64 {
            panic!("nothing to key")
        }
        let mut v = vec![Item { key: 7, tag: 1 }];
        // SAFETY: one item, or none, is never keyed.
        unsafe {
            kestrel_rt_sort_by_int(v.as_mut_ptr().cast(), 1, 16, never_int, std::ptr::null());
            kestrel_rt_sort_by_float(std::ptr::null_mut(), 0, 16, never_float, std::ptr::null());
        }
        assert_eq!(v, [Item { key: 7, tag: 1 }]);
    }
}
