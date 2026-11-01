use kestrel_kir::*;
use kestrel_syntax::{parse, LineIndex};

fn optimized(src: &str) -> Module {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = kestrel_check::check(&p);
    assert!(d.is_empty(), "check: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let mut m = lower(&p, &t.unwrap(), &LineIndex::new(src));
    optimize(&mut m);
    m
}

fn func<'m>(m: &'m Module, name: &str) -> &'m Func {
    m.funcs.iter().find(|f| f.name == name).unwrap()
}

/// The names of `f`'s struct locals that still live in memory.
fn struct_slots(f: &Func) -> Vec<&str> {
    f.locals.iter().filter(|l| matches!(l.ty, KTy::Struct(_)) && l.kind == LocalKind::Addressed).map(|l| l.name.as_str()).collect()
}

#[test]
fn vector_temporaries_become_one_local_per_field() {
    let src = "pure f(a: vec2, b: vec2) -> float {\n let d = (a - b).normalized() * 2\n d.x + d.y\n}\nmain() {}\n";
    let m = optimized(src);
    let f = func(&m, "f");
    assert!(struct_slots(f).is_empty(), "{:#?}", f.locals);
    let d = f.locals.iter().find(|l| l.name == "d").unwrap();
    assert_eq!(d.kind, LocalKind::Unused);
    assert!(f.locals.iter().any(|l| l.name == "d.x" && l.ty == KTy::F32 && l.kind == LocalKind::Value), "{:#?}", f.locals);
}

#[test]
fn a_struct_passed_as_var_stays_in_memory() {
    // `push` returns early, so it isn't inlined and `p` is still passed by pointer.
    let src = "struct P {\n x: float\n y: float\n}\npush(var p: P) {\n if p.x > 10 { return }\n p.x += 1\n}\nmain() {\n var p = P(x: 1, y: 2)\n push(var p)\n print(p)\n}\n";
    assert_eq!(struct_slots(func(&optimized(src), "main")), vec!["p"]);
}

#[test]
fn a_number_passed_as_var_to_an_inlined_call_goes_back_to_a_register() {
    let src = "bump(var n: int) {\n n += 1\n}\nmain() {\n var n = 1\n bump(var n)\n print(n)\n}\n";
    let m = optimized(src);
    let n = func(&m, "main").locals.iter().find(|l| l.name == "n").unwrap();
    assert_eq!(n.kind, LocalKind::Value);
}

#[test]
fn struct_stores_to_memory_read_every_field_before_storing() {
    let src = "struct B {\n pos: vec2\n vel: vec2\n}\nmain() {\n var bs = [B(pos: vec2(1, 2), vel: vec2(3, 4))]\n bs[0].vel = bs[0].vel.yx + bs[0].pos\n bs[0].pos = bs[0].vel\n print(bs)\n}\n";
    let m = optimized(src);
    let main = func(&m, "main");
    let (mut copies, mut built) = (0, 0);
    visit::walk(&main.body, &mut |n| {
        if let visit::Node::Stmt(Stmt::Assign(p, e)) = n {
            if let KTy::Struct(_) = e.ty {
                match &e.kind {
                    // A copy from one place in memory to another.
                    ExprKind::Load(_) => copies += 1,
                    // A value built from locals and constants only, so it can't read the place half-written.
                    ExprKind::StructLit(..) => {
                        assert!(only_locals(main, e), "{p:?} = {e:#?}");
                        built += 1;
                    }
                    _ => panic!("{p:?} = {e:#?}"),
                }
            }
        }
    });
    assert_eq!((copies, built), (1, 1));
}

/// Whether `e` is made only of constants and loads of `Value` locals.
fn only_locals(f: &Func, e: &Expr) -> bool {
    match &e.kind {
        ExprKind::StructLit(_, fields) => fields.iter().all(|x| only_locals(f, x)),
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => true,
        ExprKind::Load(p) => p.path.is_empty() && f.locals[p.root.0 as usize].kind == LocalKind::Value,
        _ => false,
    }
}

#[test]
fn a_struct_store_reads_every_field_first_in_a_function_with_no_struct_locals() {
    // `set` returns early, so it isn't inlined, and it holds no struct local to split.
    let src = "set(var xs: [vec2]) {\n if xs.len == 0 { return }\n xs[0] = vec2(xs[0].y, xs[0].x)\n}\nmain() {\n var xs = [vec2(0, 0)]\n set(var xs)\n print(xs)\n}\n";
    let m = optimized(src);
    let f = func(&m, "set");
    let mut stores = 0;
    visit::walk(&f.body, &mut |n| {
        if let visit::Node::Stmt(Stmt::Assign(p, e)) = n {
            // One store, of a value built from locals: the fields were read into locals first.
            assert!(matches!(e.kind, ExprKind::StructLit(..)) && only_locals(f, e), "{p:?} = {e:#?}");
            stores += 1;
        }
    });
    assert_eq!(stores, 1);
}

#[test]
fn a_call_result_stored_into_a_list_is_built_from_locals() {
    // `make` returns early, so it isn't inlined: its result comes back in memory, and the store
    // must still build its value from locals, not from that memory.
    let src = "make(k: float) -> vec2 {\n if k > 100 { return vec2(0, 0) }\n vec2(k, k + 1)\n}\nstore(var xs: list[vec2], i: int, k: float) {\n xs[i] = make(k)\n}\nmain() {}\n";
    let m = optimized(src);
    let f = func(&m, "store");
    let mut stores = 0;
    visit::walk(&f.body, &mut |n| {
        if let visit::Node::Stmt(Stmt::Assign(p, e)) = n {
            if let KTy::Struct(_) = e.ty {
                assert!(matches!(e.kind, ExprKind::StructLit(..)) && only_locals(f, e), "{p:?} = {e:#?}");
                stores += 1;
            }
        }
    });
    assert_eq!(stores, 1);
}

