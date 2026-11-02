//! The byte format of `to_bytes` and `from_bytes` (the I/O round's spec, decision 2). Compiled code
//! writes the bytes itself, field by field, into one `list[u8]` it sizes first, and reads them back
//! field by field; it comes here to give that list its room, to copy a string's bytes in, and to
//! make a string of bytes it reads. `encode` and `decode` are a reference encoder and decoder in
//! Rust over `RefValue` and `RefTy`, for tests and tools.
//!
//! The format, little-endian throughout and with no padding: a header of the 4 bytes `KESB`
//! (`MAGIC`), the format version (`VERSION`) and the type's fingerprint as an `i64` (the
//! `checksum` of the type's description, see `fingerprint`), 13 bytes in all; then the value. A
//! `bool` is one byte, 0 or 1; a `u8` one byte; an `int` or `float` four; an `i64` or `f64` eight
//! (a float's raw bits, NaN payloads kept); a vector its lanes in order. A string is a `u32` byte
//! length, then its UTF-8 bytes. An array or list is a `u32` length, then its items; a map a `u32`
//! live count, then each live key and value in insertion order. An optional is a byte, 0 or 1,
//! then its payload when 1; an enum a `u32` variant number, then that variant's fields; a struct
//! its fields in order.

use crate::arena::{items_fit, list_too_long, List};
use crate::checksum::{finish, mix_bytes, START};
use crate::string::KStr;

/// The first four bytes of every value `to_bytes` writes.
pub const MAGIC: [u8; 4] = *b"KESB";

/// The format version, the header's fifth byte.
pub const VERSION: u8 = 1;

/// The header's length: the magic, the version and the fingerprint.
pub const HEADER: usize = 13;

/// The fingerprint of a type with this description (`struct Game{ball:vec2,score:int}`): the
/// `checksum` of the description as a string.
pub fn fingerprint(description: &str) -> i64 {
    finish(mix_bytes(START, description.as_bytes())) as i64
}

/// Gives the list at `list` room for at least `n` items of `elem_size` bytes: a list with less
/// room moves its items into a new buffer of exactly `n` items, from the list's own arena. The
/// length is unchanged. Fails when `n` is more than a list can hold, or negative.
///
/// # Safety
/// `list` must point to a valid `List` whose items are `elem_size` bytes.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_list_reserve(list: *mut List, n: i64, elem_size: i64, align: i64) {
    // SAFETY: the caller guarantees `list`.
    let l = unsafe { &mut *list };
    // Before the room check, so a negative `n` (never made by compiled code) can't return and
    // reach a `SetLen`.
    if !items_fit(n) {
        list_too_long();
    }
    if n <= l.cap {
        return;
    }
    if l.arena.is_null() {
        l.arena = crate::arena::program_arena();
    }
    let size = (n * elem_size) as usize;
    // SAFETY: a list's arena outlives the list; inside a `par for`, other threads may be
    // allocating in the same arena. Room past the length is never read before it's written.
    let p = crate::par::shared_growth(l.arena, || unsafe { (*l.arena).alloc_uninit(size, align as usize) });
    if l.len > 0 {
        // SAFETY: the old buffer holds `len` items; the new one is fresh and bigger.
        unsafe { std::ptr::copy_nonoverlapping(l.ptr, p, (l.len * elem_size) as usize) };
    }
    l.ptr = p;
    l.cap = n;
}

/// Writes the string with these bytes at `dst`: its byte length as a `u32`, then the bytes. A null
/// pointer is the empty string. (Compiled code sizes the list first, and a list can't be longer
/// than `MAX_ITEMS` bytes, so the length fits.)
///
/// # Safety
/// `dst` must be valid for writing `4 + len` bytes, and `ptr` for reading `len` bytes unless it is
/// null.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_bytes_write_str(dst: *mut u8, ptr: *const u8, len: i64) {
    // SAFETY: the caller guarantees `ptr` for `len` bytes.
    let bytes = unsafe { crate::string::KStr::bytes(ptr, len) };
    let count = (bytes.len() as u32).to_le_bytes();
    // SAFETY: the caller guarantees room for the length and the bytes; the string's bytes are
    // never inside the list being written.
    unsafe {
        std::ptr::copy_nonoverlapping(count.as_ptr(), dst, 4);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst.add(4), bytes.len());
    }
}

