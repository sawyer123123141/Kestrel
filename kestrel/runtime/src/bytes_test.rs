use super::*;
use crate::arena::{kestrel_rt_arena_free, kestrel_rt_arena_new, kestrel_rt_arena_pop, kestrel_rt_arena_push};

fn s(text: &str) -> RefValue {
    RefValue::Str(text.to_string())
}

#[test]
fn the_header_is_the_magic_the_version_and_the_fingerprint() {
    let bytes = encode(&RefValue::Bool(true), 0x0102_0304_0506_0708);
    assert_eq!(&bytes[..4], b"KESB");
    assert_eq!(bytes[4], 1);
    assert_eq!(&bytes[5..13], &[8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(&bytes[13..], &[1]);
    assert_eq!(bytes.len(), HEADER + 1);
}

#[test]
fn values_are_little_endian_without_padding() {
    let cases: Vec<(RefValue, Vec<u8>)> = vec![
        (RefValue::Bool(false), vec![0]),
        (RefValue::U8(200), vec![200]),
        (RefValue::Int(-2), vec![254, 255, 255, 255]),
        (RefValue::I64(258), vec![2, 1, 0, 0, 0, 0, 0, 0]),
        (RefValue::Float(1.5), vec![0, 0, 192, 63]),
        (RefValue::F64(0.5), vec![0, 0, 0, 0, 0, 0, 224, 63]),
        (s("h\u{e9}"), vec![3, 0, 0, 0, 104, 195, 169]),
        (RefValue::List(vec![RefValue::U8(1), RefValue::U8(2)]), vec![2, 0, 0, 0, 1, 2]),
        (RefValue::Map(vec![(s("a"), RefValue::U8(9))]), vec![1, 0, 0, 0, 1, 0, 0, 0, 97, 9]),
        (RefValue::Opt(None), vec![0]),
        (RefValue::Opt(Some(Box::new(RefValue::U8(5)))), vec![1, 5]),
        (RefValue::Enum(2, vec![RefValue::U8(3), RefValue::U8(4)]), vec![2, 0, 0, 0, 3, 4]),
        (RefValue::Struct(vec![RefValue::U8(1), RefValue::Int(1)]), vec![1, 1, 0, 0, 0]),
    ];
    for (value, want) in cases {
        let mut out = Vec::new();
        write_value(&value, &mut out);
        assert_eq!(out, want, "{value:?}");
    }
}

#[test]
fn a_nan_keeps_its_payload() {
    let mut out = Vec::new();
    write_value(&RefValue::Float(f32::from_bits(0x7FC0_0001)), &mut out);
    assert_eq!(out, vec![1, 0, 192, 127]);
}

#[test]
fn the_fingerprint_is_the_checksum_of_the_description() {
    let desc = "struct Rng{seed:i64}";
    assert_eq!(fingerprint(desc), finish(mix_bytes(START, desc.as_bytes())) as i64);
    assert_ne!(fingerprint("struct P{a:int,b:int}"), fingerprint("struct P{b:int,a:int}"));
}

#[test]
fn reserving_gives_exactly_the_room_asked_for_and_keeps_the_items() {
    // SAFETY: the arena is pushed while the list is used and freed after.
    unsafe {
        let arena = kestrel_rt_arena_new();
        kestrel_rt_arena_push(arena);
        let mut l = List { ptr: std::ptr::null_mut(), len: 0, cap: 0, arena };
        kestrel_rt_list_reserve(&mut l, 3, 4, 4);
        assert_eq!((l.len, l.cap), (0, 3));
        l.ptr.cast::<i32>().write(7);
        l.len = 1;
        kestrel_rt_list_reserve(&mut l, 13, 4, 4);
        assert_eq!((l.len, l.cap, l.ptr.cast::<i32>().read()), (1, 13, 7));
        // Enough room already: nothing changes.
        let before = l.ptr;
        kestrel_rt_list_reserve(&mut l, 5, 4, 4);
        assert_eq!((l.cap, l.ptr), (13, before));
        kestrel_rt_arena_pop();
        kestrel_rt_arena_free(arena);
    }
}

/// Set for the child process of `a_negative_room_fails_instead_of_returning`.
const RESERVE_CHILD: &str = "KESTREL_TEST_RESERVE_CHILD";

/// The child half of `a_negative_room_fails_instead_of_returning`: does nothing unless the parent
/// set `RESERVE_CHILD`. Reserving room for -1 items must stop the program (exit 101), so getting
/// past the call exits 0.
#[test]
fn reserve_negative_child() {
    if std::env::var_os(RESERVE_CHILD).is_none() {
        return;
    }
    let mut l = List { ptr: std::ptr::null_mut(), len: 0, cap: 0, arena: std::ptr::null_mut() };
    // SAFETY: `l` is a valid empty list of 4-byte items.
    unsafe { kestrel_rt_list_reserve(&mut l, -1, 4, 4) };
    std::process::exit(0);
}

#[test]
fn a_negative_room_fails_instead_of_returning() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["bytes::tests::reserve_negative_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(RESERVE_CHILD, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(101), "{stderr}");
    assert!(stderr.contains("kestrel: a list can't hold"), "{stderr}");
}

#[test]
fn a_string_is_written_as_its_length_then_its_bytes() {
    let mut buf = [0xAAu8; 9];
    let text = "h\u{e9}";
    // SAFETY: `buf` has room for the length and the three bytes.
    unsafe { kestrel_rt_bytes_write_str(buf.as_mut_ptr().add(1), text.as_ptr(), text.len() as i64) };
    assert_eq!(buf, [0xAA, 3, 0, 0, 0, 104, 195, 169, 0xAA]);
    // A null pointer is the empty string.
    // SAFETY: as above.
    unsafe { kestrel_rt_bytes_write_str(buf.as_mut_ptr(), std::ptr::null(), 0) };
    assert_eq!(&buf[..5], &[0, 0, 0, 0, 0]);
}

fn list(ty: RefTy) -> RefTy {
    RefTy::List(Box::new(ty))
}

fn opt(ty: RefTy) -> RefTy {
    RefTy::Opt(Box::new(ty))
}

/// `value` written and read back as a `ty`.
fn round_trip(value: &RefValue, ty: &RefTy) -> Option<RefValue> {
    decode(&encode(value, 77), ty, 77)
}

#[test]
fn decode_reads_back_what_encode_wrote() {
    let shape = RefTy::Enum(vec![vec![], vec![RefTy::Float], vec![RefTy::U8, RefTy::U8]]);
    let cases: Vec<(RefValue, RefTy)> = vec![
        (RefValue::Bool(true), RefTy::Bool),
        (RefValue::U8(200), RefTy::U8),
        (RefValue::Int(-2), RefTy::Int),
        (RefValue::I64(i64::MIN), RefTy::I64),
        (RefValue::Float(f32::from_bits(0x7FC0_0001)), RefTy::Float),
        (RefValue::F64(-0.0), RefTy::F64),
        (s("h\u{e9}"), RefTy::Str),
        (RefValue::List(vec![RefValue::U8(1), RefValue::U8(2)]), list(RefTy::U8)),
        (RefValue::Map(vec![(s("b"), RefValue::Int(1)), (s("a"), RefValue::Int(2))]), RefTy::Map(Box::new(RefTy::Str), Box::new(RefTy::Int))),
        (RefValue::Opt(None), opt(RefTy::Int)),
        (RefValue::Opt(Some(Box::new(s("x")))), opt(RefTy::Str)),
        (RefValue::Enum(2, vec![RefValue::U8(3), RefValue::U8(4)]), shape.clone()),
        (RefValue::List(vec![RefValue::Enum(0, vec![]), RefValue::Enum(1, vec![RefValue::Float(1.5)])]), list(shape)),
        (RefValue::Struct(vec![RefValue::Bool(false), RefValue::List(vec![])]), RefTy::Struct(vec![RefTy::Bool, list(RefTy::Str)])),
    ];
    for (value, ty) in cases {
        let back = round_trip(&value, &ty).unwrap_or_else(|| panic!("{value:?} didn't read back"));
        assert_eq!(encode(&back, 77), encode(&value, 77), "{value:?}");
    }
}

#[test]
fn a_struct_ending_in_an_empty_struct_reads_back_at_the_end_of_the_input() {
    // `Tagged { id: int, k: Unit }` is written in 4 bytes, the fewest its type takes.
    let tagged = RefTy::Struct(vec![RefTy::Int, RefTy::Struct(vec![])]);
    assert_eq!((tagged.min_size(), RefTy::Struct(vec![]).min_size()), (4, 0));
    let item = |id| RefValue::Struct(vec![RefValue::Int(id), RefValue::Struct(vec![])]);
    let tags = RefValue::List(vec![item(1), item(2)]);
    assert_eq!(round_trip(&tags, &list(tagged)), Some(tags));
    let units = RefValue::Map(vec![(s("a"), RefValue::Struct(vec![])), (s("b"), RefValue::Struct(vec![]))]);
    assert_eq!(round_trip(&units, &RefTy::Map(Box::new(RefTy::Str), Box::new(RefTy::Struct(vec![])))), Some(units));
}

#[test]
fn decode_refuses_every_wrong_input() {
    let good = encode(&RefValue::Int(7), 77);
    assert!(decode(&good, &RefTy::Int, 77).is_some());
    // The header: the magic, the version, the fingerprint.
    for (at, byte) in [(0, b'X'), (4, 2), (5, 76)] {
        let mut bad = good.clone();
        bad[at] = byte;
        assert_eq!(decode(&bad, &RefTy::Int, 77), None, "byte {at}");
    }
    assert_eq!(decode(&good, &RefTy::Int, 78), None);
    // Every truncation, and one byte too many.
    for n in 0..good.len() {
        assert_eq!(decode(&good[..n], &RefTy::Int, 77), None);
    }
    let mut long = good.clone();
    long.push(0);
    assert_eq!(decode(&long, &RefTy::Int, 77), None);
    // A bool or optional byte of 2, a variant past the enum.
    let body = |b: &[u8]| [&good[..13], b].concat();
    assert_eq!(decode(&body(&[2]), &RefTy::Bool, 77), None);
    assert_eq!(decode(&body(&[2, 1, 0, 0, 0]), &opt(RefTy::Int), 77), None);
    assert_eq!(decode(&body(&[2, 0, 0, 0]), &RefTy::Enum(vec![vec![], vec![]]), 77), None);
    // A string that isn't UTF-8 (a lone 0xFF, an encoded surrogate).
    assert_eq!(decode(&body(&[1, 0, 0, 0, 0xFF]), &RefTy::Str, 77), None);
    assert_eq!(decode(&body(&[3, 0, 0, 0, 0xED, 0xA0, 0x80]), &RefTy::Str, 77), None);
    // Lengths past `i32::MAX`, or past what the bytes left can hold.
    assert_eq!(decode(&body(&[0xFF, 0xFF, 0xFF, 0x7F, 0, 0, 0]), &list(RefTy::U8), 77), None);
    assert_eq!(decode(&body(&[0, 0, 0, 0x80]), &list(RefTy::Struct(vec![])), 77), None);
    assert_eq!(decode(&body(&[2, 0, 0, 0, 1, 0, 0, 0]), &list(RefTy::Int), 77), None);
    // An empty struct takes no bytes, so any length up to `i32::MAX` of them is read.
    let units = decode(&body(&[3, 0, 0, 0]), &list(RefTy::Struct(vec![])), 77);
    assert_eq!(units, Some(RefValue::List(vec![RefValue::Struct(vec![]); 3])));
    // A key twice.
    let m = RefValue::Map(vec![(RefValue::U8(1), RefValue::U8(2)), (RefValue::U8(1), RefValue::U8(3))]);
    assert_eq!(round_trip(&m, &RefTy::Map(Box::new(RefTy::U8), Box::new(RefTy::U8))), None);
    // A map of 0-byte keys and values claiming `i32::MAX` entries has its second key twice, and
    // gets no room for the claim first.
    let unit = || Box::new(RefTy::Struct(vec![]));
    assert_eq!(decode(&body(&[0xFF, 0xFF, 0xFF, 0x7F]), &RefTy::Map(unit(), unit()), 77), None);
}

/// A fixed-seed 64-bit linear congruential generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A random value of type `ty`, with lists and maps of up to three items.
fn random_value(ty: &RefTy, rng: &mut Lcg) -> RefValue {
    match ty {
        RefTy::Bool => RefValue::Bool(rng.below(2) == 1),
        RefTy::U8 => RefValue::U8(rng.below(256) as u8),
        RefTy::Int => RefValue::Int(rng.next() as i32),
        RefTy::I64 => RefValue::I64((rng.next() << 32 | rng.next()) as i64),
        RefTy::Float => RefValue::Float(f32::from_bits(rng.next() as u32)),
        RefTy::F64 => RefValue::F64(f64::from_bits(rng.next() << 32 | rng.next())),
        RefTy::Str => RefValue::Str(["", "a", "h\u{e9}", "\u{1F600}x"][rng.below(4)].to_string()),
        RefTy::List(item) => RefValue::List((0..rng.below(4)).map(|_| random_value(item, rng)).collect()),
        RefTy::Map(k, v) => {
            let mut entries: Vec<(RefValue, RefValue)> = Vec::new();
            for _ in 0..rng.below(4) {
                let key = random_value(k, rng);
                if !entries.iter().any(|(e, _)| *e == key) {
                    entries.push((key, random_value(v, rng)));
                }
            }
            RefValue::Map(entries)
        }
        RefTy::Opt(inner) => RefValue::Opt((rng.below(2) == 1).then(|| Box::new(random_value(inner, rng)))),
        RefTy::Enum(variants) => {
            let v = rng.below(variants.len());
            RefValue::Enum(v as u32, variants[v].iter().map(|f| random_value(f, rng)).collect())
        }
        RefTy::Struct(fields) => RefValue::Struct(fields.iter().map(|f| random_value(f, rng)).collect()),
    }
}

#[test]
fn damaged_bytes_never_panic_and_never_read_as_another_value() {
    let shape = RefTy::Enum(vec![vec![], vec![RefTy::Float, RefTy::Str], vec![list(RefTy::U8)]]);
    let item = RefTy::Struct(vec![RefTy::Str, RefTy::Int, shape.clone(), opt(RefTy::I64)]);
    // Structs that end in an empty struct, which takes no bytes.
    let tagged = RefTy::Struct(vec![RefTy::Int, RefTy::Struct(vec![])]);
    let types = [
        RefTy::Struct(vec![RefTy::I64, RefTy::F64, RefTy::Bool, list(item.clone()), RefTy::Map(Box::new(RefTy::Str), Box::new(list(RefTy::U8))), opt(RefTy::F64)]),
        list(item),
        RefTy::Map(Box::new(RefTy::Int), Box::new(opt(shape.clone()))),
        list(list(RefTy::Str)),
        shape,
        list(tagged),
        RefTy::Map(Box::new(RefTy::Str), Box::new(RefTy::Struct(vec![]))),
    ];
    let mut rng = Lcg(0x5EED);
    let (mut read, mut refused) = (0, 0);
    for i in 0..70 {
        let ty = &types[i % types.len()];
        let value = random_value(ty, &mut rng);
        let bytes = encode(&value, 77);
        assert_eq!(decode(&bytes, ty, 77).map(|v| encode(&v, 77)), Some(bytes.clone()));
        for n in 0..bytes.len() {
            assert_eq!(decode(&bytes[..n], ty, 77), None, "{value:?} cut to {n} bytes");
        }
        for _ in 0..200 {
            let mut bad = bytes.clone();
            let at = rng.below(bad.len());
            bad[at] = rng.below(256) as u8;
            // The format has one way to write each value, so whatever reads back writes the same
            // bytes.
            match decode(&bad, ty, 77) {
                Some(v) => {
                    assert_eq!(encode(&v, 77), bad, "{value:?} changed at byte {at}");
                    read += 1;
                }
                None => refused += 1,
            }
        }
    }
    assert!(read > 0 && refused > 0, "{read} read, {refused} refused");
}

#[test]
fn a_string_is_read_when_its_bytes_are_utf8() {
    // On a thread of its own: the strings go in that thread's program arena.
    std::thread::spawn(|| {
        let mut out = crate::string::KStr { ptr: std::ptr::null(), len: 7 };
        let text = "h\u{e9}";
        // SAFETY: `text` has these bytes, and `out` is a string's room.
        let ok = unsafe { kestrel_rt_bytes_read_str(text.as_ptr(), text.len() as i64, &mut out) };
        // SAFETY: the string was just made.
        assert_eq!((ok, unsafe { crate::string::KStr::bytes(out.ptr, out.len) }), (1, text.as_bytes()));
        assert_ne!(out.ptr, text.as_ptr());
        let bad = [b'a', 0xFF];
        // SAFETY: as above.
        let ok = unsafe { kestrel_rt_bytes_read_str(bad.as_ptr(), 2, &mut out) };
        assert_eq!((ok, out.ptr, out.len), (0, std::ptr::null(), 0));
        // SAFETY: no bytes are read.
        let ok = unsafe { kestrel_rt_bytes_read_str(std::ptr::null(), 0, &mut out) };
        assert_eq!((ok, out.len), (1, 0));
    })
    .join()
    .unwrap();
}

#[test]
fn each_reason_is_its_sentence() {
    let says = |code: i64, at: i64, len: i64| reason_text(code, at, len, "Save");
    assert_eq!(says(reason::TOO_SHORT, 0, 3), "the bytes are too short to hold a saved value: 3 bytes, and the header alone is 13");
    assert_eq!(says(reason::TOO_SHORT, 0, 1), "the bytes are too short to hold a saved value: 1 byte, and the header alone is 13");
    assert_eq!(says(reason::MAGIC, 0, 20), "the bytes aren't a saved value: they don't start with KESB");
    assert_eq!(says(reason::VERSION, 2, 20), "the bytes were saved in format 2, and this Kestrel reads format 1");
    assert_eq!(says(reason::SHAPE, 0, 20), "the bytes hold another type, or another shape of `Save`: a field was added, removed, renamed or changed");
    assert_eq!(says(reason::END_EARLY, 57, 57), "the bytes end early, at byte 57 of 57");
    assert_eq!(says(reason::NOT_BOOL, 40, 57), "byte 40 should be 0 or 1");
    assert_eq!(says(reason::VARIANT, 40, 57), "the variant number at byte 40 is past the end of its enum");
    assert_eq!(says(reason::NOT_UTF8, 40, 57), "the string at byte 40 isn't UTF-8 text");
    assert_eq!(says(reason::LENGTH, 40, 57), "the length at byte 40 is more than the bytes left can hold");
    assert_eq!(says(reason::DUP_KEY, 40, 57), "the map at byte 40 has a key twice");
    assert_eq!(says(reason::TRAILING, 44, 45), "the value ends at byte 44, but there are 45 bytes");
    // Through the runtime call, as a string in the current arena (a thread of its own, as above).
    std::thread::spawn(|| {
        let mut out = crate::string::KStr { ptr: std::ptr::null(), len: 0 };
        let name = "Game";
        // SAFETY: `name` has these bytes, and `out` is a string's room.
        unsafe { kestrel_rt_bytes_reason(reason::SHAPE, 0, 20, name.as_ptr(), name.len() as i64, &mut out) };
        // SAFETY: the string was just made.
        let text = unsafe { crate::string::KStr::bytes(out.ptr, out.len) };
        assert_eq!(std::str::from_utf8(text).unwrap(), reason_text(reason::SHAPE, 0, 20, "Game"));
    })
    .join()
    .unwrap();
}
