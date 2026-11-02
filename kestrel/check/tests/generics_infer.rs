//! Working out a generic's types from the values and the expected type (pick 2, G7): calls of
//! generic functions, constructions of generic structs and variants of generic enums with no
//! types written. Each test reads the `type_args` recorded for the call.

use kestrel_check::{check_keeping_tables, Tables, Ty};
use kestrel_syntax::ast::{Block, Expr, ExprId, ExprKind, Item, Program};
use kestrel_syntax::parse;
use kestrel_syntax::visit::each_expr;

fn run(src: &str) -> (Program, Tables, Vec<String>) {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check_keeping_tables(&p, true);
    let errs = d.into_iter().filter(|d| d.is_error()).map(|d| d.message).collect();
    (p, t, errs)
}

fn clean(src: &str) -> (Program, Tables) {
    let (p, t, errs) = run(src);
    assert!(errs.is_empty(), "{errs:?}");
    (p, t)
}

fn errors(src: &str) -> Vec<String> {
    run(src).2
}

fn bodies(p: &Program) -> Vec<&Block> {
    let mut out = Vec::new();
    for item in &p.items {
        match item {
            Item::Fn(f) => out.push(&f.body),
            Item::Struct(s) => out.extend(s.methods.iter().map(|m| &m.body)),
            Item::Enum(e) => out.extend(e.methods.iter().map(|m| &m.body)),
            _ => {}
        }
    }
    out
}

/// The ids of every expression in a body that `pred` accepts, in source order.
fn find_where(p: &Program, pred: impl Fn(&Expr) -> bool) -> Vec<ExprId> {
    let mut out = Vec::new();
    for b in bodies(p) {
        each_expr(b, &mut |e: &Expr| {
            if pred(e) {
                out.push(e.id);
            }
        });
    }
    out
}

fn the(p: &Program, pred: impl Fn(&Expr) -> bool) -> ExprId {
    let all = find_where(p, pred);
    assert_eq!(all.len(), 1, "expressions: {all:?}");
    all[0]
}

fn call(name: &'static str) -> impl Fn(&Expr) -> bool {
    move |e| matches!(&e.kind, ExprKind::Call { name: n, .. } if n == name)
}

fn method_call(name: &'static str) -> impl Fn(&Expr) -> bool {
    move |e| matches!(&e.kind, ExprKind::MethodCall { name: n, .. } if n == name)
}

fn field(name: &'static str) -> impl Fn(&Expr) -> bool {
    move |e| matches!(&e.kind, ExprKind::Field { name: n, .. } if n == name)
}

fn type_args(t: &Tables, e: ExprId) -> Vec<String> {
    let list = t.generic.type_args.get(&e).unwrap_or_else(|| panic!("no type_args for {e:?}"));
    t.ty_list(*list).iter().map(|&ty| t.ty_name(ty)).collect()
}

const DECLS: &str = "struct Stack[T] {\n    items: list[T] = []\n}\nstruct Pair[A, B] {\n    a: A\n    b: B\n}\nenum Maybe[T] {\n    Nothing\n    Just { value: T }\n}\nenum Outcome[T, E] {\n    Ok { value: T }\n    Err { error: E }\n}\npure pick[T](a: T, b: T) -> T { b }\npure make[T](f: pure () -> T) -> T { f() }\npure apply[T, R](x: T, f: pure (T) -> R) -> R { f(x) }\npure first_or[T](xs: list[T], d: T?) -> T? { d }\npure count[T](xs: list[T]) -> int { xs.len }\npure both[T](a: list[T], b: list[T]) -> int { a.len + b.len }\npure push_all[T](var xs: list[T], v: T) { xs.add(v) }\nload() -> Outcome[int, string] {\n    return Outcome.Ok(value: 3)\n}\n";

