//! Stores into `var` parameters, checked where the function is called (P38), the whole replacement
//! of a `var` placeholder (P39), and an argument that passes a `var` argument's place (P40).

use kestrel_check::check_keeping_tables;
use kestrel_syntax::{parse, Diagnostic};

fn diags(src: &str) -> Vec<Diagnostic> {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    check_keeping_tables(&p, true).1.into_iter().filter(|d| d.is_error()).collect()
}

fn run(src: &str) -> Vec<String> {
    diags(src).into_iter().map(|d| d.message).collect()
}

fn clean(src: &str) {
    let errs = run(src);
    assert!(errs.is_empty(), "{errs:?}\n{src}");
}

/// `src` with `main` appended.
fn with_main(src: &str, main: &str) -> String {
    format!("{src}main() {{\n{main}}}\n")
}

const NAMES: &str = "struct Names {\n    items: list[string] = []\n    push(var self, v: string) { self.items.add(v) }\n}\n";

#[test]
fn a_store_into_var_self_is_checked_at_the_call() {
    clean(&with_main(NAMES, "    var n = Names()\n    n.push(\"a\")\n    arena {\n        var m = Names()\n        m.push(\"c\")\n    }\n"));
    let errs = run(&with_main(NAMES, "    var n = Names()\n    arena {\n        let t = \"b{1}\"\n        n.push(t)\n    }\n"));
    assert_eq!(errs, ["`Names.push` stores memory in `n` that is freed at the end of the `arena { }` block, but `n` lives longer"]);
    // The store is passed on to `f`'s caller.
    clean(&format!("{NAMES}f(var n: Names) {{ n.push(\"x\") }}\nmain() {{}}\n"));
}

#[test]
fn callers_before_callees() {
    let src = "outer(var xs: list[string]) { inner(var xs) }\ninner(var xs: list[string]) { xs.add(\"{1}\") }\n";
    let errs = run(&with_main(src, "    var ys: list[string] = []\n    arena {\n        outer(var ys)\n    }\n"));
    assert_eq!(errs, ["`outer` stores memory in `ys` that is freed at the end of the `arena { }` block, but `ys` lives longer"]);
}

#[test]
fn recursion_reaches_a_fixpoint() {
    let grow = "grow(var xs: list[string], n: int) {\n    if n > 0 {\n        xs.add(\"{n}\")\n        grow(var xs, n - 1)\n    }\n}\n";
    let errs = run(&with_main(grow, "    var ys: list[string] = []\n    arena {\n        grow(var ys, 2)\n    }\n"));
    assert_eq!(errs, ["`grow` stores memory in `ys` that is freed at the end of the `arena { }` block, but `ys` lives longer"]);
    // A recursion that stores nothing has an empty summary: fine in a block, and as a value.
    let count_down = "count_down(var xs: list[string], n: int) {\n    if n > 0 { count_down(var xs, n - 1) }\n}\n";
    clean(&with_main(count_down, "    var ys: list[string] = []\n    arena {\n        count_down(var ys, 2)\n    }\n    let f = count_down\n"));
    // Mutual recursion, `ping` checked before `pong`.
    let ping = "ping(var names: list[string], n: int) {\n    if n > 0 { pong(var names, n - 1) }\n}\npong(var names: list[string], n: int) {\n    names.add(\"p{n}\")\n    ping(var names, n)\n}\n";
    let errs = run(&with_main(ping, "    var ys: list[string] = []\n    arena {\n        ping(var ys, 3)\n        pong(var ys, 3)\n    }\n"));
    assert_eq!(
        errs,
        [
            "`ping` stores memory in `ys` that is freed at the end of the `arena { }` block, but `ys` lives longer",
            "`pong` stores memory in `ys` that is freed at the end of the `arena { }` block, but `ys` lives longer",
        ]
    );
}

const SHELF: &str = "interface Shelf {\n    put(var self, v: string)\n}\n";

