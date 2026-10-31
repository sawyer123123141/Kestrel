use kestrel_check::{check, Tables, Ty};
use kestrel_syntax::parse;

fn run(src: &str) -> (Option<Tables>, Vec<String>) {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check(&p);
    (t, d.into_iter().map(|d| d.message).collect())
}

fn ok(src: &str) -> Tables {
    let (t, errs) = run(src);
    assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    t.expect("tables")
}

fn errors(src: &str) -> Vec<String> {
    run(src).1
}

const NEVER_IF: &str = "this `if` always leaves early, so it has no value";

fn local_ty(t: &Tables, name: &str) -> Ty {
    t.locals.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no local {name}")).ty
}

#[test]
fn statements_and_inference() {
    let t = ok("main() {\n var x = 1\n x += 2\n let y: float = x\n while x < 10 { x = x * 2 }\n for i from 0 to 3 { x -= i }\n let z = if x > 3 { 1 } else { 2 }\n}\n");
    assert_eq!(local_ty(&t, "x"), Ty::Int);
    assert_eq!(local_ty(&t, "y"), Ty::Float);
    assert_eq!(local_ty(&t, "z"), Ty::Int);
    assert_eq!(local_ty(&t, "i"), Ty::Int);
    assert!(t.coerce.values().any(|&ty| ty == Ty::Float), "x should be coerced to float");
}

#[test]
fn literals_take_the_expected_type() {
    let t = ok("main() {\n let a: i64 = 5\n let b: u8 = 255\n let c: f64 = 1.5\n let d: float = 3\n}\n");
    assert_eq!(local_ty(&t, "a"), Ty::I64);
    assert_eq!(local_ty(&t, "b"), Ty::U8);
    assert_eq!(local_ty(&t, "c"), Ty::F64);
    assert_eq!(local_ty(&t, "d"), Ty::Float);
}

#[test]
fn constants() {
    let t = ok("let SIZE = 32\nlet AREA = SIZE * SIZE\nmain() {\n let x = AREA\n}\n");
    assert_eq!(t.consts[1].ty, Ty::Int);
    assert_eq!(local_ty(&t, "x"), Ty::Int);
}

#[test]
fn a_constant_may_use_one_declared_below_it() {
    let t = ok("let AREA = SIZE * SIZE\nlet SIZE = WIDTH + 1\nlet WIDTH = 31\nmain() {}\n");
    assert_eq!([t.consts[0].ty, t.consts[1].ty, t.consts[2].ty], [Ty::Int; 3]);
}

#[test]
fn constants_are_ordered_after_the_constants_they_use() {
    let t = ok("let AREA = SIZE * SIZE\nlet SIZE = WIDTH + 1\nlet WIDTH = 31\nlet OTHER = 7\nmain() {}\n");
    let ids: Vec<u32> = t.const_order.iter().map(|c| c.0).collect();
    // WIDTH (2), then SIZE (1), then AREA (0); the unrelated OTHER (3) keeps its place after them.
    assert_eq!(ids, [2, 1, 0, 3]);
}

#[test]
fn diverging_branches_satisfy_return_type() {
    ok("f(x: int) -> int {\n if x > 0 { return 1 } else { return 2 }\n}\nmain() {}\n");
    ok("f(x: int) -> int {\n if x > 0 { 1 } else { 2 }\n}\nmain() {}\n");
}

