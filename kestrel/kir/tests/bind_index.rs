use kestrel_kir::*;
use kestrel_syntax::{parse, LineIndex};

fn optimized(src: &str) -> Module {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = kestrel_check::check(&p);
    assert!(d.is_empty(), "check: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let mut m = lower(&p, &t.unwrap(), &LineIndex::new(src));
    optimize(&mut m);
    m
}

fn func<'m>(m: &'m Module, name: &str) -> &'m Func {
    m.funcs.iter().find(|f| f.name == name).unwrap()
}

fn local(f: &Func, name: &str) -> LocalIdx {
    LocalIdx(f.locals.iter().position(|l| l.name == name).unwrap() as u32)
}

/// The `Bind`s in `stmts` of an element of `root` (`Bind(e, root[...])`).
fn binds_of(stmts: &[Stmt], root: LocalIdx) -> Vec<(LocalIdx, Place)> {
    let mut out = Vec::new();
    visit::walk(stmts, &mut |n| {
        if let visit::Node::Stmt(Stmt::Bind(l, p)) = n {
            if p.root == root {
                out.push((*l, p.clone()));
            }
        }
    });
    out
}

/// How many places in `stmts` index `root` directly (`root[...]`), the `Bind`s' places included.
fn indexed(stmts: &[Stmt], root: LocalIdx) -> usize {
    let mut n = 0;
    visit::places(stmts, &mut |p, _| {
        if p.root == root && matches!(p.path.first(), Some(Proj::Index { .. })) {
            n += 1;
        }
    });
    n
}

/// How many places in `stmts` are rooted at `root`: (read, written).
fn uses(stmts: &[Stmt], root: LocalIdx) -> (usize, usize) {
    let (mut read, mut written) = (0, 0);
    visit::places(stmts, &mut |p, w| {
        if p.root == root {
            if w {
                written += 1;
            } else {
                read += 1;
            }
        }
    });
    (read, written)
}

fn step_index() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/kernels/step_index.kes")).unwrap()
}

#[test]
fn step_index_binds_parts_i_once_and_keeps_its_fields_in_locals() {
    let m = optimized(&step_index());
    let f = func(&m, "bench");
    let parts = local(f, "parts");
    // The step loop's one `Bind`, and the `for p in parts` loop's.
    let binds = binds_of(&f.body, parts);
    assert_eq!(binds.len(), 2, "{binds:#?}");
    assert_eq!(indexed(&f.body, parts), 2, "{:#?}", f.body);
    // The step's element has its four leaves in locals, loaded once and written back once.
    let e = binds.iter().find(|(l, _)| f.locals[l.0 as usize].name != "p_ref").unwrap().0;
    let name = &f.locals[e.0 as usize].name;
    for leaf in ["pos.x", "pos.y", "vel.x", "vel.y"] {
        assert!(f.locals.iter().any(|l| l.name == format!("{name}.{leaf}")), "{name}.{leaf} in {:#?}", f.locals);
    }
    assert_eq!(uses(&f.body, e), (4, 4), "{:#?}", f.body);
}

const P: &str = "struct P {\n x: float\n y: float\n}\n";

#[test]
fn a_run_of_one_element_binds_it_before_its_first_statement() {
    let src = format!("{P}f(var ps: list[P], i: int) {{\n ps[i].x = 1.0\n ps[i].y = ps[i].x * 2.0\n}}\nmain() {{}}\n");
    let m = optimized(&src);
    let f = func(&m, "f");
    let ps = f.params[0];
    let binds = binds_of(&f.body, ps);
    assert_eq!(binds.len(), 1, "{:#?}", f.body);
    assert!(matches!(f.body[0], Stmt::Bind(..)), "{:#?}", f.body);
    assert!(matches!(&binds[0].1.path[..], [Proj::Index { check: IndexCheck::Checked, .. }]), "{:#?}", binds[0]);
    assert_eq!(indexed(&f.body, ps), 1, "{:#?}", f.body);
}

/// `f`'s body (`ps`, `i`, `j` and a `var k`) binds no element of `ps`, and still indexes it `n`
/// times.
fn binds_nothing(body: &str, n: usize) {
    let src = format!("{P}g(var ps: list[P]) {{\n if ps.len > 3 {{\n  return\n }}\n ps.add(P(x: 0, y: 0))\n}}\nf(var ps: list[P], i: int, j: int) {{\n var k = i\n{body}}}\nmain() {{}}\n");
    let m = optimized(&src);
    let f = func(&m, "f");
    let ps = f.params[0];
    assert!(binds_of(&f.body, ps).is_empty(), "{:#?}", f.body);
    assert_eq!(indexed(&f.body, ps), n, "{:#?}", f.body);
}