#[test]
fn interfaces_take_every_implementation() {
    let a = "struct A: Shelf {\n    n: int = 0\n    tag: string = \"\"\n    put(var self, v: string) { self.n += 1 }\n}\n";
    let call = "    var a: Shelf = A()\n    arena {\n        a.put(\"x\")\n    }\n";
    clean(&with_main(&format!("{SHELF}{a}"), call));
    let b = "struct B: Shelf {\n    items: list[string] = []\n    put(var self, v: string) { self.items.add(v) }\n}\n";
    let decls = format!("{SHELF}{a}{b}fill_any[T: Shelf](var x: T) {{ x.put(\"a\") }}\n");
    let errs = run(&with_main(&decls, call));
    assert_eq!(errs, ["`Shelf.put` stores memory in `a` that is freed at the end of the `arena { }` block, but `a` lives longer"]);
    // The union is conservative: `A` stores nothing, but a rule call may reach `B`'s `put`.
    let errs = run(&with_main(&decls, "    var a2 = A()\n    arena {\n        fill_any[A](var a2)\n    }\n"));
    assert_eq!(errs, ["`fill_any` stores memory in `a2` that is freed at the end of the `arena { }` block, but `a2` lives longer"]);
    // A type with no heap memory has nothing to store, so its call isn't checked.
    let plain = "struct C: Shelf {\n    n: int\n    put(var self, v: string) { self.n += 1 }\n}\n";
    let decls = format!("{SHELF}{plain}{b}fill_any[T: Shelf](var x: T) {{ x.put(\"a\") }}\n");
    clean(&with_main(&decls, "    var c = C(n: 1)\n    arena {\n        fill_any[C](var c)\n    }\n"));
}

const STACK: &str = "struct Stack[T] {\n    items: list[T] = []\n    push(var self, v: T) { self.items.add(v) }\n}\nstruct Cell[T] {\n    v: T\n}\nset_cell[T](var c: Cell[T], v: T) { c.v = v }\nfill[T](var s: Stack[T], v: T, n: int) {\n    for i from 0 to n { s.push(v) }\n}\n";

#[test]
fn generic_containers_check() {
    clean(&with_main(STACK, "    var s = Stack[string]()\n    s.push(\"a\")\n    fill[string](var s, \"b\", 2)\n"));
    // `Cell[int]` holds no heap memory, so the call isn't checked.
    clean(&with_main(STACK, "    var c = Cell[int](v: 1)\n    arena {\n        set_cell[int](var c, 2)\n    }\n"));
    let errs = run(&with_main(STACK, "    var s = Stack[string]()\n    arena {\n        s.push(\"b\")\n    }\n"));
    assert_eq!(errs, ["`Stack.push` stores memory in `s` that is freed at the end of the `arena { }` block, but `s` lives longer"]);
    // R4: the summary is the same for every `T`, so `Stack[int]` is refused too, although
    // `si.items.add(1)` written here is fine. Per-type summaries are a follow-up.
    let errs = run(&with_main(STACK, "    var si = Stack[int]()\n    arena {\n        si.push(1)\n    }\n"));
    assert_eq!(errs, ["`Stack.push` stores memory in `si` that is freed at the end of the `arena { }` block, but `si` lives longer"]);
}

#[test]
fn whole_replacement_of_a_placeholder() {
    let swap = "swap[T: equatable](var a: T, var b: T) {\n    let t = a\n    a = b\n    b = t\n}\n";
    let main = "    var x = \"x\"\n    var y = \"y\"\n    swap[string](var x, var y)\n    var i = 1\n    var j = 2\n    swap[int](var i, var j)\n    arena {\n        swap[int](var i, var j)\n    }\n";
    clean(&with_main(swap, main));
    let ab = "interface A {\n    pure m(self) -> int\n}\ninterface B {\n    pure n(self) -> int\n}\n";
    clean(&format!("{ab}g[T: A + B](var a: T, b: T) {{ a = b }}\nmain() {{}}\n"));
    let errs = run(&format!("{ab}f[T: A](var a: T, b: T) {{ a = b }}\nmain() {{}}\n"));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].starts_with("can't replace `a`: it may be the caller's"), "{errs:?}");
    let d = diags("f[T](var a: T, b: T) { a = b }\nmain() {}\n");
    assert_eq!(d.len(), 1);
    assert!(d[0].message.starts_with("can't replace `a`: it may be the caller's"), "{}", d[0].message);
    assert_eq!(d[0].help.as_deref(), Some("if `T` is never an interface type, give it a built-in rule, like `[T: equatable]`"));
    // With an interface rule the message stands alone: an `equatable` type can't meet it.
    let d = diags(&format!("{ab}f[T: A](var a: T, b: T) {{ a = b }}\nmain() {{}}\n"));
    assert_eq!(d[0].help, None);
}

