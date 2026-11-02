//! `from_bytes[T](b)`: the value `to_bytes` wrote (see `bytes` and the runtime's `bytes` for the
//! format), or `none` when the bytes are anything else. A generated function per type,
//! `__kestrel_from_bytes_N(b: list[u8]) -> T?` (see `bytes_header`), checks the 13-byte header,
//! reads the value with the functions here, and gives it when the value ends exactly where the
//! bytes do:
//!
//! - `__kestrel_bytes_read_N(src: list[u8], at: i64, var out: T) -> i64` reads a `T` from byte
//!   `at` of `src` into `out` and gives the byte after it, or -1 when the bytes are wrong. A
//!   number is one `ReadBytes`; a bool or an optional's byte must be 0 or 1, an enum's variant
//!   number one of its variants. A struct is its fields in order (one bounds check for a struct of
//!   numbers); an enum's fields and an optional's payload are read into temporaries, then stored
//!   whole. An array, list or map always goes through its own function, so types that contain
//!   themselves through a collection work, as `to_bytes`'s do.
//! - Every read checks that its bytes are there first. A length or count is checked, before
//!   anything is made for it, to be at most `i32::MAX` and at most the bytes left divided by the
//!   fewest bytes an item takes (`min_size`). A list is made with exactly its length as its room,
//!   unless its items take more than 4 times as much memory as bytes (`grows`): then it grows as
//!   they are read, so a hostile length never makes more than 4 times the input's size up front.
//!   Items of 0 bytes take no room and are not read at all. A string
//!   must be valid UTF-8 (`ReadBytes` gives the empty string otherwise). A map is built by adding
//!   each key, and a key already there fails it.
//!
//! On `none`, whatever was built so far stays in the current arena until the arena is freed.
//! Offsets never overflow: `at` stays between 0 and the input's length, which a list keeps below
//! `i32::MAX`, so the sums are wrapping and nothing is checked.

use kestrel_check::Tables;

use super::bytes::{fixed_size, helper, int, load, local, number, word_size, wrapping};
use super::helpers::{elem_of, expr, with, Helpers};
use super::map_helpers::key_value;
use super::{kty, layout};
use crate::ir::*;

// The locals of a read function: the bytes, the next byte, the value read, a count and a loop
// index; temporaries follow.
const R_SRC: LocalIdx = LocalIdx(0);
const R_AT: LocalIdx = LocalIdx(1);
const R_OUT: LocalIdx = LocalIdx(2);
const R_N: LocalIdx = LocalIdx(3);
const R_I: LocalIdx = LocalIdx(4);

/// The fewest bytes a value of type `ty` takes: what a claimed length is checked against. A struct
/// is the sum of its fields, so an empty struct (or one of empty structs) takes 0: a list of those
/// is read with no loop (see `bytes_read_fn`).
fn min_size(ty: KTy, t: &Tables) -> i64 {
    match ty {
        KTy::Struct(s) => t.structs[s.0 as usize].fields.iter().map(|f| min_size(kty(f.ty), t)).sum::<i64>(),
        KTy::Str | KTy::Array(_) | KTy::List(_) | KTy::Map(..) | KTy::Enum(_) => 4,
        KTy::Optional(_) => 1,
        _ => word_size(ty).unwrap_or_else(|| unreachable!("the checker refuses `from_bytes` of a {ty:?}")),
    }
}

fn src() -> Place {
    Place::local(R_SRC)
}

fn at() -> Expr {
    local(R_AT)
}

pub(super) fn len(p: &Place) -> Expr {
    expr(ExprKind::Len(p.clone()), KTy::I64)
}

pub(super) fn cmp(op: BinOp, a: Expr, b: Expr) -> Expr {
    expr(ExprKind::Binary(op, Box::new(a), Box::new(b)), KTy::Bool)
}

/// `if cond { return -1 }`
fn fail_if(cond: Expr) -> Stmt {
    Stmt::If { cond, then_body: vec![Stmt::Return(Some(int(-1, KTy::I64)))], else_body: Vec::new() }
}

/// Fails unless `n` more bytes are left after `at`.
fn need(n: Expr) -> Stmt {
    fail_if(cmp(BinOp::Gt, n, left()))
}

