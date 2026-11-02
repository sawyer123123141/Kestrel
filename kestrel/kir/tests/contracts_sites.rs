//! `@prove` sites: each annotated function, method and loop is listed in `Module::contracts` with
//! the function that holds its code, a loop's code is one `Stmt::Prove` wherever lowering puts it,
//! and an inlined copy of a contracted function is one too. (Regions in the analysis and the
//! events: `contracts_regions.rs`.)

mod range_common;

use kestrel_kir::visit::{walk, Node};
use kestrel_kir::*;
use kestrel_syntax::ast::Contract;
use kestrel_syntax::{parse, LineIndex};
use range_common::{idx, line, optimized};

/// `src` (which needs no `main`) lowered, before `optimize`, and the source as compiled.
fn lowered(src: &str) -> (Module, String) {
    let src = format!("{src}\nmain() {{}}\n");
    let (p, d) = parse(&src);
    assert!(d.is_empty(), "parse: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = kestrel_check::check(&p);
    assert!(d.is_empty(), "check: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    (lower(&p, &t.unwrap(), &LineIndex::new(&src)), src)
}

fn body<'m>(m: &'m Module, name: &str) -> &'m [Stmt] {
    &m.funcs[idx(m, name)].body
}

/// The ids of the `Prove` blocks anywhere in `stmts`, outermost first.
fn proves(stmts: &[Stmt]) -> Vec<u32> {
    let mut out = Vec::new();
    walk(stmts, &mut |n| {
        if let Node::Stmt(Stmt::Prove { id, .. }) = n {
            out.push(*id);
        }
    });
    out
}

/// The contracts of a site, by name, and the source text at each one's span.
fn named(site: &ContractSite, src: &str) -> Vec<(&'static str, String)> {
    site.contracts.iter().map(|(c, s)| (c.name(), src[s.start as usize..s.end as usize].to_string())).collect()
}

fn the_par_body(m: &Module) -> usize {
    let [b] = m.par_bodies[..] else { panic!("one `par for` body: {:?}", m.par_bodies) };
    b.0 as usize
}

#[test]
fn function_and_method_contracts_are_listed() {
    let (m, src) = lowered(
        "@prove(no_alloc, inline)\npure f(a: int) -> int { a ^ 1 }\n\nstruct P {\n    x: int\n\n    @prove(no_panic)\n    pure get(self) -> int { self.x }\n}",
    );
    assert_eq!(m.contracts.len(), 2, "{:#?}", m.contracts);
    let (f, get) = (&m.contracts[0], &m.contracts[1]);
    assert_eq!(f.target, ProveTarget::Func(FuncIdx(idx(&m, "f") as u32)));
    assert_eq!(named(f, &src), [("no_alloc", "no_alloc".to_string()), ("inline", "inline".to_string())]);
    assert_eq!(get.target, ProveTarget::Func(FuncIdx(idx(&m, "P.get") as u32)));
    assert_eq!(named(get, &src), [("no_panic", "no_panic".to_string())]);
    assert_eq!((&f.required_by, &get.required_by), (&None, &None));
    // Lowering marks no function's own body: the function is the site.
    assert!(m.funcs.iter().all(|f| proves(&f.body).is_empty()));
}

#[test]
fn an_annotated_loop_is_one_prove_block() {
    let (m, _) = lowered("f(xs: list[int]) -> int {\n    var s = 0\n    @prove(no_alloc)\n    for x in xs.filter((e) { e > 0 }) { s = s ^ x }\n    s\n}");
    let [site] = &m.contracts[..] else { panic!("{:#?}", m.contracts) };
    let f = idx(&m, "f");
    let ProveTarget::Loop { func, line: at } = site.target else { panic!("{site:?}") };
    assert_eq!((func.0 as usize, line(at)), (f, 4));
    assert_eq!(site.contracts.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [Contract::NoAlloc]);
    let blocks: Vec<&Stmt> = body(&m, "f").iter().filter(|s| matches!(s, Stmt::Prove { .. })).collect();
    let [Stmt::Prove { id: 0, at: 0, body: inner }] = blocks[..] else { panic!("{:#?}", body(&m, "f")) };
    // Everything the loop lowered to is inside: the `filter` call and the loop over its list.
    let (mut calls, mut loops) = (0, 0);
    walk(inner, &mut |n| match n {
        Node::Expr(Expr { kind: ExprKind::Call(..), .. }) | Node::Stmt(Stmt::Call(..)) => calls += 1,
        Node::Stmt(Stmt::ForRange { .. } | Stmt::While { .. }) => loops += 1,
        _ => {}
    });
    assert!(calls >= 1 && loops == 1, "{inner:#?}");
    // Outside it, no call and no loop.
    let outside: Vec<Stmt> = body(&m, "f").iter().filter(|s| !matches!(s, Stmt::Prove { .. })).cloned().collect();
    walk(&outside, &mut |n| {
        let stray = matches!(n, Node::Expr(Expr { kind: ExprKind::Call(..), .. }) | Node::Stmt(Stmt::Call(..) | Stmt::ForRange { .. } | Stmt::While { .. }));
        assert!(!stray, "{outside:#?}");
    });
}

