//! The order of a keyed sort: positions sorted stably by `u64` order bits, with an LSD radix sort
//! of `(bits, position)` pairs (11-bit digits; a digit every key shares is skipped), or an
//! insertion sort below `SMALL` items. Both are stable, and a stable sort by a total preorder has
//! exactly one result, so the order is the one any stable sort by the same bits gives.

/// A key's order bits and the position of its item.
#[derive(Clone, Copy)]
#[repr(C)]
struct Pair {
    bits: u64,
    at: u32,
}

/// Bits per digit, and the digits that cover 64 bits (the last has 9).
const DIGIT: u32 = 11;
const BUCKETS: usize = 1 << DIGIT;
const PASSES: usize = 64usize.div_ceil(DIGIT as usize);

/// Below this many items, an insertion sort.
const SMALL: usize = 64;

fn digit(bits: u64, pass: usize) -> usize {
    ((bits >> (pass as u32 * DIGIT)) as usize) & (BUCKETS - 1)
}

/// The positions `0..bits.len()` in the order of their bits, smallest first, equal bits in
/// position order. There must be at most `u32::MAX` of them.
pub(super) fn sorted_positions(bits: &[u64]) -> Vec<u32> {
    let n = bits.len();
    let mut pairs: Vec<Pair> = bits.iter().enumerate().map(|(i, &bits)| Pair { bits, at: i as u32 }).collect();
    if n < SMALL {
        insertion_sort(&mut pairs);
    } else {
        pairs = radix_sort(pairs, bits);
    }
    pairs.into_iter().map(|p| p.at).collect()
}

/// A stable insertion sort by the bits.
fn insertion_sort(v: &mut [Pair]) {
    for i in 1..v.len() {
        let p = v[i];
        let mut j = i;
        while j > 0 && v[j - 1].bits > p.bits {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = p;
    }
}

/// A stable LSD radix sort of `src` (the pairs of `bits`, in position order) by the bits.
fn radix_sort(mut src: Vec<Pair>, bits: &[u64]) -> Vec<Pair> {
    let n = src.len();
    // Every digit's counts, in one pass over the keys.
    let mut counts = vec![[0u32; BUCKETS]; PASSES];
    for &b in bits {
        for (pass, count) in counts.iter_mut().enumerate() {
            count[digit(b, pass)] += 1;
        }
    }
    let mut dst: Vec<Pair> = Vec::with_capacity(n);
    let mut dst_filled = false;
    for (pass, count) in counts.iter().enumerate() {
        // A digit every key shares leaves the order as it is.
        if count[digit(bits[0], pass)] as usize == n {
            continue;
        }
        let mut next = [0u32; BUCKETS];
        let mut sum = 0u32;
        for (slot, &c) in next.iter_mut().zip(count.iter()) {
            *slot = sum;
            sum += c;
        }
        let out = dst.as_mut_ptr();
        for p in &src {
            let d = digit(p.bits, pass);
            // SAFETY: the counts give each pair its own slot below `n`, which `dst` has room for.
            unsafe { out.add(next[d] as usize).write(*p) };
            next[d] += 1;
        }
        if !dst_filled {
            // SAFETY: the pass above wrote all `n` slots.
            unsafe { dst.set_len(n) };
            dst_filled = true;
        }
        std::mem::swap(&mut src, &mut dst);
    }
    src
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order the standard library's stable sort gives.
    fn reference(bits: &[u64]) -> Vec<u32> {
        let mut v: Vec<u32> = (0..bits.len() as u32).collect();
        v.sort_by_key(|&i| bits[i as usize]);
        v
    }

    fn random(n: usize, seed: u64, spread: u64) -> Vec<u64> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                if spread == 0 {
                    s
                } else {
                    s % spread
                }
            })
            .collect()
    }

    #[test]
    fn matches_a_stable_sort_at_every_size_around_the_switch() {
        for n in 0..300 {
            for spread in [0, 1, 3, 2048, 1 << 40] {
                let bits = random(n, 0x9e37_79b9_7f4a_7c15 ^ n as u64, spread);
                assert_eq!(sorted_positions(&bits), reference(&bits), "n {n}, spread {spread}");
            }
        }
    }

    #[test]
    fn matches_a_stable_sort_on_large_inputs() {
        for (seed, spread) in [(1u64, 0u64), (2, 10), (3, 1 << 33), (4, u64::MAX)] {
            let bits = random(100_000, seed, spread);
            assert_eq!(sorted_positions(&bits), reference(&bits), "spread {spread}");
        }
        // Keys that differ only in the top digit or only in the bottom one.
        let top: Vec<u64> = random(5_000, 5, 0).into_iter().map(|b| b & (0x1ff << 55)).collect();
        let bottom: Vec<u64> = random(5_000, 6, 0).into_iter().map(|b| b & 0x7ff | 0xdead_0000_0000_0000).collect();
        for bits in [top, bottom] {
            assert_eq!(sorted_positions(&bits), reference(&bits));
        }
    }

    #[test]
    fn sorted_reversed_and_equal_keys() {
        let up: Vec<u64> = (0..1_000).collect();
        let down: Vec<u64> = (0..1_000).rev().collect();
        let same = vec![u64::MAX; 1_000];
        let ends: Vec<u64> = (0..1_000).map(|i| if i % 2 == 0 { 0 } else { u64::MAX }).collect();
        for bits in [up, down, same, ends] {
            assert_eq!(sorted_positions(&bits), reference(&bits));
        }
    }
}