/// The bytes left after `at`.
fn left() -> Expr {
    wrapping(BinOp::SubWrap, len(&src()), at())
}

/// The value of type `ty` at `at`.
fn read(ty: KTy) -> Expr {
    expr(ExprKind::ReadBytes(src(), vec![at()]), ty)
}

/// `at` moved past `n` bytes.
fn skip(n: Expr) -> Stmt {
    Stmt::Assign(Place::local(R_AT), wrapping(BinOp::AddWrap, at(), n))
}

/// The `u32` at `at`, widened, into the local `n`, then past it; fails unless it is at most
/// `i32::MAX` and that many items of at least `min` bytes fit in the bytes left (any number of
/// items of 0 bytes do).
fn count(n: LocalIdx, min: i64) -> Vec<Stmt> {
    let mut out = vec![
        need(int(4, KTy::I64)),
        Stmt::Let(n, expr(ExprKind::Cast(Box::new(read(KTy::I32))), KTy::I64)),
        skip(int(4, KTy::I64)),
        // Read as an `int`, a length past `i32::MAX` is negative.
        fail_if(cmp(BinOp::Lt, local(n), int(0, KTy::I64))),
    ];
    if min > 0 {
        // At most `i32::MAX` items of at most a few thousand bytes: the product fits.
        out.push(fail_if(cmp(BinOp::Gt, wrapping(BinOp::MulWrap, local(n), int(min, KTy::I64)), left())));
    }
    out
}

/// Whether the items of a list or array of `elem` are read one by one into a list that grows as
/// they come, instead of into room made up front for the claimed length: when an item's size in
/// memory is more than 4 times the fewest bytes it is written in, room for a hostile length would
/// be a large multiple of the input's size.
fn grows(elem: KTy, t: &Tables) -> bool {
    i64::from(layout::size_align(t, elem).0) > 4 * min_size(elem, t)
}

/// The locals of the read function being built.
struct Frame {
    locals: Vec<Local>,
}

impl Frame {
    /// A new temporary: in memory when it is an aggregate.
    fn temp(&mut self, name: &str, ty: KTy) -> LocalIdx {
        let kind = if ty.is_aggregate() { LocalKind::Addressed } else { LocalKind::Value };
        self.locals.push(Local { name: name.into(), ty, kind });
        LocalIdx(self.locals.len() as u32 - 1)
    }
}