#[test]
fn errors_are_reported() {
    let cases: &[(&str, &str)] = &[
        ("f() {}\n", "program needs a `main()` function"),
        ("main() {\n let x = 1\n if true { let x = 2 }\n}\n", "`x` is already defined; Kestrel doesn't allow shadowing"),
        ("main() {\n let x = 1\n x = 2\n}\n", "can't change `x`: it was declared with `let`"),
        ("main() {\n let x: int = true\n}\n", "expected int, found bool"),
        ("main() {\n let x: int = 1.5\n}\n", "expected int, found float"),
        ("main() {\n let x = 1 + true\n}\n", "`+` needs numbers, found int and bool"),
        ("main() {\n while 1 { }\n}\n", "expected bool, found int"),
        ("main() {\n let x = y\n}\n", "unknown name `y`"),
        ("main() {\n let x = 3000000000\n}\n", "`3000000000` doesn't fit in int"),
        ("main() {\n let b: u8 = 256\n}\n", "`256` doesn't fit in u8"),
        ("main() {\n break\n}\n", "`break` outside a loop"),
        ("main() {\n var total = 1\n total = total\n - 2\n}\n", "this line has no effect; did you mean to continue the line above?"),
        ("main() {\n let x = if true { 1 }\n}\n", "this `if` needs an `else` to produce a value"),
        ("f() -> int { }\nmain() {}\n", "`f` must return int, but its body can finish without a value"),
        ("let A = B\nlet C = 1\nmain() {}\n", "unknown name `B`"),
        ("f(x: vec9) {}\nmain() {}\n", "unknown type `vec9`"),
        ("f() {}\nmain() {\n let s = \"n = {f()}\"\n}\n", "this has no value to print"),
        ("pure f(var x: int) {\n print(x)\n}\nmain() {}\n", "pure functions can't call `print`"),
        ("f(x: int) {\n x = 1\n}\nmain() {}\n", "can't change parameter `x`"),
        ("main() {\n let u: u8 = 1\n let n = -u\n}\n", "can't negate a u8"),
        ("main() {}\nmain() {}\n", "`main` is defined twice"),
        ("print() {}\nmain() {}\n", "`print` is a built-in name; pick another"),
        ("f() -> vec9 { }\nmain() {}\n", "unknown type `vec9`"),
        ("main() {\n let y: int = if true { return } else { return }\n}\n", NEVER_IF),
        ("f(x: int) {}\nmain() {\n f(if true { return } else { return })\n}\n", NEVER_IF),
        ("main() {\n let s = \"a\" + 1\n}\n", "`+` needs two numbers or two strings, found string and int"),
        ("main() {\n let b = \"a\" < 1\n}\n", "`<` needs two numbers, two strings or two bools, found string and int"),
        ("main() {\n let b = true <= 1\n}\n", "`<=` needs two numbers, two strings or two bools, found bool and int"),
        ("main() {\n let b = \"a\" > true\n}\n", "`>` needs two numbers, two strings or two bools, found string and bool"),
        ("main() {\n let b = 1 >= \"x\"\n}\n", "`>=` needs two numbers, two strings or two bools, found int and string"),
        ("struct P {\n x: int\n}\nmain() {\n let b = P(1) < P(2)\n}\n", "`<` needs two numbers, two strings or two bools, found P and P"),
    ];
    for (src, expected) in cases {
        let errs = errors(src);
        assert_eq!(errs, vec![expected.to_string()], "source:\n{src}");
    }
}

#[test]
fn two_strings_or_two_bools_order_and_give_a_bool() {
    ok("main() {\n let a: bool = \"a\" < \"b\"\n let b: bool = \"a\" >= \"b\"\n let c: bool = false < true\n let d: bool = true > false\n print(a, b, c, d, \"a\" <= \"a\")\n}\n");
}

#[test]
fn names_starting_with_the_compilers_prefix_are_reserved() {
    const RESERVED: &str = "names starting with `__kestrel_` are reserved for the compiler; pick another";
    // Items, locals, loop variables and parameters can't take such a name: the compiler makes
    // functions with these names, and a user one would collide with them.
    for src in [
        "__kestrel_constants() {}\nmain() {}\n",
        "let __kestrel_copy_1 = 3\nmain() {}\n",
        "struct __kestrel_print_2 {\n x: int\n}\nmain() {}\n",
        "enum __kestrel_drop_3 {\n A\n}\nmain() {}\n",
        "main() {\n let __kestrel_par_0 = 1\n}\n",
        "main() {\n for __kestrel_x from 0 to 3 { }\n}\n",
        "f(__kestrel_x: int) {}\nmain() {}\n",
    ] {
        assert_eq!(errors(src), vec![RESERVED.to_string()], "source:\n{src}");
    }
    // Only the whole prefix is reserved.
    for src in ["__kestrel() {}\nmain() {}\n", "my__kestrel_x() {}\nmain() {}\n", "main() {\n let _kestrel_x = 1\n print(_kestrel_x)\n}\n"] {
        assert_eq!(errors(src), Vec::<String>::new(), "source:\n{src}");
    }
}

#[test]
fn always_leaving_if_has_no_value_outside_a_tail() {
    let programs = [
        "main() {\n print(if true { return } else { return })\n}\n",
        "main() {\n var x = 1\n x = if true { return } else { return }\n}\n",
        "main() {\n while if true { return } else { return } { }\n}\n",
        "f() -> int {\n return if true { return 1 } else { return 2 }\n}\nmain() {}\n",
        "main() {\n for i from if true { return } else { return } to 3 { }\n}\n",
        "main() {\n let y: int = if true { if true { return } else { return } } else { return }\n}\n",
    ];
    for src in programs {
        assert_eq!(errors(src), vec![NEVER_IF.to_string()], "source:\n{src}");
    }
}

#[test]
fn while_true_without_break_never_finishes() {
    ok("f(n: int) -> int {\n var i = 0\n while true {\n  if i * i > n { return i }\n  i += 1\n }\n}\nmain() {}\n");
    // A `break` in an inner loop leaves only that loop.
    ok("f() -> int {\n while true {\n  for i from 0 to 3 { break }\n  return 1\n }\n}\nmain() {}\n");
    let must = "`f` must return int, but its body can finish without a value";
    assert_eq!(errors("f() -> int {\n while true { break }\n}\nmain() {}\n"), vec![must]);
    assert_eq!(errors("f(c: bool) -> int {\n while c { return 1 }\n}\nmain() {}\n"), vec![must]);
}

