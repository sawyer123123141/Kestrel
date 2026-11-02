//! Function types, named functions as values, and calls through function values.

use kestrel_check::{check, Res, Tables, Ty};
use kestrel_syntax::parse;

fn run(src: &str) -> (Option<Tables>, Vec<String>) {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check(&p);
    (t, d.into_iter().filter(|d| d.is_error()).map(|d| d.message).collect())
}

fn ok(src: &str) -> Tables {
    let (t, errs) = run(src);
    assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    t.expect("tables")
}

fn errors(src: &str) -> Vec<String> {
    run(src).1
}

/// The type of local `name`, as a message shows it.
fn local_ty(t: &Tables, name: &str) -> String {
    t.ty_name(t.locals.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no local {name}")).ty)
}

const FNS: &str = "square(x: int) -> int { x * x }
pure cube(x: int) -> int { x * x * x }
shout(s: string) { print(s) }
bump(var n: int, by: int) { n += by }
";

#[test]
fn named_functions_are_values_with_their_signature() {
    let t = ok(&format!("{FNS}main() {{\n    let f = square\n    let g = cube\n    let h = shout\n    let k = bump\n    let f2 = square\n}}\n"));
    assert_eq!(local_ty(&t, "f"), "(int) -> int");
    assert_eq!(local_ty(&t, "g"), "pure (int) -> int");
    assert_eq!(local_ty(&t, "h"), "(string)");
    assert_eq!(local_ty(&t, "k"), "(var int, int)");
    assert!(t.res.values().any(|r| matches!(r, Res::FnValue(f) if t.func(*f).name == "square")));
    // The same signature is the same type.
    let ty_of = |name: &str| t.locals.iter().find(|l| l.name == name).unwrap().ty;
    assert!(matches!(ty_of("f"), Ty::Fn(_)));
    assert_eq!(ty_of("f"), ty_of("f2"));
    assert_ne!(ty_of("f"), ty_of("g"), "pure is part of the type");
}

#[test]
fn calls_through_values_are_recorded() {
    let t = ok(&format!(
        "{FNS}struct Button {{\n    on_click: (int)\n    count: int = 0\n}}\n\
         apply(f: (int) -> int, x: int) -> int {{\n    f(x)\n}}\n\
         main() {{\n    var n = 1\n    let k = bump\n    k(var n, 2)\n    var fs = list[(int) -> int]()\n    fs.add(square)\n    print(fs[0](3), apply(cube, 2), apply(square, 3))\n    \
         let b = Button(on_click: shout_int)\n    b.on_click(4)\n}}\nshout_int(n: int) {{ print(n) }}\n"
    ));
    let calls = t.res.values().filter(|r| matches!(r, Res::CallValue(_))).count();
    assert_eq!(calls, 4, "f(x), k(var n, 2), fs[0](3), b.on_click(4)");
    assert_eq!(t.call_local.len(), 2, "f(x) and k(...) call a local by name");
    let apply = t.fns.iter().find(|f| f.name == "apply").unwrap();
    assert_eq!(t.ty_name(apply.params[0].ty), "(int) -> int");
}

#[test]
fn a_pure_function_goes_where_a_non_pure_type_is_expected_not_back() {
    ok(&format!("{FNS}main() {{\n    let f: (int) -> int = cube\n    let g: pure (int) -> int = cube\n    print(f(2), g(2))\n}}\n"));
    assert_eq!(
        errors(&format!("{FNS}main() {{\n    let g: pure (int) -> int = square\n}}\n")),
        vec!["expected pure (int) -> int, found (int) -> int"]
    );
    // A struct field and an argument take the same coercions.
    ok(&format!("{FNS}struct S {{\n    f: (int) -> int\n}}\nmain() {{\n    let s = S(cube)\n    print(s.f(2))\n}}\n"));
}

#[test]
fn signatures_must_match_exactly() {
    let wrong = |decl: &str, found: &str| {
        let src = format!("{FNS}main() {{\n    let f: {decl} = {found}\n}}\n");
        errors(&src)
    };
    assert_eq!(wrong("(int) -> float", "square"), vec!["expected (int) -> float, found (int) -> int"]);
    assert_eq!(wrong("(float) -> int", "square"), vec!["expected (float) -> int, found (int) -> int"]);
    assert_eq!(wrong("(int)", "square"), vec!["expected (int), found (int) -> int"]);
    // `var` is part of the type.
    assert_eq!(wrong("(int, int)", "bump"), vec!["expected (int, int), found (var int, int)"]);
    assert_eq!(wrong("(var int, int)", "bump"), Vec::<String>::new());
}

#[test]
fn arguments_through_a_value_are_checked_against_the_signature() {
    let src = |call: &str| format!("{FNS}main() {{\n    var n = 1\n    let f = square\n    let k = bump\n    {call}\n}}\n");
    assert_eq!(errors(&src("print(f(1, 2))")), vec!["`f` takes 1 argument(s), got 2"]);
    assert_eq!(errors(&src("print(f())")), vec!["`f` takes 1 argument(s), got 0"]);
    assert_eq!(errors(&src("print(f(true))")), vec!["expected int, found bool"]);
    assert_eq!(errors(&src("print(f(x: 1))")), vec!["calls through a function value take no labels"]);
    assert_eq!(errors(&src("k(n, 2)")), vec!["this argument of `k` is `var`, so the call must say so"]);
    assert_eq!(errors(&src("print(f(var n))")), vec!["this argument of `f` isn't `var`"]);
    assert_eq!(errors(&src("k(var n, 2.5)")), vec!["expected int, found float"]);
    assert_eq!(errors(&src("k(var 3, 2)")), vec!["only a variable can be passed as `var`"]);
    let ret = errors(&src("let s: string = f(2)"));
    assert_eq!(ret, vec!["expected string, found int"]);
    // The same argument can't be passed as `var` and read in the same call.
    assert_eq!(errors(&src("k(var n, n)")), Vec::<String>::new(), "numbers are copied before the call");
}

#[test]
fn the_value_a_call_reads_cannot_be_changed_by_its_arguments() {
    let src = "struct Holder {\n    f: (var Holder)\n    n: int\n}\nmain() {\n    var h = Holder(f: poke, n: 1)\n    h.f(var h)\n}\npoke(var h: Holder) { h.n += 1 }\n";
    assert_eq!(errors(src), vec!["can't call `h.f` while this call changes `h`, which may hold its memory"]);
}

#[test]
fn calling_a_non_function_says_what_it_is() {
    assert_eq!(errors("main() {\n    let x = 3\n    print(x(1))\n}\n"), vec!["`x` is a variable of type int, not a function"]);
    assert_eq!(errors("main() {\n    let xs = [1]\n    print(xs[0](1))\n}\n"), vec!["`xs[0]` has type int, which can't be called"]);
    assert_eq!(errors("f() -> int { 1 }\nmain() {\n    print(f()(1))\n}\n"), vec!["this function value has type int, which can't be called"]);
}

#[test]
fn a_call_through_a_value_can_be_a_statement_or_a_chain() {
    ok("make(n: int) -> (int) -> int {\n    if n > 0 { double } else { square }\n}\ndouble(x: int) -> int { x * 2 }\nsquare(x: int) -> int { x * x }\n\
        main() {\n    let fs = [make(1), make(0)]\n    print(make(1)(5))\n    fs[1](3)\n    print(fs[0](4))\n}\n");
}

#[test]
fn field_calls_fall_back_to_a_field_only_without_a_method_of_that_name() {
    let src = "struct B {\n    on_hit: (int)\n    count: int\n    bump(var self) { self.count += 1 }\n}\n\
               report(n: int) { print(n) }\n\
               main() {\n    var b = B(report, 0)\n    b.on_hit(3)\n    b.bump()\n    b.nope(1)\n}\n";
    assert_eq!(errors(src), vec!["`B` has no method `nope`"]);
    // The clash of a field and a method is refused at the declaration.
    assert_eq!(
        errors("struct B {\n    on_hit: (int)\n    on_hit(self) {}\n}\nmain() {}\n"),
        vec!["`B` has a field and a method both called `on_hit`; rename one"]
    );
    // A field that is not a function keeps the old message.
    assert_eq!(errors("struct B {\n    n: int\n    n(self) {}\n}\nmain() {}\n"), vec!["`B` already has a field named `n`"]);
}

#[test]
fn builtins_natives_and_methods_are_not_values() {
    let wrap = |name: &str| format!("main() {{\n    let f = {name}\n}}\n");
    assert_eq!(errors(&wrap("sqrt")), vec!["`sqrt` is built in; wrap it: `(x: float) { sqrt(x) }`"]);
    assert_eq!(errors(&wrap("max")), vec!["`max` is built in; wrap it: `(a: int, b: int) { max(a, b) }`"]);
    assert_eq!(errors(&wrap("randf")), vec!["`randf` is built in; wrap it: `() { randf() }`"]);
    assert_eq!(errors(&wrap("lerp")), vec!["`lerp` is built in; wrap it: `(a: float, b: float, t: float) { lerp(a, b, t) }`"]);
    assert_eq!(errors(&wrap("clamp")), vec!["`clamp` is built in; wrap it: `(x: int, lo: int, hi: int) { clamp(x, lo, hi) }`"]);
    assert_eq!(errors(&wrap("print")), vec!["`print` is built in; wrap it: `(x: int) { print(x) }`"]);
    let methods = "struct P {\n    x: int\n    get(self) -> int { self.x }\n}\nenum E {\n    A\n    go(self) {}\n}\n\
                   main() {\n    let p = P(1)\n    let f = p.get\n    let e = E.A\n    let g = e.go\n    let v = vec2(1, 2)\n    let h = v.length\n}\n";
    assert_eq!(
        errors(methods),
        vec![
            "methods can't be used as values yet; wrap it: `(e) { e.get() }`",
            "methods can't be used as values yet; wrap it: `(e) { e.go() }`",
            "methods can't be used as values yet; wrap it: `(e) { e.length() }`",
        ]
    );
}

#[test]
fn function_values_cannot_be_compared() {
    let msg = "function values can't be compared".to_string();
    assert_eq!(errors("f(x: int) {}\nmain() {\n    print(f == f)\n}\n"), vec![msg.clone()]);
    assert_eq!(errors("f(x: int) {}\nmain() {\n    print(f != f)\n}\n"), vec![msg.clone()]);
    let holder = "struct H {\n    cb: (int)\n}\nf(x: int) {}\nmain() {\n    let a = H(f)\n    print(a == a)\n}\n";
    assert_eq!(errors(holder), vec![msg.clone()]);
    // Nor in a list, an optional or an enum payload.
    let deep = "enum E {\n    W { cb: (int) }\n}\nf(x: int) {}\nmain() {\n    let a = E.W(f)\n    print(a == a)\n}\n";
    assert_eq!(errors(deep), vec![msg]);
    // `print` accepts them.
    ok("f(x: int) {}\nmain() {\n    print(f)\n    print(\"{f}\")\n}\n");
}

#[test]
fn function_values_are_not_map_keys_or_search_items() {
    let keys = "main() {\n    let m = map[(int), int]()\n}\n";
    assert_eq!(errors(keys), vec!["map keys must be integers, bools or strings, not `(int)`"]);
    let has = "f(x: int) {}\nmain() {\n    let xs = list[(int)]()\n    print(xs.find(f))\n}\n";
    assert_eq!(
        errors(has),
        vec!["`find` needs items that `==` can compare (numbers, bools, strings, vectors, and structs and enums of them); `list[(int)]` holds `(int)`"]
    );
}

#[test]
fn optional_function_types_are_refused_wherever_they_are_written() {
    let msg = "optional function types aren't supported yet; give the field a do-nothing default instead: `on_hit: (int) = (n) { }`";
    assert_eq!(errors("f(x: (int)?) {}\nmain() {}\n"), vec![msg]);
    ok("f(g: (int) -> int?) {}\nmain() {}\n"); // a function giving an optional
    assert_eq!(errors("f(x: ((int) -> int)?) {}\nmain() {}\n"), vec![msg]);
    // A list of functions may itself be optional.
    ok("main() {\n    let x: list[(int)]? = none\n}\n");
}

#[test]
fn unresolved_types_inside_function_types_report_once() {
    assert_eq!(errors("f(g: (Nope) -> int) {}\nmain() {}\n"), vec!["unknown type `Nope`"]);
    assert_eq!(errors("f(g: (int) -> Nope) {}\nmain() {}\n"), vec!["unknown type `Nope`"]);
}

#[test]
fn purity_is_part_of_calling_a_value() {
    let pure_fn = "pure twice(f: (int) -> int, x: int) -> int {\n    f(f(x))\n}\nmain() {}\n";
    let msg = "`f` isn't a pure function type, so a `pure` function can't call it; declare the parameter as `pure (int) -> int`";
    assert_eq!(errors(pure_fn), vec![msg, msg]);
    ok("pure twice(f: pure (int) -> int, x: int) -> int {\n    f(f(x))\n}\nmain() {}\n");
    let par = "pure double(x: int) -> int { x * 2 }\nbump(f: (int) -> int, var xs: [int]) {\n    par for x in xs {\n        x = f(x)\n    }\n}\nmain() {}\n";
    assert_eq!(errors(par), vec!["`par for` bodies can only call `pure` functions; `f` isn't a pure function type"]);
    ok("bump(f: pure (int) -> int, var xs: [int]) {\n    par for x in xs {\n        x = f(x)\n    }\n}\nmain() {}\n");
}

#[test]
fn constants_and_field_defaults_may_name_functions() {
    let t = ok("square(x: int) -> int { x * x }\ncube(x: int) -> int { x * x * x }\n\
                let RULE = square\nlet RULES = [square, cube]\n\
                struct S {\n    f: (int) -> int = square\n}\nmain() {\n    let s = S()\n    print(s.f(2), (RULE)(3), RULES[1](2))\n}\n");
    let s = t.structs.iter().find(|s| s.name == "S").unwrap();
    assert!(s.fields[0].has_default && !s.fields[0].default_allocates, "a named function allocates nothing");
    assert_eq!(t.ty_name(t.consts.iter().find(|c| c.name == "RULE").unwrap().ty), "(int) -> int");
    // A call through a value is not a constant expression.
    let bad = errors("square(x: int) -> int { x * x }\nlet RULE = square\nlet NINE = (RULE)(3)\nmain() {}\n");
    assert_eq!(
        bad,
        vec!["constants can only use literals, operators, other constants, struct and enum values, and pure built-ins like `sqrt`"]
    );
}

#[test]
fn a_function_value_is_stored_and_returned_without_arena_errors() {
    ok("square(x: int) -> int { x * x }\nmake() -> (int) -> int {\n    arena {\n        return square\n    }\n}\n\
        struct S {\n    f: (int) -> int\n}\nmain() {\n    var s = S(square)\n    arena {\n        s.f = square\n    }\n    print(s.f(3), make()(2))\n}\n");
}

#[test]
fn only_a_user_functions_signature_is_its_value() {
    // A function value of a function that takes a function.
    let t = ok("apply(f: (int) -> int) -> int { f(1) }\nsquare(x: int) -> int { x * x }\nmain() {\n    let a = apply\n    print(a(square))\n}\n");
    assert_eq!(local_ty(&t, "a"), "((int) -> int) -> int");
}

#[test]
fn a_removed_loop_item_cannot_be_called() {
    let src = |body: &str| {
        format!("square(x: int) -> int {{ x * x }}\nmain() {{\n    var fs = list[(int) -> int]()\n    fs.add(square)\n    for f in fs {{\n        {body}\n    }}\n}}\n")
    };
    assert_eq!(errors(&src("remove f\n        print(f(1))")), vec!["`f` was removed earlier, so it can't be used here"]);
    assert_eq!(errors(&src("if true { remove f }\n        print(f(2))")), vec!["`f` may have been removed earlier, so it can't be used here"]);
    ok(&src("print(f(3))"));
}

#[test]
fn a_constant_of_function_type_is_called_by_its_name() {
    let t = ok("square(x: int) -> int { x * x }\nlet RULE = square\nmain() {\n    print(RULE(3))\n    RULE(4)\n}\n");
    assert!(t.res.values().filter(|r| matches!(r, Res::CallValue(_))).count() == 2);
    assert_eq!(t.call_const.len(), 2);
    // The call is checked like any call through a value.
    let src = |call: &str| format!("square(x: int) -> int {{ x * x }}\nlet RULE = square\nmain() {{\n    print({call})\n}}\n");
    assert_eq!(errors(&src("RULE(1, 2)")), vec!["`RULE` takes 1 argument(s), got 2"]);
    assert_eq!(errors(&src("RULE(x: 1)")), vec!["calls through a function value take no labels"]);
    assert_eq!(
        errors("square(x: int) -> int { x * x }\nlet RULE = square\npure f() -> int { RULE(2) }\nmain() {}\n"),
        vec!["`RULE` isn't a pure function type, so a `pure` function can't call it; declare the parameter as `pure (int) -> int`"]
    );
    ok("pure square(x: int) -> int { x * x }\nlet RULE = square\npure f() -> int { RULE(2) }\nmain() {}\n");
    // A constant that isn't a function keeps its message.
    assert_eq!(
        errors("let RED = 1\nmain() {\n    print(RED(2))\n}\n"),
        vec!["`RED` is a constant, not a function"]
    );
}