impl Helpers {
    /// Statements that read a value of type `ty` at `at` into the place `p`, move `at` past it, and
    /// return -1 when the bytes are wrong. `room` says the bytes are known to be there (a struct of
    /// numbers checked whole, or an item of a list whose length was checked against its size).
    fn read_stmts(&mut self, fr: &mut Frame, p: Place, ty: KTy, room: bool, list: KTy, t: &Tables) -> Vec<Stmt> {
        let mut out = Vec::new();
        match ty {
            KTy::Bool => {
                let byte = fr.temp("byte", KTy::U8);
                if !room {
                    out.push(need(int(1, KTy::I64)));
                }
                let b = || load(Place::local(byte), KTy::U8);
                out.push(Stmt::Let(byte, read(KTy::U8)));
                out.push(fail_if(cmp(BinOp::Gt, b(), int(1, KTy::U8))));
                out.push(Stmt::Assign(p, cmp(BinOp::Ne, b(), int(0, KTy::U8))));
                out.push(skip(int(1, KTy::I64)));
            }
            _ if word_size(ty).is_some() => {
                let n = int(word_size(ty).unwrap_or(0), KTy::I64);
                if !room {
                    out.push(need(n.clone()));
                }
                out.push(Stmt::Assign(p, read(ty)));
                out.push(skip(n));
            }
            KTy::Str => {
                let n = fr.temp("len", KTy::I64);
                out.extend(count(n, 1));
                out.push(Stmt::Assign(p.clone(), expr(ExprKind::ReadBytes(src(), vec![at(), local(n)]), KTy::Str)));
                // Bytes that aren't UTF-8 read as the empty string.
                out.push(fail_if(cmp(BinOp::Ne, len(&p), local(n))));
                out.push(skip(local(n)));
            }
            KTy::Struct(s) => {
                let mut room = room;
                if let (Some(n), false) = (fixed_size(ty, t), room) {
                    out.push(need(int(n, KTy::I64)));
                    room = true;
                }
                for (i, f) in t.structs[s.0 as usize].fields.iter().enumerate() {
                    out.extend(self.read_stmts(fr, with(&p, Proj::Field(i as u32)), kty(f.ty), room, list, t));
                }
            }
            KTy::Enum(e) => {
                let v = fr.temp("variant", KTy::I64);
                let variants = &t.enums[e.0 as usize].variants;
                out.push(need(int(4, KTy::I64)));
                out.push(Stmt::Let(v, expr(ExprKind::Cast(Box::new(read(KTy::I32))), KTy::I64)));
                out.push(skip(int(4, KTy::I64)));
                let below = cmp(BinOp::Lt, local(v), int(0, KTy::I64));
                let past = cmp(BinOp::Ge, local(v), int(variants.len() as i64, KTy::I64));
                out.push(fail_if(expr(ExprKind::Or(Box::new(below), Box::new(past)), KTy::Bool)));
                for (n, variant) in variants.iter().enumerate() {
                    let (mut then_body, mut fields) = (Vec::new(), Vec::new());
                    for f in &variant.fields {
                        let fty = kty(f.ty);
                        let tmp = fr.temp("field", fty);
                        then_body.extend(self.read_stmts(fr, Place::local(tmp), fty, false, list, t));
                        fields.push(load(Place::local(tmp), fty));
                    }
                    then_body.push(Stmt::Assign(p.clone(), expr(ExprKind::VariantLit(e, n as u32, fields), ty)));
                    let cond = cmp(BinOp::Eq, local(v), int(n as i64, KTy::I64));
                    out.push(Stmt::If { cond, then_body, else_body: Vec::new() });
                }
            }
            KTy::Optional(e) => {
                let pty = kty(t.elems[e.0 as usize]);
                let byte = fr.temp("byte", KTy::U8);
                let b = || load(Place::local(byte), KTy::U8);
                out.push(need(int(1, KTy::I64)));
                out.push(Stmt::Let(byte, read(KTy::U8)));
                out.push(skip(int(1, KTy::I64)));
                let payload = fr.temp("payload", pty);
                let mut some = self.read_stmts(fr, Place::local(payload), pty, false, list, t);
                some.push(Stmt::Assign(p.clone(), expr(ExprKind::Some(Box::new(load(Place::local(payload), pty))), ty)));
                let one = Stmt::If { cond: cmp(BinOp::Eq, b(), int(1, KTy::U8)), then_body: some, else_body: vec![fail_if(expr(ExprKind::Bool(true), KTy::Bool))] };
                let none = vec![Stmt::Assign(p, expr(ExprKind::NoneLit, ty))];
                out.push(Stmt::If { cond: cmp(BinOp::Eq, b(), int(0, KTy::U8)), then_body: none, else_body: vec![one] });
            }
            KTy::Array(_) | KTy::List(_) | KTy::Map(..) => {
                let f = self.bytes_read_fn(ty, list, t);
                let args = vec![Arg::Value(load(src(), list)), Arg::Value(at()), Arg::Ref(p)];
                out.push(Stmt::Assign(Place::local(R_AT), expr(ExprKind::Call(f, args), KTy::I64)));
                out.push(fail_if(cmp(BinOp::Lt, at(), int(0, KTy::I64))));
            }
            _ => unreachable!("the checker refuses `from_bytes` of a value holding a {ty:?}"),
        }
        out
    }

