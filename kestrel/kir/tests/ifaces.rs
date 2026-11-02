//! Interface values in KIR: method tables, `MakeIface` (borrowed or boxed) and `CallVirtual`.

use kestrel_kir::*;
use kestrel_syntax::{parse, LineIndex};

fn lowered(src: &str) -> Module {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = kestrel_check::check(&p);
    assert!(d.is_empty(), "check: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    lower(&p, &t.unwrap(), &LineIndex::new(src))
}

fn optimized(src: &str) -> Module {
    let mut m = lowered(src);
    optimize(&mut m);
    m
}

fn func<'m>(m: &'m Module, name: &str) -> &'m Func {
    m.funcs.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}"))
}

fn idx(m: &Module, name: &str) -> FuncIdx {
    FuncIdx(m.funcs.iter().position(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}")) as u32)
}

/// Every call argument in `stmts` (of direct, indirect and virtual calls), in order.
fn call_args(stmts: &[Stmt]) -> Vec<Arg> {
    let mut out = Vec::new();
    visit::walk(stmts, &mut |n| match n {
        visit::Node::Expr(Expr { kind: ExprKind::Call(_, a) | ExprKind::CallIndirect { args: a, .. } | ExprKind::CallVirtual { args: a, .. }, .. })
        | visit::Node::Stmt(Stmt::Call(_, a, _) | Stmt::CallIndirect { args: a, .. } | Stmt::CallVirtual { args: a, .. }) => out.extend(a.iter().cloned()),
        _ => {}
    });
    out
}

fn is_borrowed(a: &Arg) -> bool {
    matches!(a, Arg::Value(Expr { kind: ExprKind::MakeIface { data: IfaceData::Borrowed(_), .. }, .. }))
}

const SRC: &str = "interface Damageable {
    hit(var self, n: int)
    pure hp(self) -> int
    pure at(self) -> vec2
}

interface Named {
    pure hp(self) -> int
}

struct Goblin: Damageable, Named {
    health: int = 10
    hit(var self, n: int) { self.health -= n }
    pure hp(self) -> int { self.health }
    pure at(self) -> vec2 { vec2(1, 2) }
    bump(var self) {
        self.health += 1
        heal(var self)
        self.health += 100
    }
}

enum Barrel: Damageable {
    Full { oil: int }
    Empty
    hit(var self, n: int) { }
    pure hp(self) -> int { 0 }
    pure at(self) -> vec2 { vec2(0, 0) }
}

heal(var d: Damageable) { d.hit(-5) }
show(d: Damageable) -> int { d.hp() }
name(d: Named) -> int { d.hp() }
pair(a: Damageable, var b: Damageable, n: int) { b.hit(a.hp() + n) }
keep(d: Damageable) -> Damageable { d }
noisy(n: int) -> int { n }

main() {
    var g = Goblin()
    var gs = [Goblin(), Goblin()]
    heal(var g)
    print(show(g), name(g), show(Barrel.Empty))
    pair(n: noisy(3), b: var gs[noisy(0)], a: g)
    var foes: list[Damageable] = [Goblin(), Barrel.Full(oil: 2)]
    foes[0].hit(1)
    print(foes[1].at())
}
";

#[test]
fn each_type_and_interface_pair_gets_one_table_of_its_own_methods() {
    let m = lowered(SRC);
    // Goblin as Damageable, Goblin as Named, Barrel as Damageable: one table each, however
    // often each pair is made.
    assert_eq!(m.tables.len(), 3, "{:#?}", m.tables);
    let goblin: Vec<&MethodTable> = m.tables.iter().filter(|t| t.methods.contains(&idx(&m, "Goblin.hp"))).collect();
    assert_eq!(goblin.len(), 2);
    assert_eq!(goblin[0].type_id, goblin[1].type_id, "one id per concrete type");
    let barrel = m.tables.iter().find(|t| t.methods.contains(&idx(&m, "Barrel.hp"))).unwrap();
    assert_ne!(barrel.type_id, goblin[0].type_id);
    // The methods are the type's own, in the interface's order.
    let damageable = goblin.iter().find(|t| t.methods.len() == 3).unwrap();
    assert_eq!(damageable.methods, vec![idx(&m, "Goblin.hit"), idx(&m, "Goblin.hp"), idx(&m, "Goblin.at")]);
    assert_eq!(goblin.iter().find(|t| t.methods.len() == 1).unwrap().methods, vec![idx(&m, "Goblin.hp")]);
    // The print entry writes the concrete value; the copy entry copies it (see below).
    let print = &m.funcs[damageable.print.0 as usize];
    assert!(matches!(print.body[..], [Stmt::Write(_)]), "{print:#?}");
    assert_eq!(print.locals[0].kind, LocalKind::RefParam);
    let copy = &m.funcs[damageable.copy.0 as usize];
    assert_eq!(copy.ret, Some(KTy::I64));
    assert_ne!(damageable.copy, barrel.copy, "one copy entry per table");
}

