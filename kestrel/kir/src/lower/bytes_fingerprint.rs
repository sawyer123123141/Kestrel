//! The fingerprint `to_bytes` writes in its header (the I/O round's spec, decision 2): the
//! `checksum` of the type's canonical description, made at compile time.
//!
//! The description writes scalars by name (`int`, `f64`, `string`, `vec4`; the built-in `vec2`,
//! `vec3`, `ivec2` and `ivec3` too), `list[T]`, `array[T]`, `map[K,V]` and `T?`, a struct as
//! `struct Game{ball:vec2,score:int}` and an enum as `enum State{Alive,Dead{since:int}}`. A struct
//! or enum met again inside itself is written as how far out it is among the types being
//! described: `^1` for the innermost, `^2` for the one around it (`struct Node{kids:list[^1]}`). A
//! name would not do: two mutually recursive `Node`s from different modules would describe the
//! same as one `Node` holding itself. Renaming or reordering fields
//! changes it, so bytes of one shape are never read as another. A type is named as it is
//! declared, without the module it lives in (`Point`, never `geom.Point`): the shape is what must
//! match, not the file, so a type moved into a module still reads the saves it wrote. A GPU object
//! (which only a reloadable program's state description meets) is `gpu`.

use kestrel_check::Tables;

use super::kty;
use crate::ir::KTy;

/// The fold's starting value and multiplier (the runtime's `checksum::START` and `MULTIPLIER`).
const START: u64 = 0x243F_6A88_85A3_08D3;
const MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix(h: u64, w: u64) -> u64 {
    let x = (h ^ w).wrapping_mul(MULTIPLIER);
    x ^ (x >> 32)
}

/// `checksum` of a string with these bytes, as compiled code computes it (the runtime's
/// `finish(mix_bytes(START, bytes))`).
pub(super) fn checksum_of_text(bytes: &[u8]) -> i64 {
    let mut h = mix(START, bytes.len() as u64);
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        h = mix(h, u64::from_le_bytes(word));
    }
    let mut z = h;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as i64
}

/// The fingerprint of type `ty`: the `checksum` of its `description`, which names types without
/// their modules.
pub(crate) fn fingerprint(ty: KTy, t: &Tables) -> i64 {
    checksum_of_text(description(ty, t).as_bytes())
}

/// The canonical description of type `ty` (see the module comment).
pub(crate) fn description(ty: KTy, t: &Tables) -> String {
    let mut out = String::new();
    describe(ty, t, &mut Vec::new(), &mut out);
    out
}

/// `ty`'s description added to `out`; `inside` holds the structs and enums being described.
fn describe(ty: KTy, t: &Tables, inside: &mut Vec<KTy>, out: &mut String) {
    let elem = |e: crate::ir::ElemIdx| kty(t.elems[e.0 as usize]);
    match ty {
        KTy::Bool => out.push_str("bool"),
        KTy::U8 => out.push_str("u8"),
        KTy::I32 => out.push_str("int"),
        KTy::I64 => out.push_str("i64"),
        KTy::F32 => out.push_str("float"),
        KTy::F64 => out.push_str("f64"),
        KTy::F32X4 => out.push_str("vec4"),
        KTy::I32X4 => out.push_str("ivec4"),
        KTy::Str => out.push_str("string"),
        KTy::List(e) | KTy::Array(e) => {
            out.push_str(if matches!(ty, KTy::List(_)) { "list[" } else { "array[" });
            describe(elem(e), t, inside, out);
            out.push(']');
        }
        KTy::Map(k, v) => {
            out.push_str("map[");
            describe(elem(k), t, inside, out);
            out.push(',');
            describe(elem(v), t, inside, out);
            out.push(']');
        }
        KTy::Optional(e) => {
            describe(elem(e), t, inside, out);
            out.push('?');
        }
        KTy::Struct(s) => {
            let info = &t.structs[s.0 as usize];
            // A built-in struct (`vec2`...) is a scalar by name.
            if info.item == usize::MAX {
                out.push_str(&own_name(&info.name));
                return;
            }
            if back_reference(ty, inside, out) {
                return;
            }
            inside.push(ty);
            out.push_str("struct ");
            out.push_str(&own_name(&info.name));
            fields(info.fields.iter().map(|f| (f.name.as_str(), kty(f.ty))), t, inside, out);
            inside.pop();
        }
        KTy::Enum(e) => {
            let info = &t.enums[e.0 as usize];
            if back_reference(ty, inside, out) {
                return;
            }
            inside.push(ty);
            out.push_str("enum ");
            out.push_str(&own_name(&info.name));
            out.push('{');
            for (i, v) in info.variants.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&v.name);
                if !v.fields.is_empty() {
                    fields(v.fields.iter().map(|f| (f.name.as_str(), kty(f.ty))), t, inside, out);
                }
            }
            out.push('}');
            inside.pop();
        }
        KTy::Gpu => out.push_str("gpu"),
        KTy::Arena | KTy::Fn | KTy::Iface => unreachable!("the checker refuses `to_bytes` and a reload state holding a {ty:?}"),
    }
}

/// Whether `ty` is one of the types being described (`inside`); if so, `^n` added to `out`, where
/// `n` counts from the innermost (1) outward.
fn back_reference(ty: KTy, inside: &[KTy], out: &mut String) -> bool {
    let Some(n) = inside.iter().rev().position(|t| *t == ty) else { return false };
    out.push('^');
    out.push_str(&(n + 1).to_string());
    true
}

/// A struct's or enum's name without the modules the linker puts before it, in a generic copy's
/// type arguments too: `geom.Stack[geom.Point, int]` is `Stack[Point, int]`.
pub(super) fn own_name(name: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            // A word followed by a dot is a module.
            if c != '.' {
                out.push_str(&word);
                out.push(c);
            }
            word.clear();
        }
    }
    out.push_str(&word);
    out
}

/// `{name:type,...}` added to `out`.
fn fields<'a>(fields: impl Iterator<Item = (&'a str, KTy)>, t: &Tables, inside: &mut Vec<KTy>, out: &mut String) {
    out.push('{');
    for (i, (name, ty)) in fields.enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(name);
        out.push(':');
        describe(ty, t, inside, out);
    }
    out.push('}');
}

#[cfg(test)]
#[path = "bytes_test.rs"]
mod tests;