#[test]
fn values_decide() {
    let (p, t) = clean(&format!("{DECLS}main() {{\n    let a = pick(1, 2)\n    let b = pick(\"a\", \"b\")\n    let c = Pair(b: true, a: 2)\n}}\n"));
    let picks = find_where(&p, call("pick"));
    assert_eq!(type_args(&t, picks[0]), ["int"]);
    assert_eq!(type_args(&t, picks[1]), ["string"]);
    let pair = the(&p, call("Pair"));
    assert_eq!(type_args(&t, pair), ["int", "bool"]);
    assert_eq!(t.ty_name(t.ty(pair)), "Pair[int, bool]");
}

#[test]
fn numbers_widen_loosely() {
    let (p, t) = clean(&format!("{DECLS}main() {{\n    let a = pick(1, 2.5)\n}}\n"));
    let pick = the(&p, call("pick"));
    assert_eq!(type_args(&t, pick), ["float"]);
    assert_eq!(t.ty(pick), Ty::Float);
    let one = the(&p, |e| matches!(e.kind, ExprKind::Int(1)));
    assert_eq!(t.coerce.get(&one), Some(&Ty::Float));
}

#[test]
fn the_expected_type_fills_only() {
    let (p, t) = clean(&format!(
        "{DECLS}main() {{\n    let s: Stack[int] = Stack()\n    let x: float = pick(1, 2)\n    let m: Maybe[int] = Maybe.Nothing\n    let n: Maybe[string]? = Maybe.Nothing\n}}\n"
    ));
    let stack = the(&p, call("Stack"));
    assert_eq!(type_args(&t, stack), ["int"]);
    assert_eq!(t.ty_name(t.ty(stack)), "Stack[int]");
    // The values decided `int`; the result widens to the `let`'s `float` as any `int` would.
    let pick = the(&p, call("pick"));
    assert_eq!(type_args(&t, pick), ["int"]);
    assert_eq!(t.ty(pick), Ty::Int);
    assert_eq!(t.coerce.get(&pick), Some(&Ty::Float));
    let ok = the(&p, method_call("Ok"));
    assert_eq!(type_args(&t, ok), ["int", "string"]);
    let nothing = find_where(&p, field("Nothing"));
    assert_eq!(type_args(&t, nothing[0]), ["int"]);
    assert_eq!(t.ty_name(t.ty(nothing[0])), "Maybe[int]");
    // An optional expected type gives the type inside it.
    assert_eq!(type_args(&t, nothing[1]), ["string"]);
}

#[test]
fn the_expected_type_never_changes_a_decided_placeholder() {
    let errs = errors(&format!("{DECLS}main() {{\n    let f: Stack[float] = Stack(items: [1, 2])\n    let r: Outcome[float, string] = Outcome.Ok(value: 3)\n}}\n"));
    assert_eq!(errs, ["expected Stack[float], found Stack[int]", "expected Outcome[float, string], found Outcome[int, string]"]);
}

#[test]
fn deferred_arguments() {
    let (p, t) = clean(&format!(
        "{DECLS}main() {{\n    let a = make(() {{ 3 }})\n    let b = apply(4, (x) {{ float(x) }})\n    let xs: list[int] = [1]\n    let c = first_or(xs, none)\n    let d = both(xs, [])\n}}\n"
    ));
    assert_eq!(type_args(&t, the(&p, call("make"))), ["int"]);
    let apply = the(&p, call("apply"));
    assert_eq!(type_args(&t, apply), ["int", "float"]);
    let closure = find_where(&p, |e| matches!(e.kind, ExprKind::Closure(_)))[1];
    assert_eq!(t.ty_name(t.ty(closure)), "pure (int) -> float");
    let firsts = find_where(&p, call("first_or"));
    assert_eq!(type_args(&t, firsts[0]), ["int"]);
    let both = the(&p, call("both"));
    assert_eq!(type_args(&t, both), ["int"]);
    let empty = the(&p, |e| matches!(&e.kind, ExprKind::ArrayLit(items) if items.is_empty()));
    assert_eq!(t.ty_name(t.ty(empty)), "list[int]");
    // A `none` written before the value that decides is checked once the placeholder is known.
    assert!(errors(&format!("{DECLS}main() {{\n    let xs: list[int] = [1]\n    let e = first_or(d: none, xs: xs)\n}}\n")).is_empty());
}

