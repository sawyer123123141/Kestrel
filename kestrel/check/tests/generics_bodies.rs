//! Generic code is checked once against its rules (R1): calls through a rule, conversions to a
//! rule's interface, methods of instances, field defaults, `match` and closures.

use kestrel_check::{check_keeping_tables, FnId, Res, Tables, Ty};
use kestrel_syntax::ast::{Block, Expr, ExprId, Item, Program};
use kestrel_syntax::parse;
use kestrel_syntax::visit::each_expr;

fn run(src: &str) -> (Program, Tables, Vec<String>) {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check_keeping_tables(&p, true);
    let errs = d.into_iter().filter(|d| d.is_error()).map(|d| d.message).collect();
    (p, t, errs)
}

/// Checks clean and returns the program and tables.
fn clean(src: &str) -> (Program, Tables) {
    let (p, t, errs) = run(src);
    assert!(errs.is_empty(), "{errs:?}");
    (p, t)
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

/// The ids of every expression whose s-expression is `sexpr`, in source order.
fn find(p: &Program, sexpr: &str) -> Vec<ExprId> {
    let mut out = Vec::new();
    for b in bodies(p) {
        each_expr(b, &mut |e: &Expr| {
            if e.to_sexpr() == sexpr {
                out.push(e.id);
            }
        });
    }
    out
}

fn the(p: &Program, sexpr: &str) -> ExprId {
    let all = find(p, sexpr);
    assert_eq!(all.len(), 1, "expressions {sexpr}: {all:?}");
    all[0]
}

fn local_ty(t: &Tables, f: &str, name: &str) -> String {
    let fid: FnId = t.fn_by_name[f];
    let id = t.func(fid).locals.iter().find(|&&l| t.local(l).name == name).unwrap_or_else(|| panic!("no local {name} in {f}"));
    t.ty_name(t.local(*id).ty)
}

#[test]
fn a_rule_call_is_recorded() {
    let src = "interface Damageable {\n    pure hp(self) -> int\n}\npure total_hp[T: Damageable](xs: list[T]) -> int {\n    var sum = 0\n    for x in xs { sum += x.hp() }\n    sum\n}\nmain() {}\n";
    let (p, t) = clean(src);
    let call = the(&p, "(.call x hp)");
    assert!(matches!(t.res.get(&call), Some(Res::ParamCall { slot: 0, .. })), "{:?}", t.res.get(&call));
    assert_eq!(t.ty(call), Ty::Int);
    assert_eq!(local_ty(&t, "total_hp", "x"), "T");
    assert_eq!(local_ty(&t, "total_hp", "xs"), "list[T]");
}

#[test]
fn a_placeholder_is_lent_to_its_rule() {
    let src = "interface Damageable {\n    pure hp(self) -> int\n}\nshow(d: Damageable) {}\nf[T: Damageable](x: T) { show(x) }\npure g[T](x: T) -> T? { x }\nmain() {}\n";
    let (p, t) = clean(src);
    let x = find(&p, "x");
    // `x` in `show(x)` is the only `x` argument; `g`'s tail is the other.
    let lent: Vec<&ExprId> = x.iter().filter(|e| t.iface_borrows.contains(e)).collect();
    assert_eq!(lent.len(), 1, "{x:?}");
    assert!(matches!(t.coerce.get(lent[0]), Some(Ty::Iface(_))));
    let tail = x.iter().find(|e| !t.iface_borrows.contains(e) && matches!(t.coerce.get(e), Some(Ty::Optional(_)))).expect("g's tail is converted to T?");
    assert_eq!(t.ty_name(t.coerce[tail]), "T?");
}

#[test]
fn methods_on_instances_are_substituted() {
    let decl = "struct Stack[T] {\n    items: list[T] = []\n    push(var self, v: T) { self.items.add(v) }\n    pure size(self) -> int { self.items.len }\n    pure twice(self) -> int { self.size() * 2 }\n}\n";
    let (p, t, errs) = run(&format!("{decl}f(var s: Stack[int]) {{ s.push(3) }}\nmain() {{}}\n"));
    // `push` storing its parameter in `self` is checked at each call (P38), and `f` passes it on.
    assert!(errs.is_empty(), "{errs:?}");
    let push = the(&p, "(.call s push 3)");
    let args = t.generic.type_args[&push];
    assert_eq!(t.ty_list(args), [Ty::Int]);
    let Some(Res::Fn(m)) = t.res.get(&push) else { panic!("a method call: {:?}", t.res.get(&push)) };
    assert_eq!(t.func(*m).name, "Stack.push");
    // Inside its own declaration the arguments are its own placeholders.
    let own = the(&p, "(.call self size)");
    let args = t.ty_list(t.generic.type_args[&own]);
    assert_eq!(args.len(), 1);
    assert!(matches!(args[0], Ty::Param(_)) && t.ty_name(args[0]) == "T", "{args:?}");
    // A wrong argument is judged against the substituted parameter.
    let (_, _, errs) = run(&format!("{decl}g(var s: Stack[int]) {{ s.push(\"x\") }}\nmain() {{}}\n"));
    assert_eq!(errs, ["expected int, found string"]);
}

#[test]
fn generic_code_is_checked_once() {
    let (_, _, errs) = run("pure f[T](x: T) -> int { x.hp }\nmain() {}\n");
    assert_eq!(errs, ["`x` is a `T`, which can be any type, so it has no field `hp`"]);
    let (_, _, errs) = run("struct B[T] {\n    v: T\n    pure bad(self) -> int { self.v.hp }\n}\nmain() {}\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
}

#[test]
fn match_on_a_generic_enum_in_generic_code() {
    let src = "enum Outcome[T, E] {\n    Ok { value: T }\n    Err { error: E }\n}\npure unwrap_or[T, E](r: Outcome[T, E], d: T) -> T {\n    match r {\n        Ok o { o.value }\n        Err e { d }\n    }\n}\nmain() {}\n";
    let (p, t) = clean(src);
    assert_eq!(t.ty_name(t.ty(the(&p, "(. o value)"))), "T");
}

#[test]
fn defaults_of_a_generic_struct() {
    clean("struct Bag[T] {\n    items: list[T] = []\n    spare: T? = none\n}\nmain() {}\n");
    let (_, _, errs) = run("struct B2[T] {\n    v: T = 3\n}\nmain() {}\n");
    assert_eq!(errs, ["expected T, found int"]);
}

#[test]
fn a_default_cycle_through_a_generic_is_reported_once() {
    let src = "struct Node[T] {\n    kids: list[Node] = [Node()]\n}\nf(x: Node[int], y: Node[string]) {}\nmain() {}\n";
    let (_, _, errs) = run(src);
    let loops: Vec<&String> = errs.iter().filter(|e| e.contains("field defaults build each other in a loop")).collect();
    assert_eq!(loops.len(), 1, "{errs:?}");
}

#[test]
fn closures_take_and_capture_t() {
    let src = "pure stamp[T](xs: list[T], tag: T) -> int {\n    let f = (x: T) { tag }\n    var n = 0\n    for x in xs {\n        let y = f(x)\n        n += 1\n    }\n    n\n}\nmain() {}\n";
    let (_, t) = clean(src);
    assert_eq!(local_ty(&t, "stamp", "f"), "pure (T) -> T");
}

#[test]
fn a_rule_call_is_pure_if_any_rule_says_so() {
    let ifaces = "interface A {\n    pure m(self) -> int\n}\ninterface B {\n    m(self) -> int\n}\n";
    let (p, t) = clean(&format!("{ifaces}pure f[T: A + B](x: T) -> int {{ x.m() }}\npure g[T: B + A](x: T) -> int {{ x.m() }}\nmain() {{}}\n"));
    // Both record the call through `A`, whose method is `pure`.
    for call in find(&p, "(.call x m)") {
        let Some(Res::ParamCall { iface, .. }) = t.res.get(&call) else { panic!("a rule call") };
        assert_eq!(t.iface(*iface).name, "A");
    }
}

#[test]
fn a_var_self_rule_call_needs_a_changeable_receiver() {
    let ifaces = "interface Hit {\n    hit(var self, n: int)\n}\n";
    clean(&format!("{ifaces}f[T: Hit](var x: T) {{ x.hit(1) }}\nmain() {{}}\n"));
    let (_, _, errs) = run(&format!("{ifaces}f[T: Hit](x: T) {{ x.hit(1) }}\nmain() {{}}\n"));
    assert_eq!(errs, ["can't call `hit` on parameter `x`: it changes `x`, which isn't `var`"]);
    let (_, _, errs) = run(&format!("{ifaces}f[T: Hit](var x: T) {{ x.hit(\"a\") }}\nmain() {{}}\n"));
    assert_eq!(errs, ["expected int, found string"]);
}

#[test]
fn rules_make_comparing_and_sorting_possible() {
    clean("pure same[T: equatable](a: T, b: T) -> bool { a == b }\npure diff[T: key](a: T, b: T) -> bool { a != b }\npure less[T: ordered](a: T, b: T) -> bool { a <= b }\nmain() {}\n");
    clean("pure has_it[T: equatable](xs: list[T], x: T) -> bool { xs.has(x) }\nmain() {}\n");
    clean("sorted[T: ordered](xs: list[T]) -> list[T] {\n    var ys = xs\n    ys.sort()\n    ys\n}\nmain() {}\n");
    clean("pure by[T: ordered](xs: list[T]) -> int { var ys = xs\n    ys.sort_by((x) { x })\n    ys.len }\nmain() {}\n");
    clean("pure keyed[K: key](k: K) -> int {\n    var m = map[K, int]()\n    m[k] = 1\n    m.len\n}\nmain() {}\n");
    // A map type and a map literal need the rule for a `K` key too.
    let (_, _, errs) = run("pure f[K](k: K, m: map[K, int]) -> int {\n    let lit = {k: 1}\n    1\n}\nmain() {}\n");
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs.iter().all(|e| e == "`K` can't be a map key without the rule `key`: write `[K: key]`"), "{errs:?}");
    let (_, _, errs) = run("pure by[T](xs: list[T]) -> int { var ys = xs\n    ys.sort_by((x) { x })\n    ys.len }\nmain() {}\n");
    assert_eq!(errs, ["`sort_by` needs `T` to be `ordered`: write `[T: ordered]`"]);
    let (_, _, errs) = run("pure f[T: equatable](a: T, b: T) -> bool { a < b }\nmain() {}\n");
    assert_eq!(errs, ["`<` on `T` needs the rule `ordered`: write `[T: ordered]`"]);
    let (_, _, errs) = run("pure f[T, U](a: T, b: U) -> bool { a == b }\nmain() {}\n");
    assert_eq!(errs, ["`==` needs two numbers, two bools or two strings, found T and U"]);
}

#[test]
fn the_array_message_replaces_the_zero_fill_error() {
    let (_, _, errs) = run("struct Box[T] {\n    v: T\n}\npure f[T](n: int) -> int {\n    let xs = array[Box[T]](n)\n    xs.len\n}\nmain() {}\n");
    assert_eq!(errs, ["`array[Box[T]](n)` fills its items with zeros, which a `Box[T]` may not have; build a `list[Box[T]]` with `add` instead"]);
    // Plain code keeps today's message.
    let (_, _, errs) = run("interface N {\n    pure name(self) -> string\n}\nf() {\n    let xs = array[N](3)\n}\nmain() {}\n");
    assert_eq!(errs, ["`array[N](n)` would start with empty items; build it with a literal or `add`"]);
}

#[test]
fn a_placeholder_calls_through_an_interface_it_does_not_have() {
    let (_, _, errs) = run("interface A {\n    pure m(self) -> int\n}\ninterface B {\n    pure n(self) -> int\n}\npure f[T: A + B](x: T) -> int { x.k() }\nmain() {}\n");
    assert_eq!(errs, ["`T` has no method `k`: its rules (`A`, `B`) don't have one"]);
}

#[test]
fn what_a_placeholder_can_do_without_rules() {
    clean(
        "pure pick[T](a: T, b: T, first: bool) -> T {\n    if first { a } else { b }\n}\n\
         pure wrap[T](x: T) -> list[T] { [x] }\n\
         pure count[T](xs: list[T]) -> int { xs.len }\n\
         pure lookup[K: key, V](m: map[K, V], k: K, d: V) -> V { m.get(k, d) }\n\
         pure opt[T](x: T) -> T? { x }\n\
         show[T](x: T) { print(x) }\n\
         shows[T](x: T) { print(\"got {x}\") }\n\
         pure apply[T](x: T, f: pure (T) -> T) -> T { f(x) }\n\
         pure last_of[T](xs: list[T]) -> T? {\n    var out: T? = none\n    for x in xs { out = x }\n    out\n}\n\
         main() {}\n",
    );
}

#[test]
fn a_rule_call_whose_result_is_unused_is_refused_when_pure() {
    let ifaces = "interface Damageable {\n    pure hp(self) -> int\n}\n";
    let (_, _, errs) = run(&format!("{ifaces}f[T: Damageable](x: T) {{ x.hp() }}\nmain() {{}}\n"));
    assert_eq!(errs, ["the result of pure function `Damageable.hp` is unused, so this line does nothing"]);
    // A rule's method is not a value, as an interface's is not.
    let (_, _, errs) = run(&format!("{ifaces}f[T: Damageable](x: T) -> int {{ x.hp }}\nmain() {{}}\n"));
    assert_eq!(errs, ["methods can't be used as values yet; wrap it: `(e) { e.hp() }`"]);
}

#[test]
fn a_field_of_a_placeholder_says_which_rule_to_add() {
    let (_, _, errs) = run("pure f[T](x: T) -> int { x.hp }\nmain() {}\n");
    assert_eq!(errs, ["`x` is a `T`, which can be any type, so it has no field `hp`"]);
    let (p, d) = parse("pure f[T](x: T) -> int { x.hp }\nmain() {}\n");
    let (_, d2) = check_keeping_tables(&p, true);
    assert!(d.is_empty());
    assert_eq!(d2[0].help.as_deref(), Some("give `T` a rule with a method instead, like `[T: HasHp]`"));
}

#[test]
fn operators_on_a_placeholder_are_refused() {
    for (op, src) in [
        ("-", "pure f[T](a: T) -> T { -a }"),
        ("~", "pure f[T](a: T) -> T { ~a }"),
        ("*", "pure f[T](a: T, b: T) -> T { a * b }"),
        ("&", "pure f[T](a: T, b: T) -> T { a & b }"),
        ("<<", "pure f[T](a: T, b: T) -> T { a << b }"),
    ] {
        let (_, _, errs) = run(&format!("{src}\nmain() {{}}\n"));
        assert_eq!(errs, [format!("`{op}` doesn't work on `T`, which can be any type; take a function that does it, like `add: (T, T) -> T`")]);
    }
}

#[test]
fn a_written_instance_in_a_generic_body_meets_its_rules() {
    let pair = "struct Ordered2[A: ordered] {\n    a: A\n}\n";
    let (_, _, errs) = run(&format!("{pair}pure f[T](x: T) -> int {{\n    let o: Ordered2[T]? = none\n    1\n}}\nmain() {{}}\n"));
    // (The `none` after a refused annotation says it can't tell its kind; 2b noted it.)
    assert_eq!(errs[0], "`T` isn't `ordered` (numbers, bools and strings are), so it can't be `A` in `Ordered2`; add the rule: `[T: ordered]`");
    clean(&format!("{pair}pure f[T: ordered](x: T) -> int {{\n    let o: Ordered2[T]? = none\n    1\n}}\nmain() {{}}\n"));
}

#[test]
fn a_rule_call_through_an_enum_method_and_a_generic_enums_own_name() {
    let src = "enum Maybe[T] {\n    Nothing\n    Just { value: T }\n    pure is_just(self) -> bool {\n        match self {\n            Just j { true }\n            Nothing { false }\n        }\n    }\n    pure again(self) -> bool { self.is_just() }\n}\nmain() {}\n";
    let (p, t) = clean(src);
    let call = the(&p, "(.call self is_just)");
    let args = t.ty_list(t.generic.type_args[&call]);
    assert_eq!(args.len(), 1);
    assert_eq!(t.ty_name(args[0]), "T");
}

#[test]
fn replacing_a_whole_var_placeholder_is_refused_like_an_interface() {
    // A `T` may be an interface type in some copy, where the caller's concrete place is lent to
    // `a` and a replacement of another type would not fit. The refusal comes from
    // `check_iface_replace` and stands in for the arena message: nothing else is reported.
    let plain = "can't replace `a`: it may be the caller's value of another type";
    for body in [
        "f[T](var a: T, b: T) { a = b }",
        "f[T](var a: T, xs: list[T]) { a = xs[0] }",
        "f[T](var a: T, b: T, c: bool) { a = if c { a } else { b } }",
    ] {
        let (_, _, errs) = run(&format!("{body}\nmain() {{}}\n"));
        assert_eq!(errs, [plain], "{body}");
    }
    // With a rule, the example is a type that has it.
    let src = "interface D {\n    pure hp(self) -> int\n}\nstruct G: D {\n    pure hp(self) -> int { 1 }\n}\nf[T: D](var a: T, b: T) { a = b }\nmain() {}\n";
    let (_, _, errs) = run(src);
    assert_eq!(errs, ["can't replace `a`: it may be the caller's `G`; change it through its methods"]);
    // Changing a field or calling a method of it is not a replacement.
    clean("interface D {\n    hit(var self)\n}\nf[T: D](var a: T) { a.hit() }\nmain() {}\n");
    // The same replacement of a plain interface is refused as before.
    let (_, _, errs) = run("interface D {\n    pure hp(self) -> int\n}\ng(var a: D, b: D) { a = b }\nmain() {}\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
}

#[test]
fn a_declarations_own_type_is_written_with_its_placeholders() {
    // Inside `Pair`, its bare name means `Pair[A, B]` (G2): messages write it so.
    let src = "struct Pair[A, B] {\n    a: A\n    b: B\n    pure same(self) -> Pair[B, A] { self }\n}\nmain() {}\n";
    let (_, _, errs) = run(src);
    assert_eq!(errs, ["expected Pair[B, A], found Pair[A, B]"]);
}
