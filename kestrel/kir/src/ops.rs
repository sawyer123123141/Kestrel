//! Operations that `ExprKind::Math`, `ExprKind::Map`, `ExprKind::Str` and `ExprKind::Hash` perform.

/// Float operations: the operands and the result all have the expression's type (F32 or F64).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathOp {
    Sqrt,
    Floor,
    Ceil,
    Round,
    Abs,
    Min,
    Max,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Exp,
    Log,
    Pow,
}

/// What `ExprKind::Map` does with the map at its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapOp {
    /// `[key]`: an optional holding the value (bytes copied).
    Get,
    /// `[key]`: a bool.
    Has,
    /// `[key]`: removes the key; a bool, true when it was there.
    Remove,
    /// `[]`: the number of entries used, removed ones included (i64).
    Used,
    /// `[index]`: whether entry `index` is below `Used` and still there.
    Live,
    /// `[]`: starts a loop over the map; the old lock count (i64), for `MapUnlock`.
    Lock,
    /// `[src]`, a `Load` of another map of the same type: copies `src`'s live entries, in order,
    /// into the place's own entries and rebuilds its index table, reusing them
    /// (`kestrel_rt_map_copy_into`). A bool: true when it did; false, with nothing changed, when
    /// the map is being looped over or has too little room. Keys and values are copied as bytes
    /// (heap ones still point at `src`'s memory). Never allocates or fails; `src` is only read.
    CopyInto,
}

/// What `Stmt::List` does with the list at its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListOp {
    /// `[index]`: removes that item, moving the later ones down and zeroing the vacated slot. An
    /// owning item is dropped first; the runtime panics when the index is out of range.
    RemoveAt,
    /// `[]`: empties the list, zeroing the items (an owning item was dropped first) and keeping the
    /// capacity.
    Clear,
    /// `[index, value]`: opens a slot at `index` (`index == len` adds at the end), moving the
    /// later ones up, and stores `value` in it. `value` is evaluated before anything moves.
    Insert,
    /// `[kept, next]`: finishes a loop that removed items as it went (see `lower::remove`). Items
    /// `[next, len)`, which the loop didn't reach, move down to `kept`, the slots they leave are
    /// zeroed, and the length becomes `kept` plus their count. When `next` is past the length (the
    /// list shrank meanwhile) the length becomes at most `kept`.
    Compact,
    /// `[compare]` or `[compare, key]` (`sort` on strings, and `sort_by` with a string key): sorts the items
    /// stably, smallest first. `compare` is a
    /// `FnRef` to a generated function `(env, a, b) -> u8` that says how the item at the pointer
    /// `a` stands to the one at `b`: 0 for before, 1 for equal, 2 for after (a total order). Its
    /// environment word is the address of a copy of `key`, a function value, when there is one, else
    /// 0. The items move as bytes.
    Sort,
    /// `[fill]` or `[fill, key]` (`sort` on numbers or bools, and `sort_by` with a number or bool
    /// key): sorts the items stably, smallest first, by a key per item, as `Sort` does. `fill` is a
    /// `FnRef` to a generated function `(env, xs, keys)` that writes the key of each item of the
    /// list or array at `xs`, in order and widened, into `keys`, an `[i64]` (`SortKey::Int`) or
    /// `[f64]` (`SortKey::Float`) with a slot per item; the runtime calls it once, when there are
    /// at least two items. Its environment word is as for `Sort` (see `lower::sort`).
    SortBy(SortKey),
    /// `[n]` (an `i64`): gives the list room for at least `n` items; a list with less room gets a
    /// new buffer of exactly `n` items in its own arena (`kestrel_rt_list_reserve`). The length and
    /// the items are unchanged. Fails when `n` is more than a list can hold. `to_bytes` and
    /// `from_bytes` use it (see `lower::bytes` and `lower::bytes_read`) to allocate their results
    /// once, and `map` (see `lower::list_fns`) to make room for every result before its loop.
    Reserve,
    /// `[at, value]`, on a `list[u8]`: writes `value`'s bytes into the items from index `at` (an
    /// `i64`), little-endian and unaligned: a bool one byte (0 or 1), a number its width (a float
    /// its raw bits), a `vec4` or `ivec4` its lanes in order, a string its byte length as a `u32`
    /// and then its bytes. The caller has made the length cover them; nothing is checked.
    WriteBytes,
    /// `[at, n]`, on a `list[u8]`: writes the `i64` `n` (a length, a count or a variant number, from
    /// 0 to `u32::MAX`) as a `u32` into the items from index `at`, as `WriteBytes` does.
    WriteCount,
}

