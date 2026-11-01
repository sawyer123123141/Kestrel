//! Brackets after a name: parsed both ways (`Brackets`), then settled by what the name is.

use std::time::{Duration, Instant};

use kestrel_syntax::ast::*;
use kestrel_syntax::link::{link, Module};
use kestrel_syntax::{parse, parse_at};

mod common;
use common::{fn_decl, main_body};

/// The one statement of `main` as parsed, before settling.
fn unsettled(src: &str) -> String {
    let (p, d) = parse_at(&format!("main() {{\n{src}\n}}"), 0, 0, 0);
    assert!(d.is_empty(), "unexpected diagnostics: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let s = fn_decl(&p, 0).body.to_sexpr();
    s[1..s.len() - 1].to_string()
}

#[test]
fn brackets_parse_both_ways() {
    assert_eq!(unsettled("best[Enemy](foes)"), "(callv (brackets best [Enemy] Enemy) foes)");
    assert_eq!(unsettled("Result[int, string].Ok(value: 3)"), "(.call (brackets Result [int, string] -) Ok value: 3)");
    assert_eq!(unsettled("best[list[int]]"), "(brackets best [list[int]] -)");
    assert_eq!(unsettled("xs[0]"), "(index xs 0)");
    assert_eq!(unsettled("m[\"k\"]"), "(index m \"k\")");
    assert_eq!(unsettled("xs[-1]"), "(index xs (- 1))");
    assert_eq!(unsettled("handlers[i](3)"), "(callv (brackets handlers [i] i) 3)");
    assert_eq!(unsettled("xs[ys[i]]"), "(brackets xs [ys[i]] (brackets ys [i] i))");
    assert_eq!(unsettled("geom.Stack[int]()"), "(callv (brackets (. geom Stack) [int] int))");
}

const DECLS: &str = "struct Stack[T] {\n    v: T\n}\nenum Result[T, E] {\n    Ok { value: T }\n}\nenum Maybe[T] {\n    No\n}\npure best[T](xs: list[T]) -> int { 1 }\nlet TABLE = [1, 2]\n";

fn settled(body: &str) -> String {
    main_body(&format!("{DECLS}main() {{\n{body}\n}}\n"))
}

#[test]
fn settled_by_what_the_name_is() {
    assert_eq!(settled("best[Enemy](foes)"), "{(make best[Enemy] foes)}");
    assert_eq!(settled("Stack[int]()"), "{(make Stack[int])}");
    assert_eq!(settled("Result[int, string].Ok(value: 3)"), "{(.call (apply Result[int, string]) Ok value: 3)}");
    assert_eq!(settled("Maybe[int].No"), "{(. (apply Maybe[int]) No)}");
    assert_eq!(settled("let f = best[Enemy]"), "{(let f (apply best[Enemy]))}");
    assert_eq!(settled("TABLE[i]"), "{(index TABLE i)}");
    assert_eq!(settled("let handlers: list[(int)] = []\nhandlers[i](3)"), "{(let handlers []); (callv (index handlers i) 3)}");
}

#[test]
fn locals_hide_items_in_every_scope() {
    // A parameter, a loop variable, a match binding and a closure parameter named like an item.
    assert_eq!(settled("let r = (best: list[int]) { best[0] }"), "{(let r (closure (best: list[int]) {(index best 0)}))}");
    assert_eq!(settled("for best in xs {\n    best[i]\n}"), "{(for-in best xs {(index best i)})}");
    let src = "pure f[T](xs: list[T]) -> int { 1 }\ng(f: list[int], i: int) -> int { f[i] }\nmain() {}\n";
    let p = parse(src).0;
    assert_eq!(fn_decl(&p, 1).body.to_sexpr(), "{(index f i)}");
}

fn module(name: &str, src: &str, after: Option<&Program>, base: u32) -> Module {
    let (program, d) = match after {
        None => parse_at(src, 0, 0, 0),
        Some(p) => parse_at(src, base, p.expr_count, p.bind_count),
    };
    assert!(d.is_empty(), "unexpected diagnostics in {name}: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    // `main` uses `geom`, the second module.
    let resolved = program.items.iter().filter_map(|i| match i {
        Item::Use(u) => Some(u.clauses.iter().map(|c| (c.module == "geom").then_some(1))),
        _ => None,
    });
    let resolved = resolved.flatten().collect();
    Module { name: name.to_string(), file: format!("{name}.kes"), program, resolved }
}

/// Links `main_src` (the entry file) with `geom_src`.
fn linked(main_src: &str, geom_src: &str) -> (Program, Vec<String>) {
    let main = module("main", main_src, None, 0);
    let geom = module("geom", geom_src, Some(&main.program), main_src.len() as u32);
    let (program, diags) = link(vec![main, geom], |_| false);
    (program, diags.into_iter().map(|d| d.message).collect())
}

fn find_fn<'a>(p: &'a Program, name: &str) -> &'a FnDecl {
    p.items
        .iter()
        .find_map(|i| match i {
            Item::Fn(f) if f.name == name => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no function {name}"))
}

#[test]
fn a_local_hides_nothing_across_modules() {
    let main_src = "use geom\npure best[T](xs: list[T]) -> int { 1 }\nmain() { print(geom.first()) }\n";
    let geom_src = "pub first() -> int {\n    let best: list[int] = [4, 5]\n    let i = 1\n    best[i]\n}\n";
    let (p, diags) = linked(main_src, geom_src);
    assert!(diags.is_empty(), "{diags:?}");
    assert!(find_fn(&p, "geom.first").body.to_sexpr().ends_with("(index best i)}"));

    let main_src = "use geom\nmain() {\n    let xs: list[int] = [1]\n    print(geom.best[int](xs))\n}\n";
    let geom_src = "pub pure best[T](xs: list[T]) -> int { 1 }\n";
    let (p, diags) = linked(main_src, geom_src);
    assert!(diags.is_empty(), "{diags:?}");
    let body = find_fn(&p, "main").body.to_sexpr();
    assert!(body.contains("(make geom.best[int] xs)"), "{body}");
}

#[test]
fn placeholders_in_bodies_are_left_alone() {
    let main_src = "use geom\nlet M = 1\nmain() { print(geom.pairs([1, 2]).len, M) }\n";
    let geom_src = "let N = 2\npub pure pairs[M](xs: list[M]) -> list[M] {\n    var out: list[M] = list[M]()\n    let keep = (x: M) { x }\n    for x in xs { out.add(keep(x)) }\n    out\n}\npub pure twice[N](x: N) -> list[N] {\n    let ys: list[N] = [x, x]\n    let again = pairs[N](ys)\n    again\n}\n";
    let (p, diags) = linked(main_src, geom_src);
    assert!(diags.is_empty(), "{diags:?}");
    let ty_of = |f: &FnDecl, i: usize| match &f.body.stmts[i] {
        Stmt::Let { ty, .. } => ty.as_ref().map(|t| t.to_sexpr()),
        _ => None,
    };
    let pairs = find_fn(&p, "geom.pairs");
    assert_eq!(pairs.params[0].ty.to_sexpr(), "list[M]");
    assert_eq!(pairs.ret.as_ref().map(|t| t.to_sexpr()).as_deref(), Some("list[M]"));
    assert_eq!(ty_of(pairs, 0).as_deref(), Some("list[M]"));
    let Stmt::Let { value, .. } = &pairs.body.stmts[0] else { panic!("var out") };
    assert_eq!(value.to_sexpr(), "(make list[M])");
    let Stmt::Let { value, .. } = &pairs.body.stmts[1] else { panic!("let keep") };
    assert_eq!(value.to_sexpr(), "(closure (x: M) {x})");
    let twice = find_fn(&p, "geom.twice");
    assert_eq!(twice.params[0].ty.to_sexpr(), "N");
    assert_eq!(ty_of(twice, 0).as_deref(), Some("list[N]"));
    let Stmt::Let { value, .. } = &twice.body.stmts[1] else { panic!("let again") };
    assert_eq!(value.to_sexpr(), "(make geom.pairs[N] ys)");
}

#[test]
fn a_bracket_pile_parses_quickly() {
    let src = "main() {\n    let x = a[b[c[d[e[f[g[h[i[j[k[l[m[n[o[p[q[r[s[t[\n}\n";
    let start = Instant::now();
    let (_, diags) = parse_at(src, 0, 0, 0);
    assert!(!diags.is_empty());
    assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
}

#[test]
fn a_failed_attempt_leaves_nothing() {
    let (p, diags) = parse_at("main() {\n    let a = f[list[int]]\n    let b = xs[0xFF]\n}\n", 0, 0, 0);
    assert!(diags.is_empty(), "{:?}", diags.iter().map(|d| &d.message).collect::<Vec<_>>());
    let mut ints = Vec::new();
    kestrel_syntax::visit::each_expr(&fn_decl(&p, 0).body, &mut |e| {
        if matches!(e.kind, ExprKind::Int(255)) {
            ints.push(e.id);
        }
    });
    assert_eq!(p.hex_literals, ints);
    assert_eq!(ints.len(), 1);
}

#[test]
fn types_after_a_local_are_an_error() {
    let (_, diags) = parse("main() {\n    let xs: list[int] = [1]\n    let w = xs[int, string]\n}\n");
    let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(msgs, ["`[...]` after `xs` indexes it, but `int, string` isn't a value; types go after a generic type or function"]);
}

#[test]
fn types_after_a_field_of_a_value_name_the_field() {
    // The base isn't a path: the message names the field alone, not `_.items`.
    let (_, diags) = parse("main() {\n    let w = ps[0].items[int, string]\n}\n");
    let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(msgs, ["`[...]` after `items` indexes it, but `int, string` isn't a value; types go after a generic type or function"]);
}

#[test]
fn from_bytes_takes_types_in_its_brackets() {
    // Like `list[int]()`: the brackets hold types whatever they look like, with no item named so.
    assert_eq!(settled("from_bytes[Game](b)"), "{(make from_bytes[Game] b)}");
    assert_eq!(settled("from_bytes[list[int]](b)"), "{(make from_bytes[list[int]] b)}");
    assert_eq!(settled("from_bytes[(int) -> int](b)"), "{(make from_bytes[(int) -> int] b)}");
    let (_, diags) = parse("main() {\n    let g = from_bytes[Game]\n}\n");
    let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(msgs, ["expected `(` with the bytes to read, like `from_bytes[Game](bytes)`"]);
}
