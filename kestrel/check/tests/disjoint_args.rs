//! The call exclusivity rule compares places by their fields: separate fields of one variable may
//! be passed beside each other, one as `var` (a reloadable program's state is one variable, and
//! its `frame` steps one part of it with another), while a place and anything inside it, two
//! elements of one list, two lanes of one `vec4`, and anything beside a place holding an `Arena`
//! still clash. A message names both places.

use kestrel_check::check_keeping_tables;
use kestrel_syntax::parse;

fn run(src: &str) -> Vec<String> {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    check_keeping_tables(&p, true).1.into_iter().filter(|d| d.is_error()).map(|d| d.message).collect()
}

/// The help of the only error `src` has.
fn help(src: &str) -> String {
    let (p, _) = parse(src);
    let errs: Vec<_> = check_keeping_tables(&p, true).1.into_iter().filter(|d| d.is_error()).collect();
    assert_eq!(errs.len(), 1, "{:?}", errs.iter().map(|d| &d.message).collect::<Vec<_>>());
    errs[0].help.clone().unwrap_or_default()
}

const DECLS: &str = "struct P {
    x: int = 0
    xs: list[int] = []
    bump(var self, q: P) { self.x += q.x }
    take(self, var q: P) { q.x = self.x }
}
struct W {
    mem: Arena = Arena()
    s: string = \"\"
    reset(var self, s: string) { self = W() }
}
enum E {
    A { q: P }
    B
}
struct G {
    a: P = P()
    b: P = P()
    items: list[P] = []
    v: vec4 = vec4(0, 0, 0, 0)
    f: float = 0.0
    e: E = E.B
    w: W
    s: string = \"\"
}
both(var p: P, q: P) {}
two(var p: P, var q: P) {}
inner(var p: P, xs: list[int]) {}
whole(var g: G, a: P) {}
scrut(var p: P, e: E) {}
lanes(var x: float, var y: float) {}
replace(var w: W, s: string) { w = W() }
";

/// `DECLS` and a `main` with `g`, running `body`.
fn with(body: &str) -> String {
    format!("{DECLS}main() {{\n    var g = G(w: W())\n    let c = g.f > 1.0\n    {body}\n}}\n")
}

#[test]
fn separate_fields_pass() {
    for body in [
        "two(var g.a, var g.b)",
        "both(var g.a, g.b)",
        "g.a.bump(g.b)",
        "g.a.take(var g.b)",
        "two(var g.items[0], var g.a)",
        "lanes(var g.v.x, var g.f)",
        "both(var g.a, if c { g.b } else { P() })",
        "let cb = both\n    cb(var g.a, g.b)",
        "for p in g.items {\n        two(var p, var g.a)\n    }",
        "match g.e {\n        A x { both(var x.q, g.a) }\n        else {}\n    }",
    ] {
        assert_eq!(run(&with(body)), Vec::<String>::new(), "{body}");
    }
}

#[test]
fn overlapping_places_still_clash() {
    let cases = [
        ("both(var g.a, g.a)", "`g.a` is passed as `var` and is also passed in another argument of the same call"),
        ("two(var g.items[0], var g.items[1])", "`g.items[1]` is passed as `var` and `g.items[0]` may be the same element"),
        ("lanes(var g.v.x, var g.v.y)", "`g.v.y` is passed as `var` and `g.v.x` is a lane of the same vector"),
        ("replace(var g.w, g.s)", "`g.w` holds an `Arena`, so nothing else from `g` can be passed beside it as `var`"),
        ("g.a.bump(g.a)", "`g.a` is changed by this call, so it can't also be passed to it"),
        ("g.a.take(var g.a)", "`g.a` is passed as `var` and is also the receiver of this call"),
        // A place and anything inside it.
        ("inner(var g.a, g.a.xs)", "`g.a` is passed as `var` and `g.a.xs` is inside it"),
        ("whole(var g, g.a)", "`g` is passed as `var` and `g.a` is inside it"),
        // A loop element is an element of its list.
        ("for p in g.items {\n        both(var p, g.items[0])\n    }", "`p` is passed as `var` and `g.items[0]` may be the same element"),
        // A `match` binding is a part of its scrutinee.
        ("match g.e {\n        A x { scrut(var x.q, g.e) }\n        else {}\n    }", "`x.q` is passed as `var` and `g.e` holds it"),
        // Each branch an argument may end in counts.
        ("both(var g.a, if c { g.b } else { g.a })", "`g.a` is passed as `var` and is also passed in another argument of the same call"),
        // A changed receiver that holds an `Arena` frees what its siblings may hold.
        ("g.w.reset(g.s)", "`g.w` holds an `Arena` and is changed by this call, so nothing else from `g` can be passed to it"),
        // A function value's call is checked the same way.
        ("let cb = both\n    cb(var g.a, g.a)", "`g.a` is passed as `var` and is also passed in another argument of the same call"),
    ];
    for (body, msg) in cases {
        assert_eq!(run(&with(body)), [msg], "{body}");
    }
}

#[test]
fn every_clash_says_what_to_do() {
    let cases = [
        ("both(var g.a, g.a)", "copy one into a separate variable first"),
        ("inner(var g.a, g.a.xs)", "pass separate fields, or copy one into a separate variable first"),
        ("lanes(var g.v.x, var g.v.y)", "copy one lane into a separate variable first"),
        ("replace(var g.w, g.s)", "copy what you need into a separate variable first"),
        ("g.a.bump(g.a)", "copy it into a separate variable first"),
        ("g.a.take(var g.a)", "copy what the method needs into a separate variable first"),
    ];
    for (body, want) in cases {
        assert_eq!(help(&with(body)), want, "{body}");
    }
}