/// The widened type of a `ListOp::SortBy` key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortKey {
    /// An integer or bool key, as an `i64`.
    Int,
    /// A float key, as an `f64`: by value, with a NaN after every number.
    Float,
}

/// What `ExprKind::Str` does with its string operand (`args[0]`, a string value; for `Join` a list
/// or array of strings). A string may be zero-filled (null pointer, length 0), which is the empty
/// string. Characters are Unicode code points; every string made is new, in the current arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrOp {
    /// `[s]`: the number of characters (i64).
    Chars,
    /// `[s, i]`: the character at index `i` (any integer) as a new string in the current arena;
    /// the runtime panics when `i` is out of range.
    At,
    /// `[s, pos]`: the byte length (i64) of the character starting at byte `pos` (i64).
    CharLen,
    /// `[s, pos, n]`: a new string in the current arena of the bytes `pos..pos + n` (i64s).
    Piece,
    /// `[s, sub]`: the character index (i64) of the first `sub` in `s`, or -1. An empty `sub` is 0.
    Find,
    /// `[s, sub]`: whether `s` starts with `sub` (bool).
    Starts,
    /// `[a, b]`: how `a` stands to `b` by code point (the order of their UTF-8 bytes): a u8, 0 for
    /// before, 1 for equal, 2 for after.
    Cmp,
    /// `[s, sub]`: whether `s` ends with `sub` (bool).
    Ends,
    /// `[s, from, to]`: a new string of the characters `from..to` (any integers); the runtime
    /// panics unless `0 <= from <= to <= chars`.
    Slice,
    /// `[s]`: a new string without whitespace at either end.
    Trim,
    /// `[s]`: a new string in upper case.
    Upper,
    /// `[s]`: a new string in lower case.
    Lower,
    /// `[list, sep]`: a new string of the list's (or array's) strings with `sep` between them.
    Join,
    /// `[s, sep]`: a new `list[string]` of the pieces between the separators, all in the current
    /// arena; the runtime panics when `sep` is empty.
    Split,
    /// `[s]`: an `int?`, `none` unless `s` is a whole number that fits.
    ToInt,
    /// `[s]`: a `float?`, `none` unless `s` is a finite number.
    ToFloat,
    /// `[bits, digits]`: a new string of `0x` and the low `digits` hex digits (upper case) of the
    /// `i64` `bits`, as `first_difference` shows a float's bits (`0x42CE8000`).
    Hex,
    /// `[code, at, len, name]`: a new string, the sentence saying why `from_bytes` couldn't read a
    /// `name` (a string) from `len` bytes: the reason `code` (see `lower::bytes_read::why`) at
    /// byte `at` (all `i64`s). Made only when a read has failed, so it counts for no contract.
    BytesReason,
}

/// What `ExprKind::Hash` does: one step of `checksum`'s fold (see the runtime's `checksum`), or a
/// float's raw bits. Each gives an `i64`; none allocates or fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashOp {
    /// `[h, x]`: `x` mixed into the fold `h` (an `i64`). `x` is a bool (0 or 1), a `u8` (its
    /// value), an `int` or `i64` (sign-extended), a `float` (its 32 bits, zero-extended) or `f64`
    /// (its 64 bits), each NaN made the canonical quiet NaN, a `vec4` or `ivec4` (its lanes in
    /// order, as those numbers), or a string (its byte length, then its bytes in 8-byte
    /// little-endian words, the last padded with zeros).
    Mix,
    /// `[h]`: the fold finished (SplitMix64's finaliser), the value `checksum` gives.
    Finish,
    /// `[x]`: the raw bits of the `float` (zero-extended) or `f64` `x`, NaN payloads and the sign
    /// of zero kept: `first_difference` compares floats by these.
    Bits,
}