#[test]
fn a_par_for_and_a_while_are_regions_too() {
    let (m, _) = lowered(
        "f(var xs: list[int], n: int) -> int {\n    var i = 0\n    @prove(no_checks)\n    while i < n { i += 1 }\n    @prove(no_alloc)\n    par for x in xs { x = x + 1 }\n    i\n}",
    );
    let f = idx(&m, "f");
    let lines: Vec<(usize, u32)> = m
        .contracts
        .iter()
        .map(|s| match s.target {
            ProveTarget::Loop { func, line: at } => (func.0 as usize, line(at)),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(lines, [(f, 4), (f, 6)]);
    let blocks: Vec<&Stmt> = body(&m, "f").iter().filter(|s| matches!(s, Stmt::Prove { .. })).collect();
    let [Stmt::Prove { id: 0, body: w, .. }, Stmt::Prove { id: 1, body: p, .. }] = blocks[..] else { panic!("{:#?}", body(&m, "f")) };
    assert!(matches!(&w[..], [Stmt::While { .. }]), "{w:#?}");
    // The `par for`'s header (its list) and the statement that runs the body.
    assert!(matches!(p.last(), Some(Stmt::ParFor { .. })), "{p:#?}");
    assert!(proves(&m.funcs[the_par_body(&m)].body).is_empty());
}

#[test]
fn a_loop_in_a_par_for_body_belongs_to_the_body() {
    let (m, _) = lowered(
        "f(var ps: list[int]) {\n    par for p in ps {\n        var acc = p\n        @prove(no_checks)\n        for i from 0 to 4 { acc = acc * 1000 }\n        p = acc\n    }\n}",
    );
    let [site] = &m.contracts[..] else { panic!("{:#?}", m.contracts) };
    let par = the_par_body(&m);
    let ProveTarget::Loop { func, line: at } = site.target else { panic!("{site:?}") };
    assert_eq!((func.0 as usize, line(at)), (par, 5), "the site is in `{}`", m.funcs[func.0 as usize].name);
    assert_eq!(proves(&m.funcs[par].body), [0]);
    assert!(proves(body(&m, "f")).is_empty());
    // Inlining copies statements but moves none: the site keeps its function.
    let m = optimized("f(var ps: list[int]) {\n    par for p in ps {\n        var acc = p\n        @prove(no_checks)\n        for i from 0 to 4 { acc = acc * 1000 }\n        p = acc\n    }\n}");
    let ProveTarget::Loop { func, .. } = m.contracts[0].target else { panic!() };
    assert_eq!(proves(&m.funcs[func.0 as usize].body), [0]);
}

#[test]
fn a_loop_in_a_closure_belongs_to_the_closure() {
    let (m, _) = lowered(
        "f(n: int) -> int {\n    let g: (int) -> int = (k) {\n        var acc = 0\n        @prove(no_checks)\n        for i from 0 to k { acc = acc ^ i }\n        acc\n    }\n    g(n)\n}",
    );
    let [site] = &m.contracts[..] else { panic!("{:#?}", m.contracts) };
    let ProveTarget::Loop { func, line: at } = site.target else { panic!("{site:?}") };
    let code = &m.funcs[func.0 as usize];
    assert!(code.name.starts_with("__kestrel_closure_"), "the site is in `{}`", code.name);
    assert_eq!(line(at), 5);
    assert_eq!(proves(&code.body), [0]);
    assert!(proves(body(&m, "f")).is_empty());
}

const GET: &str = "@prove(no_checks)
pure get(xs: [int], i: int) -> int {
    @assume(i >= 0 and i < xs.len)
    xs[i]
}

pure step(xs: [int], k: int) -> int {
    get(xs, k + 1) + 1
}";

#[test]
fn an_inlined_contracted_function_is_a_prove_block() {
    let m = optimized(GET);
    let site = m.contracts.iter().position(|s| s.target == ProveTarget::Func(FuncIdx(idx(&m, "get") as u32))).expect("`get`'s site") as u32;
    assert!(proves(body(&m, "get")).is_empty(), "the own copy is the function itself");
    // In `step`: `Seq([Let(get.i, k + 1), Prove { site, line 8, [Assume { .., [.., Let(t, ..)] }] }], Move(t))`:
    // `get`'s value is already a temporary (made inside its assumption), read after the block as it
    // is, with no second temporary.
    let mut found = Vec::new();
    walk(body(&m, "step"), &mut |n| {
        if let Node::Expr(Expr { kind: ExprKind::Seq(stmts, value), .. }) = n {
            found.push((stmts.clone(), (**value).clone()));
        }
    });
    let [(stmts, value)] = &found[..] else { panic!("{:#?}", body(&m, "step")) };
    let [args @ .., Stmt::Prove { id, at, body: inner }] = &stmts[..] else { panic!("{stmts:#?}") };
    assert_eq!((*id, line(*at)), (site, 8));
    // The argument is the caller's code, outside the block.
    let step = &m.funcs[idx(&m, "step")];
    assert!(matches!(args, [Stmt::Let(l, _)] if step.locals[l.0 as usize].name == "get.i"), "{args:#?}");
    let [Stmt::Assume { body: assumed, .. }] = &inner[..] else { panic!("{inner:#?}") };
    let Some(Stmt::Let(t, _)) = assumed.last() else { panic!("{assumed:#?}") };
    assert!(matches!(&value.kind, ExprKind::Load(p) | ExprKind::Move(p) if p.root == *t && p.path.is_empty()), "{value:#?}");
}

#[test]
fn an_interface_contract_binds_each_implementation() {
    let (m, src) = lowered(
        "interface Shape {\n    @prove(no_alloc)\n    area(self) -> float\n}\n\nstruct Goblin: Shape {\n    r: float\n    area(self) -> float { self.r * 2.0 }\n}\n\nstruct Box: Shape {\n    w: float\n    area(self) -> float { self.w * self.w }\n}\n\nmeasure(g: Goblin) -> float {\n    let s: Shape = g\n    s.area()\n}",
    );
    // (target, required by, each contract and where its span starts)
    type Seen = (ProveTarget, Option<String>, Vec<(&'static str, usize)>);
    let found: Vec<Seen> = m
        .contracts
        .iter()
        .map(|s| (s.target, s.required_by.clone(), s.contracts.iter().map(|(c, sp)| (c.name(), sp.start as usize)).collect()))
        .collect();
    let at = |text: &str| src.find(text).unwrap_or_else(|| panic!("`{text}`"));
    let site = |name: &str, text: &str| {
        let target = ProveTarget::Func(FuncIdx(idx(&m, name) as u32));
        (target, Some("`Shape.area`".to_string()), vec![("no_alloc", at(text))])
    };
    // Both, though only `Goblin` is ever made a `Shape`: the spans are each method's name.
    assert_eq!(found, [site("Goblin.area", "area(self) -> float { self.r"), site("Box.area", "area(self) -> float { self.w")]);
}

#[test]
fn each_instance_of_a_generic_interface_has_its_own_sites() {
    // `Handler[int].handle` and `Handler[string].handle` are two methods: a site made from one
    // names only its own implementations, through its own stub (decision 16).
    let src = "interface Handler[E] {\n    @prove(no_alloc)\n    handle(self, e: E) -> int\n}\nstruct A: Handler[int] {\n    handle(self, e: int) -> int { e }\n}\nstruct B: Handler[string] {\n    handle(self, e: string) -> int { e.len }\n}\nstruct C: Handler[int] {\n    handle(self, e: int) -> int { e + 1 }\n}";
    let (m, _) = lowered(src);
    let by = |imp: &str| {
        let target = ProveTarget::Func(FuncIdx(idx(&m, imp) as u32));
        let site = m.contracts.iter().find(|s| s.target == target).unwrap_or_else(|| panic!("{imp}: {:#?}", m.contracts));
        (site.required_by.clone().unwrap_or_default(), m.funcs[site.through.expect("an interface site").0 as usize].name.clone())
    };
    let int = ("`Handler[int].handle`".to_string(), "Handler[int].handle".to_string());
    assert_eq!(by("A.handle"), int);
    assert_eq!(by("C.handle"), int);
    assert_eq!(by("B.handle"), ("`Handler[string].handle`".to_string(), "Handler[string].handle".to_string()));
    assert_eq!(m.contracts.len(), 3, "{:#?}", m.contracts);
}