#[test]
fn a_copy_entry_copies_the_concrete_bytes_and_then_what_they_hold() {
    let m = lowered(
        "interface Named {
    pure size(self) -> int
}
struct Pack: Named {
    n: int = 1
    tags: list[int] = []
    pure size(self) -> int { self.tags.len }
}
main() {
    let d: Named = Pack()
    print(d.size())
}
",
    );
    let copy = &m.funcs[m.tables[0].copy.0 as usize];
    let [Stmt::Let(c, Expr { kind: ExprKind::BlockDup { size, align, .. }, .. }), Stmt::BindEnv { local, offset: 0, .. }, fix, Stmt::Return(Some(_))] = &copy.body[..] else {
        panic!("{copy:#?}")
    };
    // The concrete type's size, as codegen lays it out.
    let value = &copy.locals[local.0 as usize];
    assert_eq!(value.kind, LocalKind::RefParam);
    assert!(matches!(value.ty, KTy::Struct(_)));
    assert_eq!((*size, *align), m.size_align(value.ty));
    assert_eq!(copy.locals[c.0 as usize].ty, KTy::I64);
    // The list gets its own items, through the list's copy function.
    assert!(matches!(fix, Stmt::Call(_, a, _) if matches!(&a[..], [Arg::Ref(p)] if p.root == *local)), "{fix:#?}");
}

#[test]
fn an_owning_types_copy_entry_fails_with_both_names() {
    let m = lowered(
        "interface Named {
    pure size(self) -> int
}
struct Pool: Named {
    mem: Arena = Arena()
    pure size(self) -> int { 0 }
}
show(d: Named) -> int { d.size() }
main() {
    var p = Pool()
    print(show(p))
}
",
    );
    let copy = &m.funcs[m.tables[0].copy.0 as usize];
    let msg = "can't copy a `Pool` into a `Named`: it owns an `Arena`; pass it as `var` or keep it in a `list[Pool]`";
    let message = Expr { kind: ExprKind::StrLit(msg.to_string()), ty: KTy::Str, line: 0 };
    assert_eq!(copy.body, vec![Stmt::Fail { message, line: 0 }]);
}

#[test]
fn a_receiver_that_isnt_a_place_is_used_as_it_is_without_a_copy() {
    let m = lowered(
        "interface Named {
    pure plus(self, n: int) -> int
}
struct Pack: Named {
    n: int = 1
    pure plus(self, k: int) -> int { self.n + k }
}
main() {
    let a: Named = Pack()
    let b: Named = Pack(n: 2)
    var n = 0
    let c = n == 0
    print((if c { a } else { b }).plus(n + 1), a.plus(n))
}
",
    );
    // The checker refuses a later argument that changes `a` or `b` (see `check_changed_later`), so
    // neither the `if` nor the place `a` is copied.
    let mut copies = 0;
    visit::walk(&func(&m, "main").body, &mut |n| {
        if let visit::Node::Stmt(Stmt::CopyIface(_)) = n {
            copies += 1;
        }
    });
    assert_eq!(copies, 0, "{:#?}", func(&m, "main").body);
}

#[test]
fn a_call_through_an_interface_names_its_slot_and_leaves_self_out_of_the_signature() {
    let m = lowered(SRC);
    let mut calls = Vec::new();
    visit::walk(&func(&m, "main").body, &mut |n| match n {
        visit::Node::Stmt(Stmt::CallVirtual { slot, sig, .. }) | visit::Node::Expr(Expr { kind: ExprKind::CallVirtual { slot, sig, .. }, .. }) => {
            calls.push((*slot, sig.clone()))
        }
        _ => {}
    });
    let hit = KSig { params: vec![(KTy::I32, false)], ret: None, pure: false };
    let at = KSig { params: Vec::new(), ret: Some(KTy::Struct(StructIdx(0))), pure: true };
    assert_eq!(calls, vec![(0, hit), (2, at)]);
    let hp: Vec<u32> = {
        let mut out = Vec::new();
        visit::walk(&func(&m, "show").body, &mut |n| {
            if let visit::Node::Expr(Expr { kind: ExprKind::CallVirtual { slot, .. }, .. }) = n {
                out.push(*slot);
            }
        });
        out
    };
    assert_eq!(hp, vec![1]);
}

