//! Interface declarations and what implements them.

use kestrel_check::{check, implements, Tables, Ty};
use kestrel_syntax::parse;

fn run(src: &str) -> (Option<Tables>, Vec<String>) {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check(&p);
    (t, d.into_iter().map(|d| d.message).collect())
}

const SHAPES: &str = "interface Shape {
    pure area(self) -> int
    scale(var self, by: int)
}

struct Square: Shape {
    side: int
    pure area(self) -> int { self.side * self.side }
    scale(var self, by: int) { self.side *= by }
}

enum Blob: Shape {
    Big
    Small
    pure area(self) -> int { 1 }
    pure scale(var self, by: int) { }
}

struct Plain {
    x: int
}

main() { }
";

#[test]
fn interfaces_register_their_methods_in_order() {
    let (t, errs) = run(SHAPES);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.expect("tables");
    let shape = t.iface_by_name["Shape"];
    let info = t.iface(shape);
    let names: Vec<&str> = info.methods.iter().map(|&m| t.func(m).name.as_str()).collect();
    assert_eq!(names, ["Shape.area", "Shape.scale"]);
    assert!(info.methods.iter().all(|&m| t.func(m).is_abstract));
    // The interface's own `self` is typed as the interface.
    assert_eq!(t.func(info.methods[0]).params[0].ty, Ty::Iface(shape));
}

#[test]
fn implements_follows_the_declared_lists() {
    let (t, errs) = run(SHAPES);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.expect("tables");
    let shape = t.iface_by_name["Shape"];
    assert!(implements(&t, Ty::Struct(t.struct_by_name["Square"]), shape));
    assert!(implements(&t, Ty::Enum(t.enum_by_name["Blob"]), shape));
    assert!(!implements(&t, Ty::Struct(t.struct_by_name["Plain"]), shape));
    assert!(!implements(&t, Ty::Int, shape));
}

#[test]
fn abstract_methods_are_numbered_where_the_interface_appears() {
    // Functions and methods keep their ids; an interface's methods are numbered where it appears.
    let (t, _) = run("f() -> int { 1 }\ninterface I { g(self) }\nh() -> int { 2 }\nmain() { }\n");
    let t = t.expect("tables");
    let name = |i: usize| t.fns[i].name.as_str();
    assert_eq!((name(0), name(1), name(2), name(3)), ("f", "I.g", "h", "main"));
}

#[test]
fn an_interface_type_can_be_named_and_only_implementors_convert() {
    let (_, errs) = run("interface I { g(self) }\nmain() {\n    let x: I = 1\n}\n");
    assert_eq!(errs, vec!["expected I, found int"]);
}

#[test]
fn conformance_messages() {
    let (_, errs) = run("interface I { pure f(self, x: int) -> int }\nstruct A: I {\n    pure f(self, x: float) -> int { 1 }\n}\nmain() { }\n");
    assert_eq!(errs, vec!["`A.f` doesn't match `I.f`: it should be `pure f(self, x: int) -> int`"]);
    let (_, errs) = run("interface I { f(self) }\nstruct A: I {\n    pure f(self) { }\n}\nmain() { }\n");
    assert!(errs.is_empty(), "{errs:?}");
}

const HANDLERS: &str = "interface Handler[E] {
    pure handle(self, e: E) -> int
}

struct A: Handler[int] {
    pure handle(self, e: int) -> int { e }
}

struct B: Handler[string] {
    pure handle(self, e: string) -> int { e.len }
}

struct C: Handler[int] {
    pure handle(self, e: int) -> int { e + 1 }
}

total(hs: list[Handler[int]]) -> int { hs.len }

main() { }
";

#[test]
fn an_interface_instance_is_one_id_per_types_with_its_own_methods() {
    let (t, errs) = run(HANDLERS);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.expect("tables");
    let (a, b, c) = (t.struct_by_name["A"], t.struct_by_name["B"], t.struct_by_name["C"]);
    let (ints, strings) = (t.strukt(a).ifaces[0], t.strukt(b).ifaces[0]);
    // `Handler[int]` written twice (and in a type) is one interface; `Handler[string]` another.
    assert_eq!(t.strukt(c).ifaces, vec![ints]);
    assert_ne!(ints, strings);
    assert_eq!(t.ty_name(Ty::Iface(ints)), "Handler[int]");
    let def = t.iface_by_name["Handler"];
    assert_eq!(t.iface(ints).instance.map(|i| i.def), Some(def));
    // Each instance has its own abstract methods, with its types and `self` the instance.
    let (mi, ms) = (t.iface(ints).methods[0], t.iface(strings).methods[0]);
    assert_ne!(mi, ms);
    assert_ne!(mi, t.iface(def).methods[0]);
    assert_eq!(t.func(mi).name, "Handler[int].handle");
    assert_eq!((t.func(mi).params[0].ty, t.func(mi).params[1].ty), (Ty::Iface(ints), Ty::Int));
    assert_eq!(t.func(ms).params[1].ty, Ty::Str);
    assert!(t.func(mi).is_abstract && t.func(mi).is_pure);
    assert!(implements(&t, Ty::Struct(c), ints));
    assert!(!implements(&t, Ty::Struct(b), ints));
}

#[test]
fn an_instance_conforms_with_its_own_types() {
    let src = "interface Handler[E] { pure handle(self, e: E) -> int }\nstruct A: Handler[float] {\n    pure handle(self, e: int) -> int { e }\n}\nstruct B: Handler[int] {\n    x: int\n}\nmain() { }\n";
    let (_, errs) = run(src);
    assert_eq!(
        errs,
        vec![
            "`A.handle` doesn't match `Handler[float].handle`: it should be `pure handle(self, e: float) -> int`",
            "`B` says it is a `Handler[int]` but has no method `handle`; add `pure handle(self, e: int) -> int`",
        ]
    );
}
