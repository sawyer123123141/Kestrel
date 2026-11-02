//! Parsing dotted module names: `use net.packet` and full names in types (`net.packet.Packet`).

use kestrel_syntax::ast::*;

mod common;
use common::*;

fn clauses(p: &Program, i: usize) -> &[UseClause] {
    match &p.items[i] {
        Item::Use(u) => &u.clauses,
        other => panic!("expected a use, got {other:?}"),
    }
}

#[test]
fn a_use_names_a_module_by_dotted_names() {
    let src = "use net.packet\nuse { Packet, make from net.packet\n    ui.draw, geom\n}\nmain() {}\n";
    let p = ok(src);
    let c = &clauses(&p, 0)[0];
    assert_eq!((c.module.as_str(), c.names.len()), ("net.packet", 0));
    // The span covers every name of the module.
    assert_eq!(&src[c.module_span.start as usize..c.module_span.end as usize], "net.packet");
    let cs = clauses(&p, 1);
    let names: Vec<&str> = cs[0].names.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!((cs[0].module.as_str(), names), ("net.packet", vec!["Packet", "make"]));
    assert_eq!(&src[cs[0].module_span.start as usize..cs[0].module_span.end as usize], "net.packet");
    let modules: Vec<&str> = cs[1..].iter().map(|c| c.module.as_str()).collect();
    assert_eq!(modules, ["ui.draw", "geom"]);
    assert_eq!(&src[cs[1].module_span.start as usize..cs[1].module_span.end as usize], "ui.draw");
}

#[test]
fn a_dotted_use_needs_a_name_after_each_dot() {
    assert_eq!(errs("use net..packet\nmain() {}\n"), vec!["expected a module name, found `.`"]);
    assert_eq!(errs("use { a from net. }\nmain() {}\n"), vec!["expected a module name, found `}`"]);
}

#[test]
fn full_names_work_in_every_type_position() {
    let src = "struct Box {
    p: net.packet.Packet
    ps: list[geom.Point]
    o: geom.Point?
    s: geom.Stack[int]
    a: [ui.Thing]
}
f(p: geom.Point) -> net.packet.Packet {
    let q: geom.Stack[geom.Point] = make()
    q
}
main() {}
";
    let p = ok(src);
    let Item::Struct(s) = &p.items[0] else { panic!("a struct") };
    let tys: Vec<String> = s.fields.iter().map(|f| f.ty.to_sexpr()).collect();
    assert_eq!(tys, ["net.packet.Packet", "list[geom.Point]", "geom.Point?", "geom.Stack[int]", "[ui.Thing]"]);
    let f = fn_decl(&p, 1);
    assert_eq!(f.params[0].ty.to_sexpr(), "geom.Point");
    assert_eq!(f.ret.as_ref().map(TypeExpr::to_sexpr).as_deref(), Some("net.packet.Packet"));
    let Stmt::Let { ty: Some(t), .. } = &f.body.stmts[0] else { panic!("a let with a type") };
    assert_eq!(t.to_sexpr(), "geom.Stack[geom.Point]");
    // The span covers the whole name.
    let field = &s.fields[0].ty;
    assert_eq!(&src[field.span.start as usize..field.span.end as usize], "net.packet.Packet");
}

#[test]
fn full_names_work_in_interface_lists_and_rules() {
    let src = "struct Game: rollback.Rollback, Named {\n    x: int\n}\nenum E: net.packet.Shape {\n    A\n}\npure f[G: net.packet.Shape + ordered](g: G) {}\nmain() {}\n";
    let p = ok(src);
    let Item::Struct(s) = &p.items[0] else { panic!("a struct") };
    let names: Vec<&str> = s.ifaces.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["rollback.Rollback", "Named"]);
    let span = s.ifaces[0].span;
    assert_eq!(&src[span.start as usize..span.end as usize], "rollback.Rollback");
    let Item::Enum(e) = &p.items[1] else { panic!("an enum") };
    assert_eq!(e.ifaces[0].name, "net.packet.Shape");
    let rules: Vec<&str> = fn_decl(&p, 2).type_params[0].rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(rules, ["net.packet.Shape", "ordered"]);
}

#[test]
fn full_names_work_in_the_types_of_an_expression() {
    assert_eq!(body("let g = from_bytes[net.packet.Packet](b)"), "{(let g (make from_bytes[net.packet.Packet] b))}");
}