#[test]
fn a_struct_with_exactly_16_fields_at_the_bottom_is_split() {
    let src = "struct W {\n a: vec3\n b: vec3\n c: vec3\n d: vec3\n e: vec3\n f: float\n}\nmain() {\n var w = W(a: vec3(1, 1, 1), b: vec3(1, 1, 1), c: vec3(1, 1, 1), d: vec3(1, 1, 1), e: vec3(1, 1, 1), f: 1)\n w.f = w.f + 1\n print(w)\n}\n";
    let m = optimized(src);
    let f = func(&m, "main");
    assert!(struct_slots(f).is_empty(), "{:#?}", f.locals);
    let w = f.locals.iter().find(|l| l.name == "w").unwrap();
    assert_eq!(w.kind, LocalKind::Unused);
    assert!(f.locals.iter().any(|l| l.name == "w.e.z" && l.kind == LocalKind::Value), "{:#?}", f.locals);
}

#[test]
fn a_struct_with_more_than_16_fields_at_the_bottom_stays_in_memory() {
    let src = "struct W {\n a: vec3\n b: vec3\n c: vec3\n d: vec3\n e: vec3\n f: vec2\n}\nmain() {\n var w = W(a: vec3(1, 1, 1), b: vec3(1, 1, 1), c: vec3(1, 1, 1), d: vec3(1, 1, 1), e: vec3(1, 1, 1), f: vec2(1, 1))\n w.f = w.f.yx\n print(w)\n}\n";
    assert_eq!(struct_slots(func(&optimized(src), "main")), vec!["w"]);
}

/// The names of `f`'s enum locals that still live in memory.
fn enum_slots(f: &Func) -> Vec<&str> {
    f.locals.iter().filter(|l| matches!(l.ty, KTy::Enum(_)) && l.kind == LocalKind::Addressed).map(|l| l.name.as_str()).collect()
}

fn heightmap() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/heightmap/heightmap.kes")).unwrap()
}

#[test]
fn the_heightmap_checksum_keeps_its_block_in_registers() {
    let m = optimized(&heightmap());
    for name in ["Chunk.checksum", "main"] {
        let f = func(&m, name);
        assert!(enum_slots(f).is_empty(), "{name}: {:#?}", f.locals);
        assert!(f.locals.iter().any(|l| l.name.ends_with(".tag") && l.ty == KTy::U8 && l.kind == LocalKind::Value), "{name}: {:#?}", f.locals);
    }
}

const BLOCK: &str = "enum Block {\n Air\n Dirt\n Stone { hardness: u8 }\n Ore { at: vec2, rich: bool }\n}\nmake(k: int) -> Block {\n if k > 2 {\n  return Block.Ore(at: vec2(1, 2), rich: true)\n }\n Block.Dirt\n}\nkeep(var b: Block, k: int) {\n if k > 5 {\n  return\n }\n b = Block.Air\n}\n";

/// The enum locals `main` keeps in memory, for `main`'s body `body` (which has `var b` and `k`).
fn kept(body: &str) -> Vec<String> {
    let src = format!("{BLOCK}main() {{\n var b = Block.Stone(hardness: 3)\n let k = 2\n{body}}}\n");
    let m = optimized(&src);
    enum_slots(func(&m, "main")).into_iter().map(str::to_string).collect()
}

#[test]
fn an_enum_read_by_tag_and_fields_and_given_variants_is_split() {
    assert!(kept(" if k > 1 {\n  b = Block.Air\n }\n match b {\n  Stone s { print(s.hardness) }\n  else { print(0) }\n }\n").is_empty());
}

#[test]
fn an_enum_given_a_value_if_or_another_split_enum_is_split() {
    let body = " var c = if k > 1 { Block.Dirt } else { Block.Ore(at: vec2(3, 4), rich: false) }\n c = b\n match c {\n  Ore o { print(o.at.x) }\n  else { print(1) }\n }\n";
    assert!(kept(body).is_empty());
}

#[test]
fn an_enum_hashed_or_turned_to_bytes_reads_only_its_variant_so_it_is_split() {
    // `checksum` and `to_bytes` read the tag, then the fields of the variant the tag names: no
    // byte of another variant.
    assert!(kept(" print(checksum(b))\n").is_empty());
    assert!(kept(" print(to_bytes(b).len)\n").is_empty());
}

#[test]
fn an_enum_printed_stored_in_a_list_passed_var_or_given_a_call_is_not_split() {
    assert_eq!(kept(" print(b)\n"), vec!["b"]);
    assert_eq!(kept(" var bs = list[Block]()\n bs.add(b)\n print(bs.len)\n"), vec!["b"]);
    assert_eq!(kept(" keep(var b, k)\n match b {\n  Air { print(0) }\n  else { print(1) }\n }\n"), vec!["b"]);
    assert_eq!(kept(" b = make(k)\n match b {\n  Air { print(0) }\n  else { print(1) }\n }\n"), vec!["b"]);
}

#[test]
fn a_copy_into_an_enum_kept_in_memory_keeps_both() {
    assert_eq!(kept(" var c = Block.Dirt\n c = b\n print(c)\n"), vec!["b", "c"]);
}
