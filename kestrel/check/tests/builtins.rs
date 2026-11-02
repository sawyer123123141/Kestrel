use kestrel_check::{check, Builtin, Res, Tables, Ty, VEC2, VEC3};
use kestrel_syntax::parse;

fn run(src: &str) -> (Option<Tables>, Vec<String>) {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check(&p);
    (t, d.into_iter().map(|d| d.message).collect())
}

fn local_ty(t: &Tables, name: &str) -> Ty {
    t.locals.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no local {name}")).ty
}

#[test]
fn float_functions_take_the_common_float_type() {
    let src = "main() {\n let w: f64 = 2\n let a = sqrt(16)\n let b = pow(2, w)\n let c = atan2(1.5, 2)\n let d = floor(w)\n let e = PI\n let f: f64 = TAU\n let g = sin(u8(3))\n let h = PI * w\n print(a, b, c, d, e, f, g, h)\n}\n";
    let (t, errs) = run(src);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    let expected = [
        ("a", Ty::Float),
        ("b", Ty::F64),
        ("c", Ty::Float),
        ("d", Ty::F64),
        ("e", Ty::Float),
        ("f", Ty::F64),
        ("g", Ty::Float),
        ("h", Ty::F64),
    ];
    for (name, ty) in expected {
        assert_eq!(local_ty(&t, name), ty, "type of `{name}`");
    }
    let builtins: Vec<Builtin> = t.res.values().filter_map(|r| if let Res::Builtin(b) = r { Some(*b) } else { None }).collect();
    for b in [Builtin::Sqrt, Builtin::Pow, Builtin::Atan2, Builtin::Floor, Builtin::Sin, Builtin::Pi, Builtin::Tau] {
        assert!(builtins.contains(&b), "{b:?} should be resolved");
    }
}

#[test]
fn generic_functions_and_vector_methods_have_the_common_type() {
    let src = "main() {\n let w: f64 = 2\n let small: u8 = 7\n let a = abs(-3)\n let b = sign(small)\n let c = min(3, 1.5)\n let d = max(2, w)\n let e = clamp(15, 0, w)\n let f = lerp(0, 10, 0.5)\n let g = lerp(w, 4, 0.5)\n let h = lerp(vec2(0, 0), vec2(1, 1), 1)\n let i = vec2(3, 4).length()\n let j = vec2(3, 4).normalized()\n let k = vec3(1, 2, 3).dot(vec3(1, 1, 1))\n let l = vec3(1, 2, 3).normalized().distance_to(vec3(0, 0, 0))\n let m = clamp(small, 1, 5)\n let n = abs(i64(-4))\n print(a, b, c, d, e, f, g, h, i, j, k, l, m, n)\n}\n";
    let (t, errs) = run(src);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    let expected = [
        ("a", Ty::Int),
        ("b", Ty::U8),
        ("c", Ty::Float),
        ("d", Ty::F64),
        ("e", Ty::F64),
        ("f", Ty::Float),
        ("g", Ty::F64),
        ("h", Ty::Struct(VEC2)),
        ("i", Ty::Float),
        ("j", Ty::Struct(VEC2)),
        ("k", Ty::Float),
        ("l", Ty::Float),
        ("m", Ty::U8),
        ("n", Ty::I64),
    ];
    for (name, ty) in expected {
        assert_eq!(local_ty(&t, name), ty, "type of `{name}`");
    }
    let builtins: Vec<Builtin> = t.res.values().filter_map(|r| if let Res::Builtin(b) = r { Some(*b) } else { None }).collect();
    for b in [
        Builtin::Abs,
        Builtin::Sign,
        Builtin::Min,
        Builtin::Max,
        Builtin::Clamp,
        Builtin::Lerp,
        Builtin::Length,
        Builtin::Normalized,
        Builtin::Dot,
        Builtin::DistanceTo,
    ] {
        assert!(builtins.contains(&b), "{b:?} should be resolved");
    }
}

#[test]
fn cross_gives_a_vec3() {
    let (t, errs) = run("main() {\n let c = vec3(1, 0, 0).cross(vec3(0, 1, 0))\n print(c)\n}\n");
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    assert_eq!(local_ty(&t, "c"), Ty::Struct(VEC3));
    assert!(t.res.values().any(|r| matches!(r, Res::Builtin(Builtin::Cross))), "Cross should be resolved");
}

