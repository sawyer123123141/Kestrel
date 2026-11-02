//! Parsing interface declarations and the interface lists of structs and enums.

use kestrel_syntax::ast::*;

mod common;
use common::*;

fn interface(p: &Program, i: usize) -> &InterfaceDecl {
    match &p.items[i] {
        Item::Interface(d) => d,
        other => panic!("expected an interface, got {other:?}"),
    }
}

#[test]
fn an_interface_lists_method_signatures() {
    let p = ok("interface I { pure f(self) -> int \n g(var self, x: int) }\nmain() {}\n");
    let d = interface(&p, 0);
    assert_eq!(d.name, "I");
    assert!(!d.is_pub);
    assert_eq!(d.methods.len(), 2);
    let (f, g) = (&d.methods[0], &d.methods[1]);
    assert_eq!((f.name.as_str(), f.is_pure), ("f", true));
    assert!(f.params[0].is_self && !f.params[0].is_var);
    assert_eq!(f.ret.as_ref().unwrap().to_sexpr(), "int");
    assert_eq!((g.name.as_str(), g.is_pure), ("g", false));
    assert!(g.params[0].is_self && g.params[0].is_var);
    assert_eq!(g.params[1].bind.name, "x");
    assert!(g.ret.is_none());
    // The `self` parameter's type is the interface.
    assert_eq!(f.params[0].ty.to_sexpr(), "I");
}

#[test]
fn an_interface_can_be_pub_and_span_lines() {
    let p = ok("pub interface Damageable {\n    take_damage(var self, amount: int)\n    pure hp(self) -> int\n}\nmain() {}\n");
    let d = interface(&p, 0);
    assert!(d.is_pub);
    assert_eq!(d.methods.len(), 2);
}

#[test]
fn structs_and_enums_list_their_interfaces() {
    let p = ok("struct G: A, B {\n    x: int\n}\nenum E: A {\n    X\n}\nstruct H {\n    y: int\n}\nmain() {}\n");
    let Item::Struct(g) = &p.items[0] else { panic!("a struct") };
    let names: Vec<&str> = g.ifaces.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["A", "B"]);
    let Item::Enum(e) = &p.items[1] else { panic!("an enum") };
    assert_eq!(e.ifaces.len(), 1);
    assert_eq!(e.ifaces[0].name, "A");
    let Item::Struct(h) = &p.items[2] else { panic!("a struct") };
    assert!(h.ifaces.is_empty());
}

#[test]
fn interfaces_take_placeholders_and_lists_and_rules_take_types() {
    let src = "interface Handler[E, F: equatable] {
    pure handle(self, e: E) -> F
}
struct B: Handler[list[int], string], Named {
    x: int
}
f[H: Handler[I, int] + Named, I](h: H) {}
main() {}
";
    let p = ok(src);
    let i = interface(&p, 0);
    assert_eq!(kestrel_syntax::type_params_sexpr(&i.type_params), "[E, F: equatable]");
    let Item::Struct(b) = &p.items[1] else { panic!("a struct") };
    let refs: Vec<String> = b.ifaces.iter().map(IfaceRef::to_sexpr).collect();
    assert_eq!(refs, ["Handler[list[int], string]", "Named"]);
    assert_eq!(&src[b.ifaces[0].span.start as usize..b.ifaces[0].span.end as usize], "Handler");
    let Item::Fn(f) = &p.items[2] else { panic!("a function") };
    assert_eq!(kestrel_syntax::type_params_sexpr(&f.type_params), "[H: Handler[I, int] + Named, I]");
}

#[test]
fn interface_syntax_errors() {
    assert_eq!(errs("interface I {\n    x: int\n}\nmain() {}\n"), vec!["expected `(` for a method, found `:`; interfaces have methods only"]);
    assert_eq!(errs("interface I {\n    f(self) { }\n}\nmain() {}\n"), vec!["interface methods have no body; remove the `{ }`"]);
    assert_eq!(errs("interface I {\n    native f(self)\n}\nmain() {}\n"), vec!["native functions can't be methods"]);
    assert_eq!(errs("struct S: {\n}\nmain() {}\n"), vec!["expected an interface name, found `{`"]);
}
