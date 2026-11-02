//! Hover on generic code: a call through a `T`'s rule is its interface's method.

use kestrel_driver::analyze;
use kestrel_lsp::resolve::resolve;

#[test]
fn a_call_through_a_rule_shows_the_interface_method() {
    let src = "interface Damageable {\n    pure hp(self) -> int\n}\npure total[T: Damageable](x: T) -> int {\n    x.hp()\n}\nmain() {}\n";
    let a = analyze("main.kes", src, true, &|_| None);
    assert!(a.diags.is_empty(), "{:?}", a.diags);
    let off = src.find("hp()").unwrap() as u32 + 1;
    let s = resolve(&a, off).expect("something at the call");
    assert_eq!(s.code, "pure Damageable.hp(self) -> int");
    // It leads to the method's declaration in the interface.
    assert_eq!(s.def.map(|d| d.start), Some(src.find("hp(self)").unwrap() as u32));
}

#[test]
fn a_generic_with_copies_leads_to_its_declaration() {
    let src = "pure best[T](xs: list[T]) -> T {\n    xs[0]\n}\nmain() {\n    let a: list[int] = [1]\n    let b: list[float] = [2.5]\n    print(best(a))\n    print(best(b))\n}\n";
    let a = analyze("main.kes", src, true, &|_| None);
    assert!(a.diags.is_empty(), "{:?}", a.diags);
    let t = a.tables.as_ref().expect("tables");
    let copies: Vec<&str> = t.fns.iter().filter(|f| f.copy.is_some()).map(|f| f.name.as_str()).collect();
    assert_eq!(copies, ["best[int]", "best[float]"]);
    // The copies share the generic's span; the generic comes first, so its declaration is found.
    let decl = src.find("best").unwrap() as u32;
    for off in [decl + 1, src.rfind("best(b)").unwrap() as u32 + 1] {
        let s = resolve(&a, off).expect("something at the name");
        assert_eq!(s.code, "pure best(xs: list[T]) -> T");
        assert_eq!(s.def.map(|d| d.start), Some(decl));
    }
}

const GEOM: &str = "pub struct Stack[T] {\n    items: list[T] = []\n    push(var self, v: T) { self.items.add(v) }\n}\n\npub struct Item {\n    id: int\n}\n\npub pure best[T](xs: list[T], score: pure (T) -> int) -> T? {\n    let n: T? = none\n    n\n}\n";
const MAIN: &str = "use { Stack, best, Item from geom }\nuse geom\n\nmain() {\n    var s = Stack[int]()\n    s.push(1)\n    let items: list[Item] = [Item(id: 4)]\n    let low = geom.best[Item](items, (i) { 10 - i.id })\n    let top = best(items, (i) { i.id })\n    print(s, low, top)\n}\n";

fn across_files() -> kestrel_driver::Analysis {
    let read = |p: &std::path::Path| (p.file_name()? == "geom.kes").then(|| GEOM.to_string());
    let a = analyze("game/main.kes", MAIN, true, &read);
    assert!(a.diags.is_empty(), "{:?}", a.diags);
    a
}

fn defined_in(a: &kestrel_driver::Analysis, def: kestrel_syntax::Span) -> (usize, String) {
    let i = a.source_at(def.start);
    let s = &a.sources[i];
    (i, s.src[(def.start - s.base) as usize..(def.end - s.base) as usize].to_string())
}

#[test]
fn generics_of_another_module_lead_to_its_file() {
    let a = across_files();
    let at = |needle: &str, nth: usize, into: u32| MAIN.match_indices(needle).nth(nth).unwrap().0 as u32 + into;
    // `Stack` in `Stack[int]()` (hover shows the instance), `best` in `geom.best[Item]` and in the plain call, `Item` in `[Item]`.
    for (off, code, name) in [
        (at("Stack[int]", 0, 1), "struct geom.Stack[int]", "Stack"),
        (at("best[Item]", 0, 1), "pure geom.best(xs: list[T], score: pure (T) -> int) -> T?", "best"),
        (at("best(items", 0, 1), "pure geom.best(xs: list[T], score: pure (T) -> int) -> T?", "best"),
        (at("[Item]", 1, 2), "struct geom.Item {", "Item"),
    ] {
        let s = resolve(&a, off).unwrap_or_else(|| panic!("nothing at {off}"));
        assert!(s.code.starts_with(code), "{}", s.code);
        assert_eq!(defined_in(&a, s.def.expect("a definition")), (1, name.to_string()));
    }
}

#[test]
fn the_type_before_a_variant_leads_to_its_declaration() {
    // `Maybe` in `Maybe[int].Nothing` has no resolution of its own: it is the declaration.
    let src = "enum Maybe[T] {\n    Nothing\n    Just { v: T }\n}\nmain() {\n    let m = Maybe[int].Nothing\n    print(m)\n}\n";
    let a = analyze("main.kes", src, true, &|_| None);
    assert!(a.diags.is_empty(), "{:?}", a.diags);
    let s = resolve(&a, src.find("Maybe[int]").unwrap() as u32 + 1).expect("something at the name");
    assert!(s.code.starts_with("enum Maybe"), "{}", s.code);
    assert_eq!(s.def.map(|d| d.start), Some(src.find("Maybe").unwrap() as u32));
}

#[test]
fn the_built_in_result_names_the_prelude_and_leads_to_it() {
    let src = "load() -> Result[int, string] {\n    3\n}\nmain() {\n    print(load())\n}\n";
    let a = analyze("main.kes", src, true, &|_| None);
    assert!(a.diags.is_empty(), "{:?}", a.diags);
    let s = resolve(&a, src.find("Result").unwrap() as u32 + 1).expect("the type");
    let want = "// built in (<builtin>/prelude.kes): every file sees it without a `use`\nenum Result[T, E] {\n    Ok(value: T)\n    Err(error: E)\n}";
    assert_eq!(s.code, want);
    // Its definition is the prelude's file, as a built-in module's item is in its own.
    let (i, name) = defined_in(&a, s.def.expect("a definition"));
    assert_eq!((a.sources[i].path.as_str(), name.as_str()), ("<builtin>/prelude.kes", "Result"));
}

#[test]
fn hover_on_an_interface_instance_writes_its_types() {
    let src = "interface Handler[E] {\n    pure handle(self, e: E) -> int\n}\nstruct Click: Handler[int] {\n    x: int\n    pure handle(self, e: int) -> int { e }\n}\ntotal(hs: list[Handler[int]]) -> int { hs.len }\nmain() {}\n";
    let a = analyze("main.kes", src, true, &|_| None);
    assert!(a.diags.is_empty(), "{:?}", a.diags);
    let decl = src.find("Handler").unwrap() as u32;
    let instance = "interface Handler[int] {\n    pure Handler[int].handle(self, e: int) -> int\n}";
    // In an interface list and in a type: the instance; on the declaration: its placeholders.
    for (off, code) in [
        (src.find("Handler[int] {").unwrap() as u32 + 1, instance),
        (src.find("list[Handler").unwrap() as u32 + 6, instance),
        (decl + 1, "interface Handler[E] {\n    pure Handler.handle(self, e: E) -> int\n}"),
    ] {
        let s = resolve(&a, off).expect("something at the name");
        assert_eq!(s.code, code);
        assert_eq!(s.def.map(|d| d.start), Some(decl));
    }
}