#[test]
fn lerp_converts_an_integer_t_and_integer_ends_to_float() {
    // `lerp(vec2, vec2, 1)`: `t` goes to float. `lerp(0, 10, 1)`: all three go to float.
    let (t, errs) = run("main() {\n print(lerp(vec2(0, 0), vec2(1, 1), 1), lerp(0, 10, 1))\n}\n");
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    assert_eq!(t.coerce.values().filter(|&&ty| ty == Ty::Float).count(), 4, "{:?}", t.coerce);
    assert!(!t.coerce.values().any(|&ty| ty == Ty::F64));
}

#[test]
fn the_other_argument_meets_an_f64_one() {
    // `pow(2, w)`: the integer first goes to float, then both meet at f64.
    let (t, errs) = run("main() {\n let w: f64 = 2\n print(pow(2, w))\n}\n");
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    assert!(t.coerce.values().any(|&ty| ty == Ty::F64), "the literal 2 should end up as f64");
    assert!(!t.coerce.values().any(|&ty| ty == Ty::Float), "no stop at float on the way");
}

#[test]
fn builtins_are_pure_and_constants_may_use_them() {
    let src = "let ROOT2 = sqrt(2.0)\nlet HALF_PI = PI / 2\npure hyp(a: float, b: float) -> float {\n sqrt(a * a + b * b)\n}\nmain() {\n print(ROOT2, HALF_PI, hyp(3, 4))\n}\n";
    let (t, errs) = run(src);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    assert_eq!(t.consts[0].ty, Ty::Float);
    assert_eq!(t.consts[1].ty, Ty::Float);
}

#[test]
fn random_numbers_have_their_types_and_count_as_io() {
    let src = "roll() -> int {\n randi_range(1, 6)\n}\nseed() {\n rand_seed(1)\n}\nquiet() -> int {\n 1\n}\nmain() {\n let small: u8 = 7\n let w: f64 = 2\n let a = randf()\n let b = randf_range(1, 2)\n let c = randf_range(1, w)\n let d = randi_range(1, 6)\n let e = randi_range(small, 9)\n let f = randi_range(1, i64(9))\n rand_seed(3)\n rand_seed(small)\n randf()\n randi_range(1, 2)\n print(a, b, c, d, e, f, quiet())\n}\n";
    let (t, errs) = run(src);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    let expected = [("a", Ty::Float), ("b", Ty::Float), ("c", Ty::F64), ("d", Ty::Int), ("e", Ty::U8), ("f", Ty::I64)];
    for (name, ty) in expected {
        assert_eq!(local_ty(&t, name), ty, "type of `{name}`");
    }
    let io = |name: &str| t.fns.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}")).io;
    assert!(io("roll") && io("seed"), "random numbers make a function do I/O, like `print`");
    assert!(!io("quiet"));
}

#[test]
fn builtin_tables_agree() {
    for &name in Builtin::GLOBAL_NAMES {
        let b = Builtin::global(name).unwrap_or_else(|| panic!("`{name}` is listed but not found"));
        assert_eq!(b.name(), name);
        assert!(Builtin::vector_method(name).is_none(), "`{name}` is both a global and a vector method");
    }
    for name in ["length", "normalized", "dot", "distance_to", "cross"] {
        let b = Builtin::vector_method(name).unwrap();
        assert_eq!(b.name(), name);
        assert!(Builtin::global(name).is_none(), "vector method `{name}` must not be a global name");
    }
    assert_eq!(Builtin::global("PI"), Some(Builtin::Pi));
    assert!(Builtin::global("sqrt").unwrap().is_pure());
    assert!(!Builtin::global("randf").unwrap().is_pure());
    assert!(!Builtin::global("rand_seed").unwrap().is_pure());
    for name in ["to_bytes", "from_bytes", "first_difference"] {
        assert!(Builtin::global(name).unwrap().is_pure(), "`{name}` is pure");
    }
    assert_eq!(Builtin::global("first_difference").unwrap().arity(), 2);
    // `fail` can stop a `pure` function; `exit` ends the program, an effect.
    assert!(Builtin::global("fail").unwrap().is_pure() && Builtin::global("fail").unwrap().never_returns());
    assert!(!Builtin::global("exit").unwrap().is_pure() && Builtin::global("exit").unwrap().never_returns());
    assert!(Builtin::global("main").is_none());
}