#[test]
fn a_literal_decides_through_its_parameter() {
    let (p, t) = clean(&format!("{DECLS}main() {{\n    let n = count([1.5, 2.5])\n    let s = Stack(items: [1, 2])\n}}\n"));
    assert_eq!(type_args(&t, the(&p, call("count"))), ["float"]);
    let lits = find_where(&p, |e| matches!(&e.kind, ExprKind::ArrayLit(items) if !items.is_empty()));
    assert_eq!(t.ty_name(t.ty(lits[0])), "list[float]");
    assert_eq!(t.ty_name(t.ty(the(&p, call("Stack")))), "Stack[int]");
    assert_eq!(t.ty_name(t.ty(lits[1])), "list[int]");
}

#[test]
fn exactness() {
    // `3` converts to the exact `float` the `var` argument decided.
    let (p, t) = clean(&format!("{DECLS}main() {{\n    var xs: list[float] = [1.0]\n    push_all(var xs, 3)\n}}\n"));
    assert_eq!(type_args(&t, the(&p, call("push_all"))), ["float"]);
    // A function value's parameter type decides `T` exactly: a later `float` can't widen it.
    let fns = "pure tof(x: int) -> float { float(x) }\npure use_fn[T](f: pure (T) -> float, x: T) -> float { f(x) }\n";
    let (p, t) = clean(&format!("{DECLS}{fns}main() {{\n    let a = use_fn(tof, 2)\n}}\n"));
    assert_eq!(type_args(&t, the(&p, call("use_fn"))), ["int"]);
    assert_eq!(
        errors(&format!("{DECLS}{fns}main() {{\n    let a = use_fn(tof, 2.5)\n}}\n")),
        ["`T` in `use_fn` can't be both `int` (from `f`, which must match exactly) and `float` (from `x`)"]
    );
    assert_eq!(
        errors(&format!("{DECLS}main() {{\n    var xs: list[int] = [1]\n    push_all(var xs, 2.5)\n}}\n")),
        ["`T` in `push_all` can't be both `int` (from `xs`, which must match exactly) and `float` (from `v`)"]
    );
}

#[test]
fn an_exact_conflict_says_what_to_write() {
    let (p, d) = parse(&format!("{DECLS}main() {{\n    var xs: list[int] = [1]\n    push_all(var xs, 2.5)\n    let q = pick(1, \"x\")\n}}\n"));
    let (_, d2) = check_keeping_tables(&p, true);
    assert!(d.is_empty());
    let helps: Vec<Option<String>> = d2.into_iter().filter(|d| d.is_error()).map(|d| d.help).collect();
    assert_eq!(helps, [Some("write the type, like `push_all[float](...)`".to_string()), None]);
}

#[test]
fn closure_results_come_after_the_expected_type() {
    let convert = "pure convert[T, U](xs: list[T], f: pure (T) -> U) -> list[U] { list[U]() }\n";
    let (p, t) = clean(&format!(
        "{DECLS}{convert}main() {{\n    let xs: list[int] = [1]\n    let b: list[float] = convert(xs, (x: int) {{ x }})\n    let c: list[float] = convert(xs, (x) {{ x }})\n    let x: float = make(() {{ 3 }})\n}}\n"
    ));
    let converts = find_where(&p, call("convert"));
    assert_eq!(type_args(&t, converts[0]), ["int", "float"]);
    assert_eq!(type_args(&t, converts[1]), ["int", "float"]);
    assert_eq!(type_args(&t, the(&p, call("make"))), ["float"]);
}

#[test]
fn the_exact_meeting_is_named() {
    let src = format!("{DECLS}pure m3[T](a: T, b: list[T], c: T) -> int {{ 1 }}\nmain() {{\n    let x = m3(1, [2], 2.5)\n}}\n");
    assert_eq!(errors(&src), ["`T` in `m3` can't be both `int` (from `b`, which must match exactly) and `float` (from `c`)"]);
}

