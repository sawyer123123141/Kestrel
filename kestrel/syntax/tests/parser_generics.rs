//! Placeholder lists after a struct, enum or function name, and the R8 refusals.

use kestrel_syntax::ast::*;
use kestrel_syntax::type_params_sexpr;

mod common;
use common::{errs, fn_decl, ok};

const DECLS: &str = "struct Stack[T]: Named {\n    items: list[T] = []\n    push(var self, v: T) { self.items.add(v) }\n}\nenum Result[T, E] {\n    Ok { value: T }\n    Err { error: E }\n}\npub pure best[T: Damageable + Named, K: key](xs: list[T], k: K) -> T? { none }\nplain(x: int) -> int { x }\nmain() {}\n";

#[test]
fn placeholder_lists_and_rules() {
    let p = ok(DECLS);
    let Item::Struct(s) = &p.items[0] else { panic!("a struct") };
    assert_eq!(type_params_sexpr(&s.type_params), "[T]");
    assert_eq!(s.ifaces.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Named"]);
    assert!(s.methods[0].type_params.is_empty());
    let Item::Enum(e) = &p.items[1] else { panic!("an enum") };
    assert_eq!(type_params_sexpr(&e.type_params), "[T, E]");
    let best = fn_decl(&p, 2);
    assert_eq!(type_params_sexpr(&best.type_params), "[T: Damageable + Named, K: key]");
    assert!(best.is_pub && best.is_pure);
    assert_eq!(best.params[0].ty.to_sexpr(), "list[T]");
    assert!(fn_decl(&p, 3).type_params.is_empty());
}

#[test]
fn rule_words_stay_names_elsewhere() {
    ok("main() {\n    let key = 3\n    let ordered = key + 1\n    print(ordered)\n}\n");
}

#[test]
fn methods_and_natives_have_no_placeholders_but_interfaces_do() {
    let m = "methods can't have placeholders of their own yet; use a generic function that takes the struct instead";
    assert!(errs("struct S[T] {\n    x: T\n    conv[U](self) -> int { 1 }\n}\nmain() {}\n").contains(&m.to_string()));
    assert!(errs("enum E[T] {\n    A\n    wrap[U](self) -> int { 1 }\n}\nmain() {}\n").contains(&m.to_string()));
    ok("interface N[T] {\n    pure name(self) -> T\n}\nmain() {}\n");
    let im = "interface methods can't have placeholders of their own; put the placeholder on the interface, like `interface Handler[E]`";
    assert_eq!(errs("interface N[T] {\n    pure name[U](self) -> U\n}\nmain() {}\n"), [im]);
    assert_eq!(errs("interface N<T> {\n    pure name(self) -> T\n}\nmain() {}\n"), ["Kestrel writes generic types with square brackets: `N[T]`"]);
    assert_eq!(errs("native peek[T](x: T) -> int\nmain() {}\n"), ["native functions can't have placeholders"]);
    let (p, _) = kestrel_syntax::parse_at("native peek[T](x: T) -> int\nmain() {}\n", 0, 0, 0);
    assert_eq!(p.items.len(), 1, "the native is dropped (P35)");
    ok("main[T]() {}\n"); // the checker refuses it (Task 2b)
}
