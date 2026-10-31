//! The call exclusivity rule compares places by their fields: separate fields of one variable may
//! be passed beside each other, one as `var` (a reloadable program's state is one variable, and
//! its `frame` steps one part of it with another), while a place and anything inside it, two
//! elements of one list, two lanes of one `vec4`, and anything beside a place holding an `Arena`
//! still clash.

use kestrel_check::check_keeping_tables;
use kestrel_syntax::parse;

fn run(src: &str) -> Vec<String> {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    check_keeping_tables(&p, true).1.into_iter().filter(|d| d.is_error()).map(|d| d.message).collect()
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
}
struct G {
    a: P = P()
    b: P = P()
    items: list[P] = []
    v: vec4 = vec4(0, 0, 0, 0)
    w: W
    s: string = \"\"
}
both(var p: P, q: P) {}
two(var p: P, var q: P) {}
lanes(var x: float, var y: float) {}
replace(var w: W, s: string) { w = W() }
";

/// `DECLS` and a `main` with `g`, running `body`.
fn with(body: &str) -> String {
    format!("{DECLS}main() {{\n    var g = G(w: W())\n    {body}\n}}\n")
}

#[test]
fn separate_fields_pass() {
    for body in ["two(var g.a, var g.b)", "both(var g.a, g.b)", "g.a.bump(g.b)", "g.a.take(var g.b)", "two(var g.items[0], var g.a)"] {
        assert_eq!(run(&with(body)), Vec::<String>::new(), "{body}");
    }
}

#[test]
fn overlapping_places_still_clash() {
    let shared = "`g` is passed as `var` and also used in another argument of the same call";
    for body in ["both(var g.a, g.a)", "two(var g.items[0], var g.items[1])", "lanes(var g.v.x, var g.v.y)", "replace(var g.w, g.s)"] {
        assert_eq!(run(&with(body)), [shared], "{body}");
    }
    assert_eq!(run(&with("g.a.bump(g.a)")), ["`g` is changed by this call, so it can't also be passed to it"]);
    assert_eq!(run(&with("g.a.take(var g.a)")), ["`g` is passed as `var` and is also the receiver of this call"]);
}