/// Reads the `len` bytes at `src` as a new string in the current arena, written at `out`, and
/// gives 1; gives 0, with the empty string at `out`, when they aren't valid UTF-8. (Compiled code
/// has checked that the bytes are there.)
///
/// # Safety
/// `src` must be valid for reading `len` bytes unless `len` is 0, and `out` for writing 16 bytes.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_bytes_read_str(src: *const u8, len: i64, out: *mut KStr) -> i8 {
    // SAFETY: the caller guarantees `src` for `len` bytes.
    let bytes = unsafe { KStr::bytes(src, len) };
    let ok = std::str::from_utf8(bytes).is_ok();
    // SAFETY: the caller guarantees `out`.
    unsafe { KStr::make(if ok { bytes } else { &[] }, out) };
    i8::from(ok)
}

/// Why `from_bytes` failed: the codes compiled code passes to `kestrel_rt_bytes_reason` (the same
/// numbers as `kestrel_kir`'s `lower::bytes_read::why`).
pub mod reason {
    /// Fewer bytes than the header.
    pub const TOO_SHORT: i64 = 1;
    /// The first four bytes aren't `KESB`.
    pub const MAGIC: i64 = 2;
    /// Another format version; `at` is the version the bytes have.
    pub const VERSION: i64 = 3;
    /// Another type's fingerprint.
    pub const SHAPE: i64 = 4;
    /// A value needs bytes past the end; `at` is where it starts.
    pub const END_EARLY: i64 = 5;
    /// A bool's or an optional's byte isn't 0 or 1.
    pub const NOT_BOOL: i64 = 6;
    /// An enum's variant number is past its variants.
    pub const VARIANT: i64 = 7;
    /// A string's bytes aren't UTF-8.
    pub const NOT_UTF8: i64 = 8;
    /// A length is past `i32::MAX` or past what the bytes left can hold.
    pub const LENGTH: i64 = 9;
    /// A map has a key twice.
    pub const DUP_KEY: i64 = 10;
    /// The value ends before the bytes do; `at` is where it ends.
    pub const TRAILING: i64 = 11;
}

/// The sentence `from_bytes` gives for reason `code` (the language round, decision 13): `at` is
/// the byte the wrong value starts at (from 0; the version for `reason::VERSION`, where the value
/// ends for `reason::TRAILING`), `len` the input's length, and `name` the type read.
pub fn reason_text(code: i64, at: i64, len: i64, name: &str) -> String {
    let bytes = |n: i64| if n == 1 { "1 byte".to_string() } else { format!("{n} bytes") };
    match code {
        reason::TOO_SHORT => format!("the bytes are too short to hold a saved value: {}, and the header alone is {HEADER}", bytes(len)),
        reason::MAGIC => "the bytes aren't a saved value: they don't start with KESB".to_string(),
        reason::VERSION => format!("the bytes were saved in format {at}, and this Kestrel reads format {VERSION}"),
        reason::SHAPE => format!("the bytes hold another type, or another shape of `{name}`: a field was added, removed, renamed or changed"),
        reason::END_EARLY => format!("the bytes end early, at byte {at} of {len}"),
        reason::NOT_BOOL => format!("byte {at} should be 0 or 1"),
        reason::VARIANT => format!("the variant number at byte {at} is past the end of its enum"),
        reason::NOT_UTF8 => format!("the string at byte {at} isn't UTF-8 text"),
        reason::LENGTH => format!("the length at byte {at} is more than the bytes left can hold"),
        reason::DUP_KEY => format!("the map at byte {at} has a key twice"),
        reason::TRAILING => format!("the value ends at byte {at}, but there are {}", bytes(len)),
        _ => format!("the bytes aren't a saved `{name}`"),
    }
}

/// Writes `reason_text` at `out`, as a new string in the current arena; `name` (`name_len` bytes)
/// is the type read. Compiled code calls it only when a read has failed.
///
/// # Safety
/// `name` must be valid for reading `name_len` bytes of UTF-8 unless `name_len` is 0, and `out`
/// for writing 16 bytes.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_bytes_reason(code: i64, at: i64, len: i64, name: *const u8, name_len: i64, out: *mut KStr) {
    // SAFETY: the caller guarantees `name` for `name_len` bytes.
    let name = String::from_utf8_lossy(unsafe { KStr::bytes(name, name_len) });
    let text = reason_text(code, at, len, &name);
    // SAFETY: the caller guarantees `out`.
    unsafe { KStr::make(text.as_bytes(), out) };
}