#[test]
fn partly_leaving_if_is_still_a_value() {
    ok("f(x: int) -> int {\n let y = if x > 0 { x * 2 } else { return -1 }\n y\n}\nmain() {}\n");
    ok("f(x: int) -> int {\n let y: int = if x > 0 { if x > 5 { return 1 } else { return 2 } } else { 3 }\n y\n}\nmain() {}\n");
}

#[test]
fn value_branches_must_produce_a_value() {
    let programs = [
        "f(c: bool) -> int {\n var y = 0\n if c { 1 } else { y = 2 }\n}\nmain() {}\n",
        "main() {\n let z: int = if true { 1 } else { }\n}\n",
        "main() {\n let c = true\n let x = if c { 1 } else { }\n}\n",
        "main() {\n let z: int = if true { } else { 1 }\n}\n",
    ];
    for src in programs {
        assert_eq!(errors(src), vec!["this branch has no value".to_string()], "source:\n{src}");
    }
}

#[test]
fn assigning_var_param_marks_function() {
    let t = ok("bump(var x: int) {\n x += 1\n}\nmain() {}\n");
    assert!(t.fns[0].mutates_params);
}

use kestrel_check::Res;

#[test]
fn calls_resolve_and_mark_var_arguments() {
    let t = ok("pure square(x: int) -> int { x * x }\nbump(var n: int, by: int) {\n n += by\n}\nmain() {\n var total = 0\n bump(var total, by: square(3))\n bump(var total, 2)\n print(\"total\", total, 1.5, true)\n let f = float(total) / 2.0\n let i = int(f)\n}\n");
    let total = t.locals.iter().find(|l| l.name == "total").unwrap();
    assert!(total.addressed);
    assert!(t.fns[1].mutates_params);
    assert!(t.fns[2].io);
    assert_eq!(t.call_args.len(), 3);
    assert!(t.res.values().any(|r| *r == Res::Print));
    assert!(t.res.values().any(|r| *r == Res::Convert(Ty::Int)));
    assert_eq!(local_ty(&t, "f"), Ty::Float);
    assert_eq!(local_ty(&t, "i"), Ty::Int);
}

#[test]
fn labels_reorder_arguments() {
    let t = ok("f(a: int, b: int) -> int { a - b }\nmain() {\n let x = f(b: 1, a: 2)\n}\n");
    assert_eq!(t.call_args.values().next().unwrap(), &vec![1, 0]);
}

#[test]
fn call_errors() {
    let cases: &[(&str, &str)] = &[
        ("pure f() -> int {\n print(1)\n 1\n}\nmain() {}\n", "pure functions can't call `print`"),
        ("g() {}\npure f() {\n g()\n}\nmain() {}\n", "pure functions can only call pure functions; `g` isn't pure"),
        ("f(var x: int) {}\nmain() {\n var a = 1\n f(a)\n}\n", "parameter `x` of `f` is `var`, so the call must say so"),
        ("f(x: int) {}\nmain() {\n var a = 1\n f(var a)\n}\n", "parameter `x` of `f` isn't `var`"),
        ("f(var x: int) {}\nmain() {\n let a = 1\n f(var a)\n}\n", "can't pass `a` as `var`: it was declared with `let`"),
        ("f(var x: int, var y: int) {}\nmain() {\n var a = 1\n f(var a, var a)\n}\n", "`a` is passed as `var` and is also passed in another argument of the same call"),
        ("f(x: int) {}\nmain() {\n f(1, 2)\n}\n", "`f` takes 1 argument(s), got 2"),
        ("f(x: int) {}\nmain() {\n f()\n}\n", "missing argument `x` for `f`"),
        ("f(x: int) {}\nmain() {\n f(y: 1)\n}\n", "`f` has no parameter named `y`"),
        ("f(a: int, b: int) {}\nmain() {\n f(b: 1, 2)\n}\n", "positional arguments must come before labelled ones"),
        ("main() {\n g()\n}\n", "unknown function `g`"),
        ("pure sq(x: int) -> int { x * x }\nmain() {\n sq(2)\n}\n", "the result of pure function `sq` is unused, so this line does nothing"),
        ("f(var x: i64) {}\nmain() {\n var a = 1\n f(var a)\n}\n", "`var` parameter `x` needs exactly i64, found int"),
        ("main() {\n let x = int(true)\n}\n", "can't convert bool to int"),
        ("main() {\n let x = int(1, 2)\n}\n", "`int(...)` takes exactly one value"),
        ("f(x: int) {}\nmain() {\n f(1.5)\n}\n", "expected int, found float"),
    ];
    for (src, expected) in cases {
        assert_eq!(errors(src), vec![expected.to_string()], "source:\n{src}");
    }
}