#[test]
fn a_missing_argument_is_the_only_error() {
    assert_eq!(errors(&format!("{DECLS}main() {{\n    let a = apply(f: (x) {{ 1 }})\n}}\n")), ["missing argument `x` for `apply`"]);
    assert_eq!(errors(&format!("{DECLS}main() {{\n    let s = Stack(item: [])\n}}\n")), ["`Stack` has no field named `item`"]);
}

/// The errors of `body` in a program where `Pool` owns an arena.
fn move_errors(body: &str) -> Vec<String> {
    let decls = "struct Pool {\n    mem: Arena\n    n: int\n}\nstruct Two[T] {\n    a: Pool\n    b: Pool\n    tag: T\n}\nstruct Box3[T] {\n    f: pure (int) -> int\n    a: Pool\n    tag: T\n}\nenum Held[T] {\n    Both { a: Pool, b: Pool, tag: T }\n}\n";
    errors(&format!("{decls}main() {{\n    let p = Pool(mem: Arena(), n: 1)\n    {body}\n}}\n"))
}

#[test]
fn inferred_constructions_move_in_source_order() {
    // Each inferred form gives exactly what the written form gives.
    let pairs = [
        ("let t = Two(a: p, b: p, tag: 1)", "let t = Two[int](a: p, b: p, tag: 1)"),
        ("let h = Held.Both(a: p, b: p, tag: 1)", "let h = Held[int].Both(a: p, b: p, tag: 1)"),
        ("let t = Two(a: p, b: Pool(mem: Arena(), n: p.n), tag: 1)", "let t = Two[int](a: p, b: Pool(mem: Arena(), n: p.n), tag: 1)"),
        // A deferred closure before the move sees nothing moved; one after it sees the move.
        ("let b = Box3(f: (x) { x + p.n }, a: p, tag: 1)", "let b = Box3[int](f: (x) { x + p.n }, a: p, tag: 1)"),
        ("let b = Box3(a: p, f: (x) { x + p.n }, tag: 1)", "let b = Box3[int](a: p, f: (x) { x + p.n }, tag: 1)"),
        ("let b = Box3(tag: 1, f: (x) { x }, a: p)", "let b = Box3[int](tag: 1, f: (x) { x }, a: p)"),
    ];
    for (inferred, written) in pairs {
        assert_eq!(move_errors(inferred), move_errors(written), "{inferred}");
    }
    let moved = ["`p` was moved earlier, so it can't be used here"];
    assert_eq!(move_errors(pairs[0].0), moved);
    assert_eq!(move_errors(pairs[1].0), moved);
    assert_eq!(move_errors(pairs[2].0), moved);
    assert!(move_errors(pairs[5].0).is_empty());
}

#[test]
fn recursion_binds_the_callers_placeholder() {
    let src = format!("{DECLS}pure walk[T](x: T, n: int) -> int {{\n    if n == 0 {{ return 0 }}\n    walk(x, n - 1)\n}}\nmain() {{\n    let w = walk(\"a\", 3)\n}}\n");
    let (p, t) = clean(&src);
    let walks = find_where(&p, call("walk"));
    assert_eq!(type_args(&t, walks[0]), ["T"]);
    assert_eq!(type_args(&t, walks[1]), ["string"]);
}

#[test]
fn a_generic_passes_its_var_parameter_on() {
    // The callee's `T` and the caller's are different placeholders with the same name: the
    // callee's is worked out as the caller's, so the `var` argument has exactly its type.
    let src = format!("{DECLS}pure fill_all[T](var ys: list[T], v: T) {{\n    push_all(var ys, v)\n}}\nmain() {{\n    var zs: list[int] = []\n    fill_all(var zs, 3)\n}}\n");
    let (p, t) = clean(&src);
    let push = the(&p, call("push_all"));
    assert_eq!(type_args(&t, push), ["T"]);
    assert_eq!(type_args(&t, the(&p, call("fill_all"))), ["int"]);
}