#[test]
fn builtin_errors() {
    let cases: &[(&str, &str)] = &[
        ("main() {\n print(sqrt(1, 2))\n}\n", "`sqrt` takes one value"),
        ("main() {\n print(sqrt())\n}\n", "`sqrt` takes one value"),
        ("main() {\n print(atan2(1))\n}\n", "`atan2` takes two values"),
        ("main() {\n print(pow(1, 2, 3))\n}\n", "`pow` takes two values"),
        ("main() {\n print(sqrt(\"x\"))\n}\n", "`sqrt` needs a number here, found string"),
        ("main() {\n print(sqrt(true))\n}\n", "`sqrt` needs a number here, found bool"),
        ("main() {\n print(atan2(1, vec2(1, 2)))\n}\n", "`atan2` needs a number here, found vec2"),
        ("main() {\n print(sqrt(x: 4))\n}\n", "built-in functions take plain values, not labels or `var`"),
        ("main() {\n var x = 4.0\n print(sqrt(var x))\n}\n", "built-in functions take plain values, not labels or `var`"),
        ("main() {\n sqrt(4)\n}\n", "the result of `sqrt` is unused, so this line does nothing"),
        ("main() {\n to_bytes(4)\n}\n", "the result of `to_bytes` is unused, so this line does nothing"),
        ("main() {\n let b = to_bytes(1)\n print(from_bytes(b))\n}\n", "`from_bytes` needs the type to read, like `from_bytes[Game](bytes)`"),
        ("main() {\n print(first_difference(1, true))\n}\n", "`first_difference` compares two values of one type; these are `int` and `bool`"),
        ("nothing() {}\nmain() {\n print(first_difference(1, nothing()))\n}\n", "`first_difference` needs a value, and this gives nothing"),
        ("nothing() {}\nmain() {\n print(to_bytes(nothing()))\n}\n","`to_bytes` needs a value, and this gives nothing"),
        ("main() {\n let PI = 3\n}\n", "`PI` is a built-in name; pick another"),
        ("main() {\n let min = 3\n}\n", "`min` is a built-in name; pick another"),
        ("round(x: int) -> int { x }\nmain() {}\n", "`round` is a built-in name; pick another"),
        ("log(x: int) {\n print(x)\n}\nmain() {}\n", "`log` is a built-in name; pick another"),
        ("main() {\n for round from 0 to 3 {\n print(round)\n }\n}\n", "`round` is a built-in name; pick another"),
        ("fail(x: int) {\n print(x)\n}\nmain() {}\n", "`fail` is a built-in name; pick another"),
        ("main() {\n let exit = 3\n}\n", "`exit` is a built-in name; pick another"),
        ("main() {\n PI = 3\n}\n", "can't change constant `PI`"),
        ("main() {\n let f = sqrt\n}\n", "`sqrt` is built in; wrap it: `(x: float) { sqrt(x) }`"),
        ("main() {\n let x = PI(1)\n}\n", "`PI` is a constant; write it without `(...)`"),
        ("let X = randf()\nmain() {}\n", "constants can only use literals, operators, other constants, struct and enum values, and pure built-ins like `sqrt`"),
        ("main() {\n print(abs(\"x\"))\n}\n", "`abs` needs a number here, found string"),
        ("main() {\n print(abs(1, 2))\n}\n", "`abs` takes one value"),
        ("main() {\n print(min(1))\n}\n", "`min` takes two values"),
        ("main() {\n print(max(1, true))\n}\n", "`max` needs a number here, found bool"),
        ("main() {\n print(clamp(1, 2))\n}\n", "`clamp` takes three values"),
        // Like `+`, a literal takes the type of the value before it.
        ("main() {\n let small: u8 = 7\n print(max(small, 300))\n}\n", "`300` doesn't fit in u8"),
        ("main() {\n print(sign(vec2(1, 2)))\n}\n", "`sign` needs a number here, found vec2"),
        ("main() {\n print(lerp(1, 2))\n}\n", "`lerp` takes three values"),
        // (A number next to a vector fills its lanes: `lerp(1, vec2(1, 2), 0.5)` is fine.)
        ("main() {\n print(lerp(vec3(1), vec2(1, 2), 0.5))\n}\n", "`lerp` needs two numbers or two vectors of the same size, found vec3 and vec2"),
        ("main() {\n print(lerp(vec2(1, 2), true, 0.5))\n}\n", "`lerp` needs two numbers or two vectors of the same size, found vec2 and bool"),
        ("main() {\n print(lerp(ivec2(1, 2), 3, 0.5))\n}\n", "`lerp` needs floats; convert with `vec2(...)` first"),
        ("main() {\n print(lerp(vec2(1, 2), vec3(1, 2, 3), 0.5))\n}\n", "`lerp` needs two numbers or two vectors of the same size, found vec2 and vec3"),
        ("main() {\n print(lerp(\"a\", \"b\", 0.5))\n}\n", "`lerp` needs two numbers or two vectors of the same size, found string and string"),
        ("main() {\n print(lerp(1, 2, vec2(1, 2)))\n}\n", "`lerp` needs a number here, found vec2"),
        ("main() {\n let t: f64 = 0.5\n print(lerp(vec2(1, 2), vec2(3, 4), t))\n}\n", "expected float, found f64"),
        ("main() {\n print(vec2(1, 2).length(1))\n}\n", "`length` takes no values"),
        ("main() {\n print(vec2(1, 2).normalized(1))\n}\n", "`normalized` takes no values"),
        ("main() {\n print(vec2(1, 2).dot())\n}\n", "`dot` takes one value"),
        ("main() {\n print(vec2(1, 2).distance_to(1, 2))\n}\n", "`distance_to` takes one value"),
        ("main() {\n print(vec3(1, 2, 3).dot(vec2(1, 2)))\n}\n", "expected vec3, found vec2"),
        ("main() {\n print(vec2(1, 2).dot(2))\n}\n", "expected vec2, found int"),
        ("main() {\n print(vec2(1, 2).dot(other: vec2(1, 2)))\n}\n", "built-in functions take plain values, not labels or `var`"),
        ("main() {\n vec2(1, 2).length()\n}\n", "the result of `length` is unused, so this line does nothing"),
        ("main() {\n print(vec2(1, 2).foo())\n}\n", "`vec2` has no method `foo`"),
        ("main() {\n print(vec3(1, 2, 3).cross())\n}\n", "`cross` takes one value"),
        ("main() {\n print(vec3(1, 2, 3).cross(vec2(1, 2)))\n}\n", "expected vec3, found vec2"),
        ("main() {\n print(vec2(1, 2).cross(vec2(1, 2)))\n}\n", "`cross` needs a vec3; vec2 has no cross product"),
        ("main() {\n vec3(1, 2, 3).cross(vec3(1, 2, 3))\n}\n", "the result of `cross` is unused, so this line does nothing"),
        (
            "bump(var v: vec2) -> vec2 {\n v = vec2(0, 0)\n v\n}\nmain() {\n var v = vec2(1, 2)\n print(v.dot(bump(var v)))\n}\n",
            "`v` is passed here, but a later argument changes it",
        ),
        (
            "struct P {\n pos: vec2\n}\nshift(var p: P) -> vec2 {\n p.pos = vec2(0, 0)\n vec2(1, 1)\n}\nmain() {\n var p = P(pos: vec2(3, 4))\n print(p.pos.dot(shift(var p)))\n}\n",
            "`p` is passed here, but a later argument changes it",
        ),
        ("main() {\n print(randf(1))\n}\n", "`randf` takes no values"),
        ("main() {\n print(randf_range(1))\n}\n", "`randf_range` takes two values"),
        ("main() {\n print(randf_range(\"a\", 1))\n}\n", "`randf_range` needs a number here, found string"),
        ("main() {\n print(randi_range(1.5, 2))\n}\n", "`randi_range` needs integers, found float"),
        ("main() {\n print(randi_range(1, \"a\"))\n}\n", "`randi_range` needs integers, found string"),
        ("main() {\n print(randi_range(1))\n}\n", "`randi_range` takes two values"),
        ("main() {\n rand_seed(1.5)\n}\n", "`rand_seed` needs an integer, found float"),
        ("main() {\n rand_seed()\n}\n", "`rand_seed` takes one value"),
        ("main() {\n let x = rand_seed(1)\n}\n", "this expression has no value to store"),
        ("main() {\n let x: int = randf()\n}\n", "expected int, found float"),
        (
            "pure roll() -> int {\n randi_range(1, 6)\n}\nmain() {}\n",
            "pure functions can't use random numbers: each call changes the generator",
        ),
        (
            "pure seed() {\n rand_seed(1)\n}\nmain() {}\n",
            "pure functions can't use random numbers: each call changes the generator",
        ),
        (
            "main() {\n var xs = [1.0]\n par for x in xs {\n x = randf()\n }\n}\n",
            "`par for` bodies can't use random numbers: iterations run at the same time",
        ),
    ];
    for (src, expected) in cases {
        assert_eq!(run(src).1, vec![expected.to_string()], "source:\n{src}");
    }
}

#[test]
fn errors_inside_arguments_are_still_found() {
    let (_, errs) = run("main() {\n print(sqrt(nope, 2))\n}\n");
    assert_eq!(errs, vec!["`sqrt` takes one value".to_string(), "unknown name `nope`".to_string()]);
}