/// A value for the reference encoder: the shape the byte format sees. A vector is a `Struct` of
/// its lanes; an array is a `List`.
#[derive(Clone, Debug, PartialEq)]
pub enum RefValue {
    Bool(bool),
    U8(u8),
    Int(i32),
    I64(i64),
    Float(f32),
    F64(f64),
    Str(String),
    List(Vec<RefValue>),
    Map(Vec<(RefValue, RefValue)>),
    Opt(Option<Box<RefValue>>),
    /// The variant number and that variant's fields.
    Enum(u32, Vec<RefValue>),
    Struct(Vec<RefValue>),
}

/// The bytes `to_bytes` writes for `value` of a type whose fingerprint is `fingerprint`.
pub fn encode(value: &RefValue, fingerprint: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER);
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&fingerprint.to_le_bytes());
    write_value(value, &mut out);
    out
}

/// `value`'s bytes, without a header, added to `out`.
pub fn write_value(value: &RefValue, out: &mut Vec<u8>) {
    let count = |n: usize, out: &mut Vec<u8>| out.extend_from_slice(&(n as u32).to_le_bytes());
    match value {
        RefValue::Bool(b) => out.push(u8::from(*b)),
        RefValue::U8(n) => out.push(*n),
        RefValue::Int(n) => out.extend_from_slice(&n.to_le_bytes()),
        RefValue::I64(n) => out.extend_from_slice(&n.to_le_bytes()),
        RefValue::Float(x) => out.extend_from_slice(&x.to_bits().to_le_bytes()),
        RefValue::F64(x) => out.extend_from_slice(&x.to_bits().to_le_bytes()),
        RefValue::Str(s) => {
            count(s.len(), out);
            out.extend_from_slice(s.as_bytes());
        }
        RefValue::List(items) => {
            count(items.len(), out);
            items.iter().for_each(|v| write_value(v, out));
        }
        RefValue::Map(entries) => {
            count(entries.len(), out);
            for (k, v) in entries {
                write_value(k, out);
                write_value(v, out);
            }
        }
        RefValue::Opt(None) => out.push(0),
        RefValue::Opt(Some(v)) => {
            out.push(1);
            write_value(v, out);
        }
        RefValue::Enum(variant, fields) => {
            out.extend_from_slice(&variant.to_le_bytes());
            fields.iter().for_each(|v| write_value(v, out));
        }
        RefValue::Struct(fields) => fields.iter().for_each(|v| write_value(v, out)),
    }
}

/// A type for the reference decoder: the shape the byte format sees, as `RefValue` is a value's. A
/// vector is a `Struct` of its lanes; an array is a `List`.
#[derive(Clone, Debug, PartialEq)]
pub enum RefTy {
    Bool,
    U8,
    Int,
    I64,
    Float,
    F64,
    Str,
    List(Box<RefTy>),
    Map(Box<RefTy>, Box<RefTy>),
    Opt(Box<RefTy>),
    /// Each variant's fields.
    Enum(Vec<Vec<RefTy>>),
    Struct(Vec<RefTy>),
}

impl RefTy {
    /// The fewest bytes a value of this type takes: the bound a length is checked against before
    /// anything is made for it. A struct is the sum of its fields, so an empty struct takes 0.
    pub fn min_size(&self) -> usize {
        match self {
            RefTy::Bool | RefTy::U8 | RefTy::Opt(_) => 1,
            RefTy::Int | RefTy::Float | RefTy::Str | RefTy::List(_) | RefTy::Map(..) | RefTy::Enum(_) => 4,
            RefTy::I64 | RefTy::F64 => 8,
            RefTy::Struct(fields) => fields.iter().map(RefTy::min_size).sum::<usize>(),
        }
    }
}