    /// `__kestrel_bytes_read_N(src: list[u8], at: i64, var out: T) -> i64`: reads a `T` from byte
    /// `at` of `src` (of type `list`, `list[u8]`) into `out`, and gives the byte after it, or -1 when
    /// the bytes are wrong.
    pub(crate) fn bytes_read_fn(&mut self, ty: KTy, list: KTy, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.bytes_read.get(&ty) {
            return f;
        }
        let idx = self.reserve();
        self.bytes_read.insert(ty, idx);
        let locals = vec![
            Local { name: "src".into(), ty: list, kind: LocalKind::RefParam },
            number("at"),
            Local { name: "out".into(), ty, kind: LocalKind::RefParam },
            number("n"),
            number("i"),
        ];
        let mut fr = Frame { locals };
        let out = Place::local(R_OUT);
        let n = || local(R_N);
        let mut body = Vec::new();
        match ty {
            KTy::Array(e) | KTy::List(e) => {
                let elem = elem_of(ty, t);
                let min = min_size(elem, t);
                body.extend(count(R_N, min));
                // Items of a fixed size are all there: the length was checked against it.
                let room = fixed_size(elem, t).is_some();
                let index = |p: &Place| with(p, Proj::Index { at: local(R_I), check: IndexCheck::Known });
                if min > 0 && grows(elem, t) {
                    // Into a list that grows as the items come (`out` itself for a list), so a
                    // hostile length makes nothing the bytes don't hold; an array is then copied
                    // from it into one of exactly its length.
                    let items = if matches!(ty, KTy::List(_)) { out.clone() } else { Place::local(fr.temp("items", KTy::List(e))) };
                    body.push(Stmt::Assign(items.clone(), expr(ExprKind::ListNew, KTy::List(e))));
                    let item = fr.temp("item", elem);
                    let mut each = self.read_stmts(&mut fr, Place::local(item), elem, room, list, t);
                    each.push(Stmt::ListAdd(items.clone(), load(Place::local(item), elem)));
                    body.push(Stmt::ForRange { var: R_I, start: int(0, KTy::I64), end: n(), body: each });
                    if matches!(ty, KTy::Array(_)) {
                        body.push(Stmt::Assign(out.clone(), expr(ExprKind::ArrayNew(Box::new(n())), ty)));
                        let copy = vec![Stmt::Assign(index(&out), load(index(&items), elem))];
                        body.push(Stmt::ForRange { var: R_I, start: int(0, KTy::I64), end: n(), body: copy });
                    }
                } else {
                    if matches!(ty, KTy::List(_)) {
                        body.push(Stmt::Assign(out.clone(), expr(ExprKind::ListNew, ty)));
                        body.push(Stmt::List(ListOp::Reserve, out.clone(), vec![n()]));
                        // The room is exactly `n`, and every item is written before anything reads it.
                        body.push(Stmt::SetLen(out.clone(), n()));
                    } else {
                        body.push(Stmt::Assign(out.clone(), expr(ExprKind::ArrayNew(Box::new(n())), ty)));
                    }
                    // Items of 0 bytes have nothing to read (and room for them takes no memory),
                    // so any length is read with no loop. Otherwise the loop never changes the
                    // list's length, so `i` stays below it.
                    if min > 0 {
                        let each = self.read_stmts(&mut fr, index(&out), elem, room, list, t);
                        body.push(Stmt::ForRange { var: R_I, start: int(0, KTy::I64), end: n(), body: each });
                    }
                }
            }
            KTy::Map(..) => {
                let (k, v) = key_value(ty, t);
                body.extend(count(R_N, min_size(k, t) + min_size(v, t)));
                body.push(Stmt::Assign(out.clone(), expr(ExprKind::MapNew, ty)));
                let (key, value, before) = (fr.temp("key", k), fr.temp("value", v), fr.temp("before", KTy::I64));
                let mut each = self.read_stmts(&mut fr, Place::local(key), k, false, list, t);
                each.extend(self.read_stmts(&mut fr, Place::local(value), v, false, list, t));
                each.push(Stmt::Let(before, len(&out)));
                each.push(Stmt::MapSet(out.clone(), load(Place::local(key), k), load(Place::local(value), v), false));
                // A key already there replaced its value instead of adding an entry.
                each.push(fail_if(cmp(BinOp::Eq, len(&out), local(before))));
                body.push(Stmt::ForRange { var: R_I, start: int(0, KTy::I64), end: n(), body: each });
            }
            _ => body.extend(self.read_stmts(&mut fr, out, ty, false, list, t)),
        }
        body.push(Stmt::Return(Some(at())));
        let name = format!("__kestrel_bytes_read_{}", idx.0);
        self.funcs[(idx.0 - self.base) as usize] = helper(name, vec![R_SRC, R_AT, R_OUT], KTy::I64, fr.locals, body, true);
        idx
    }
}