#[test]
fn an_index_written_between_two_uses_binds_nothing() {
    binds_nothing(" ps[k].x = 1.0\n k = j\n ps[k].y = 2.0\n", 2);
}

#[test]
fn another_element_beside_the_run_binds_nothing() {
    binds_nothing(" ps[i].x = ps[j].x\n ps[i].y = 2.0\n", 3);
}

#[test]
fn the_list_passed_on_between_two_uses_binds_nothing() {
    binds_nothing(" ps[i].x = 1.0\n g(var ps)\n ps[i].y = 2.0\n", 2);
}

#[test]
fn an_add_to_the_list_between_two_uses_binds_nothing() {
    binds_nothing(" ps[i].x = 1.0\n ps.add(P(x: 3, y: 4))\n ps[i].y = 2.0\n", 2);
}

#[test]
fn a_use_only_inside_an_if_starts_no_run() {
    // A `Bind` before the `if` would check `ps[i]` when `i > 0` is false, which the program doesn't.
    binds_nothing(" if i > 0 {\n  ps[i].x = 1.0\n }\n ps[i].y = 2.0\n", 2);
}

#[test]
fn a_statement_that_can_fail_before_its_first_use_starts_no_run() {
    // `k + 1` can overflow, so the run starts at the next statement.
    let src = format!("{P}f(var ps: list[P], i: int, k: int) {{\n ps[i].x = ps[i].x + float(k + 1)\n ps[i].y = ps[i].x * 2.0\n}}\nmain() {{}}\n");
    let m = optimized(&src);
    let f = func(&m, "f");
    let ps = f.params[0];
    assert_eq!(binds_of(&f.body, ps).len(), 1, "{:#?}", f.body);
    let at = f.body.iter().position(|s| matches!(s, Stmt::Bind(..))).unwrap();
    assert_eq!(indexed(&f.body[..at], ps), 2, "{:#?}", f.body);
}

#[test]
fn a_loop_element_that_only_copies_a_struct_out_stays_in_memory() {
    // `positions[i] = p.pos` copies the whole `vec2`: no field of `p` is held in a local.
    let src = "struct Q {\n pos: vec2\n vel: vec2\n}\nf(ps: list[Q], var out: list[vec2]) {\n for i, p in ps {\n  out[i] = p.pos\n }\n}\nmain() {}\n";
    let m = optimized(src);
    let f = func(&m, "f");
    assert!(!f.locals.iter().any(|l| l.name.starts_with("p_ref.")), "{:#?}", f.locals);
}

#[test]
fn a_loop_element_copied_out_and_also_computed_on_is_still_promoted() {
    let src = "struct Q {\n pos: vec2\n vel: vec2\n}\nf(ps: list[Q], var out: list[vec2]) -> float {\n var t = 0.0\n for i, p in ps {\n  out[i] = p.pos\n  t += p.vel.x\n }\n t\n}\nmain() {}\n";
    let m = optimized(src);
    let f = func(&m, "f");
    assert!(f.locals.iter().any(|l| l.name == "p_ref.vel.x"), "{:#?}", f.locals);
}

#[test]
fn a_list_of_numbers_binds_the_number() {
    let src = "f(var fs: list[float], i: int) {\n fs[i] = fs[i] * 2.0\n fs[i] += 0.25\n}\nmain() {}\n";
    let m = optimized(src);
    let f = func(&m, "f");
    // `+=` reads its index into a local of its own, which holds the same index.
    assert_eq!(binds_of(&f.body, f.params[0]).len(), 1, "{:#?}", f.body);
    assert_eq!(indexed(&f.body, f.params[0]), 1, "{:#?}", f.body);
}

#[test]
fn compound_assignments_to_one_element_are_one_run() {
    let src = format!("{P}f(var ps: list[P], i: int) {{
 ps[i].x += 1.0
 ps[i].y -= ps[i].x
 ps[i].x *= 0.5
}}
main() {{}}
");
    let m = optimized(&src);
    let f = func(&m, "f");
    let ps = f.params[0];
    assert_eq!(binds_of(&f.body, ps).len(), 1, "{:#?}", f.body);
    assert_eq!(indexed(&f.body, ps), 1, "{:#?}", f.body);
    let e = binds_of(&f.body, ps)[0].0;
    // Both fields loaded once and written back once.
    assert_eq!(uses(&f.body, e), (2, 2), "{:#?}", f.body);
}