/// The value `bytes` hold, of type `ty` whose fingerprint is `fingerprint`, as `from_bytes` reads
/// it: `None` for a wrong magic, version or fingerprint, bytes that end early or go on after the
/// value, a bool or optional byte that isn't 0 or 1, a variant number past the enum, a string that
/// isn't UTF-8, a length past `i32::MAX` or past what the bytes left can hold, or a map key twice.
pub fn decode(bytes: &[u8], ty: &RefTy, fingerprint: i64) -> Option<RefValue> {
    let header = bytes.get(..HEADER)?;
    if header[..4] != MAGIC || header[4] != VERSION || header[5..] != fingerprint.to_le_bytes() {
        return None;
    }
    let mut at = HEADER;
    let value = read_value(bytes, &mut at, ty)?;
    (at == bytes.len()).then_some(value)
}

/// The value of type `ty` at byte `*at` of `bytes`, moving `*at` past it; `None` as for `decode`.
pub fn read_value(bytes: &[u8], at: &mut usize, ty: &RefTy) -> Option<RefValue> {
    fn take<const N: usize>(bytes: &[u8], at: &mut usize) -> Option<[u8; N]> {
        let word = bytes.get(*at..at.checked_add(N)?)?;
        *at += N;
        word.try_into().ok()
    }
    // A length or count: at most `i32::MAX`, and no more items than the bytes left can hold.
    let count = |at: &mut usize, item: usize| -> Option<usize> {
        let n = u32::from_le_bytes(take(bytes, at)?) as usize;
        (n <= i32::MAX as usize && n.checked_mul(item)? <= bytes.len() - *at).then_some(n)
    };
    Some(match ty {
        RefTy::Bool => match take::<1>(bytes, at)?[0] {
            b @ (0 | 1) => RefValue::Bool(b == 1),
            _ => return None,
        },
        RefTy::U8 => RefValue::U8(take::<1>(bytes, at)?[0]),
        RefTy::Int => RefValue::Int(i32::from_le_bytes(take(bytes, at)?)),
        RefTy::I64 => RefValue::I64(i64::from_le_bytes(take(bytes, at)?)),
        RefTy::Float => RefValue::Float(f32::from_bits(u32::from_le_bytes(take(bytes, at)?))),
        RefTy::F64 => RefValue::F64(f64::from_bits(u64::from_le_bytes(take(bytes, at)?))),
        RefTy::Str => {
            let n = count(at, 1)?;
            let text = std::str::from_utf8(&bytes[*at..*at + n]).ok()?;
            *at += n;
            RefValue::Str(text.to_string())
        }
        RefTy::List(item) if item.min_size() == 0 => {
            // Items of 0 bytes (empty structs) read nothing: any length of them is the same value
            // repeated. (Compiled code makes no memory for them; this makes a `RefValue` each, so
            // tests keep such lengths small.)
            let n = count(at, 0)?;
            let unit = read_value(bytes, at, item)?;
            RefValue::List(vec![unit; n])
        }
        RefTy::List(item) => {
            let n = count(at, item.min_size())?;
            RefValue::List((0..n).map(|_| read_value(bytes, at, item)).collect::<Option<_>>()?)
        }
        RefTy::Map(k, v) => {
            let n = count(at, k.min_size() + v.min_size())?;
            // Room for no more entries than bytes left: when a key and its value both take 0 bytes
            // (`map[Unit, Unit]`) the count is bounded only by `i32::MAX`.
            let mut entries: Vec<(RefValue, RefValue)> = Vec::with_capacity(n.min(bytes.len() - *at));
            for _ in 0..n {
                let key = read_value(bytes, at, k)?;
                if entries.iter().any(|(e, _)| *e == key) {
                    return None;
                }
                entries.push((key, read_value(bytes, at, v)?));
            }
            RefValue::Map(entries)
        }
        RefTy::Opt(inner) => match take::<1>(bytes, at)?[0] {
            0 => RefValue::Opt(None),
            1 => RefValue::Opt(Some(Box::new(read_value(bytes, at, inner)?))),
            _ => return None,
        },
        RefTy::Enum(variants) => {
            let v = u32::from_le_bytes(take(bytes, at)?);
            let fields = variants.get(v as usize)?;
            RefValue::Enum(v, fields.iter().map(|f| read_value(bytes, at, f)).collect::<Option<_>>()?)
        }
        RefTy::Struct(fields) => RefValue::Struct(fields.iter().map(|f| read_value(bytes, at, f)).collect::<Option<_>>()?),
    })
}

#[cfg(test)]
#[path = "bytes_test.rs"]
mod tests;