#[test]
fn what_cannot_be_told() {
    let errs = errors(&format!(
        "{DECLS}pure make_empty[T]() -> list[T] {{ list[T]() }}\npair_of[A, B](a: A) {{ }}\nmain() {{\n    let a = make_empty()\n    pair_of(1)\n    let s = Stack()\n    let t = Stack(items: [])\n    let m = Maybe.Nothing\n    let r = Outcome.Ok(value: 1)\n    let q = apply(1, (x) {{ }})\n    let u = make((x) {{ 1 }})\n}}\n"
    ));
    assert_eq!(
        errs,
        [
            "can't tell what `T` is in `make_empty`; write it, like `make_empty[int](...)`",
            "can't tell what `B` is in `pair_of`; write it, like `pair_of[int, int](...)`",
            "can't tell what `T` is in `Stack`; write it, like `Stack[int]()`",
            "can't tell what `T` is in `Stack`; write it, like `Stack[int]()`",
            "can't tell what `T` is in `Maybe`; write it, like `Maybe[int].Nothing`",
            "can't tell what `E` is in `Outcome`; write it, like `Outcome[int, int].Ok(...)`",
            "can't tell what `R` is in `apply`; write it, like `apply[int, int](...)`",
            "this closure takes 1 parameter, but `pure () -> T` takes no parameters",
        ]
    );
}

#[test]
fn one_error_per_call() {
    // The first conflict; the deferred closure after it isn't checked (no "give `x` a type").
    let errs = errors(&format!("{DECLS}pure three[T](a: T, b: T, f: pure (T) -> int) -> int {{ 1 }}\nmain() {{\n    let a = three(1, \"x\", (x) {{ 1 }})\n    let b = apply((y) {{ 1 }}, 2)\n}}\n"));
    assert_eq!(
        errs,
        [
            "`T` in `three` can't be both `int` (from `a`) and `string` (from `b`)",
            "can't tell what `T` is in `apply`; write it, like `apply[int, int](...)`",
        ]
    );
}

#[test]
fn worked_out_types_meet_their_rules() {
    let src = format!(
        "{DECLS}interface Named {{\n    label(self) -> string\n}}\nstruct Enemy: Named {{\n    hp: int\n    tags: list[int] = []\n    label(self) -> string {{ \"e\" }}\n}}\npure same[T: equatable](a: T, b: T) -> bool {{ a == b }}\nswap[T: equatable](var a: T, var b: T) {{\n    let c = a\n    a = b\n    b = c\n}}\nmain() {{\n    let e = Enemy(hp: 1)\n    let s = same(e, e)\n    let o: int? = none\n    let q = pick(o, o)\n    var n: Named = Enemy(hp: 2)\n    var k: Named = Enemy(hp: 3)\n    swap(var n, var k)\n    let w = pick(n, k)\n}}\n"
    );
    assert_eq!(
        errors(&src),
        [
            "`Enemy` isn't `equatable` (numbers, bools, strings, vectors, and structs and enums of them are), so it can't be `T` in `same`",
            "`int?` is an optional, so it can't be `T` yet; hold it in a struct field instead",
            "`Named` isn't `equatable` (numbers, bools, strings, vectors, and structs and enums of them are), so it can't be `T` in `swap`",
        ]
    );
}

#[test]
fn nothing_unfinished_escapes_plain_code() {
    let src = format!(
        "{DECLS}main() {{\n    let a = pick(1, 2)\n    let s: Stack[int] = Stack()\n    let t = Stack(items: [\"a\"])\n    let p = Pair(a: 1, b: s)\n    let m: Maybe[float] = Maybe.Just(value: 2.0)\n    let r = load()\n    let c = apply(2, (x) {{ Stack(items: [x]) }})\n}}\n"
    );
    let (p, t) = clean(&src);
    let main = p.items.iter().find_map(|i| match i {
        Item::Fn(f) if f.name == "main" => Some(&f.body),
        _ => None,
    });
    each_expr(main.expect("main"), &mut |e: &Expr| {
        assert!(t.is_concrete(t.ty(e.id)), "{:?} has type {}", e.kind, t.ty_name(t.ty(e.id)));
    });
    assert_eq!(t.ty_name(t.ty(the(&p, call("Pair")))), "Pair[int, Stack[int]]");
    assert_eq!(t.ty_name(t.ty(the(&p, method_call("Just")))), "Maybe[float]");
}