#[test]
fn places_are_lent_and_var_ones_as_values_and_everything_else_is_boxed() {
    let m = lowered(SRC);
    let args = call_args(&func(&m, "main").body);
    // `heal(var g)`, `show(g)`, `name(g)`, and `pair`'s `var gs[...]` and `g`.
    assert_eq!(args.iter().filter(|a| is_borrowed(a)).count(), 5, "{args:#?}");
    // A `var` interface argument is never a bare reference to the concrete place.
    assert!(!args.iter().any(|a| matches!(a, Arg::Ref(p) if p.root == LocalIdx(0))), "{args:#?}");
    let mut boxed = 0;
    visit::walk(&func(&m, "main").body, &mut |n| {
        if let visit::Node::Expr(Expr { kind: ExprKind::MakeIface { data: IfaceData::Boxed(v), .. }, .. }) = n {
            assert!(matches!(v.ty, KTy::Struct(_) | KTy::Enum(_)));
            boxed += 1;
        }
    });
    // `show(Barrel.Empty)` and the list literal's two items.
    assert_eq!(boxed, 3);
    assert_eq!(check_stack_envs(&m), Ok(()));
}

#[test]
fn a_reordered_call_keeps_the_borrowed_value_its_own_argument() {
    let m = lowered(SRC);
    let pair = idx(&m, "pair");
    let mut found = None;
    visit::walk(&func(&m, "main").body, &mut |n| {
        if let visit::Node::Stmt(Stmt::Call(f, a, _)) = n {
            if *f == pair {
                found = Some(a.clone());
            }
        }
    });
    let args = found.expect("a call to `pair`");
    assert!(is_borrowed(&args[0]) && is_borrowed(&args[1]), "{args:#?}");
    // `gs[noisy(0)]`'s index was evaluated in its turn, into a temporary.
    let Arg::Value(Expr { kind: ExprKind::MakeIface { data: IfaceData::Borrowed(p), .. }, .. }) = &args[1] else { unreachable!() };
    assert!(matches!(&p.path[..], [Proj::Index { at: Expr { kind: ExprKind::Load(_), .. }, .. }]), "{p:#?}");
}

#[test]
fn a_borrowed_value_outside_a_call_is_refused() {
    let mut m = lowered(SRC);
    let main = m.funcs.iter_mut().find(|f| f.name == "main").unwrap();
    let mut stray = None;
    visit::walk(&main.body, &mut |n| {
        if let visit::Node::Expr(e @ Expr { kind: ExprKind::MakeIface { data: IfaceData::Borrowed(_), .. }, .. }) = n {
            stray.get_or_insert_with(|| e.clone());
        }
    });
    let tmp = LocalIdx(main.locals.len() as u32);
    main.locals.push(Local { name: "tmp".into(), ty: KTy::Iface, kind: LocalKind::Addressed });
    main.body.push(Stmt::Let(tmp, stray.expect("a borrowed value")));
    assert_eq!(check_stack_envs(&m), Err("`main` has a borrowed interface value that isn't a call's argument".to_string()));
}

#[test]
fn copying_an_interface_value_goes_through_its_table() {
    let m = lowered(SRC);
    // `keep` returns its parameter, which belongs to the caller: the result is a copy.
    let mut copies = 0;
    visit::walk(&func(&m, "keep").body, &mut |n| {
        if let visit::Node::Stmt(Stmt::CopyIface(_)) = n {
            copies += 1;
        }
    });
    assert_eq!(copies, 1);
}

#[test]
fn a_lent_place_stays_in_memory_and_is_not_kept_in_registers() {
    let m = optimized(SRC);
    // `g` is a plain struct, but `show(g)` takes its address: `scalarize` leaves it whole.
    let main = func(&m, "main");
    assert_eq!(main.locals[0].kind, LocalKind::Addressed, "{:#?}", main.locals);
    // `bump` lends `self` to `heal`: `promote` keeps no field of `self` in a local, so the
    // change `heal` makes through the interface value is seen.
    let bump = func(&m, "Goblin.bump");
    assert!(!bump.locals.iter().any(|l| l.name.starts_with("self.")), "{:#?}", bump.locals);
}