#[test]
fn pure_and_par_for() {
    let src = "pure tag(var xs: list[string], v: string) { xs.add(v) }\ntag_all(var groups: list[list[string]]) {\n    par for g in groups { tag(var g, \"x\") }\n}\n";
    clean(&format!("{src}main() {{}}\n"));
    let errs = run(&with_main(src, "    var groups: list[list[string]] = []\n    arena {\n        tag_all(var groups)\n    }\n"));
    assert_eq!(errs, ["`tag_all` stores memory in `groups` that is freed at the end of the `arena { }` block, but `groups` lives longer"]);
}

#[test]
fn a_call_inside_a_native_call_is_judged_by_its_scratch_arena() {
    let src = "native show(s: string, n: int)\nkeep_name(var names: list[string], v: string) { names.add(v) }\n";
    let main = "    var kept: list[string] = []\n    let n = 1\n    show(\"a{n}\", if n > 0 { keep_name(var kept, \"x\"); 1 } else { 2 })\n";
    let d = diags(&with_main(src, main));
    let msgs: Vec<&str> = d.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(msgs, ["`keep_name` stores memory in `kept` that is freed when the call to `show` returns, but `kept` lives longer"]);
    assert_eq!(d[0].help.as_deref(), Some("call it before the call to `show`, or pass a variable declared inside the argument"));
}

#[test]
fn an_argument_may_not_pass_part_of_a_var_argument() {
    // P40: replacing an owning `w` frees what `v` points at, however the argument reaches it.
    let w = "enum Pick {\n    One\n    Two\n}\nstruct W {\n    mem: Arena = Arena()\n    s: string = \"\"\n}\nf(var w: W, v: string) {\n    w = W()\n    print(v)\n}\n";
    let shared = "`w` is passed as `var` and `w.s` is inside it";
    for arg in ["w.s", "if n > 0 { w.s } else { \"\" }", "match Pick.One {\n        One { w.s }\n        else { \"\" }\n    }"] {
        let errs = run(&with_main(w, &format!("    var w = W()\n    let n = 1\n    f(var w, {arg})\n")));
        assert_eq!(errs, [shared], "{arg}");
    }
    clean(&with_main(w, "    var w = W()\n    var other = W()\n    f(var w, other.s)\n    f(var w, if true { other.s } else { \"\" })\n"));
    // The receiver of a `var self` method counts too.
    let m = "struct V {\n    s: string = \"\"\n    reset(var self, v: string) { self.s = v }\n}\n";
    let errs = run(&with_main(m, "    var v = V()\n    v.reset(if true { v.s } else { \"\" })\n"));
    assert_eq!(errs, ["`v` is changed by this call and `v.s` is inside it"]);
}

#[test]
fn broken_programs_do_not_panic() {
    let keep = "keep_name(var names: list[string], v: string) { names.add(v) }\n";
    let shelf = format!("{SHELF}struct Bad: Shelf {{\n    items: list[string] = []\n    put(var self) {{ self.items.add(\"{{1}}\") }}\n}}\n");
    for src in [
        with_main(keep, "    var xs: list[string] = []\n    arena {\n        keep_name(var xs, \"a\", 3)\n    }\n"),
        with_main(keep, "    arena {\n        keep_name(var nope, \"a\")\n    }\n"),
        with_main(keep, "    keep_name(var 3, \"a\")\n"),
        with_main(keep, "    arena {\n        nope.push(\"a\")\n    }\n"),
        with_main(&shelf, "    var b = Bad()\n    b.put()\n"),
        with_main(&format!("{keep}{shelf}"), "    var s: Shelf = Bad()\n    arena {\n        s.put(\"x\")\n    }\n    let g = keep_name\n"),
        with_main(keep, "    var xs: list[string] = []\n    let f = keep_name\n    arena {\n        f(var xs, \"a\")\n    }\n    keep_name(var xs)\n"),
    ] {
        assert!(!run(&src).is_empty(), "{src}");
    }
}
