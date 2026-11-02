//! Generics used with their types written (G8): `best[Enemy](foes)`, `Stack[int]()`,
//! `Outcome[int, string].Ok(..)`, `Maybe[int].Nothing`, `let f = best[Enemy]`.

use kestrel_check::{check_keeping_tables, Res, Tables, Ty};
use kestrel_syntax::ast::{Block, Expr, ExprId, ExprKind, Item, Program};
use kestrel_syntax::parse;
use kestrel_syntax::visit::each_expr;

fn run(src: &str) -> (Program, Tables, Vec<String>) {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check_keeping_tables(&p, true);
    let errs = d.into_iter().filter(|d| d.is_error()).map(|d| d.message).collect();
    (p, t, errs)
}

fn clean(src: &str) -> (Program, Tables) {
    let (p, t, errs) = run(src);
    assert!(errs.is_empty(), "{errs:?}");
    (p, t)
}

fn errors(src: &str) -> Vec<String> {
    run(src).2
}

fn bodies(p: &Program) -> Vec<&Block> {
    let mut out = Vec::new();
    for item in &p.items {
        match item {
            Item::Fn(f) => out.push(&f.body),
            Item::Struct(s) => out.extend(s.methods.iter().map(|m| &m.body)),
            Item::Enum(e) => out.extend(e.methods.iter().map(|m| &m.body)),
            _ => {}
        }
    }
    out
}

/// The ids of every expression in a body that `pred` accepts, in source order.
fn find_where(p: &Program, pred: impl Fn(&Expr) -> bool) -> Vec<ExprId> {
    let mut out = Vec::new();
    for b in bodies(p) {
        each_expr(b, &mut |e: &Expr| {
            if pred(e) {
                out.push(e.id);
            }
        });
    }
    out
}

fn the(p: &Program, pred: impl Fn(&Expr) -> bool) -> ExprId {
    let all = find_where(p, pred);
    assert_eq!(all.len(), 1, "expressions: {all:?}");
    all[0]
}

fn type_call(name: &'static str) -> impl Fn(&Expr) -> bool {
    move |e| matches!(&e.kind, ExprKind::TypeCall { name: n, .. } if n == name)
}

fn type_apply(name: &'static str) -> impl Fn(&Expr) -> bool {
    move |e| matches!(&e.kind, ExprKind::TypeApply { name: n, .. } if n == name)
}

fn type_args(t: &Tables, e: ExprId) -> Vec<String> {
    let list = t.generic.type_args.get(&e).unwrap_or_else(|| panic!("no type_args for {e:?}"));
    t.ty_list(*list).iter().map(|&ty| t.ty_name(ty)).collect()
}

const DECLS: &str = "struct Stack[T] {\n    items: list[T] = []\n}\nenum Outcome[T, E] {\n    Ok { value: T }\n    Err { error: E }\n}\nenum Maybe[T] {\n    Nothing\n    Just { value: T }\n}\nstruct Enemy {\n    hp: int\n}\npure best[T](xs: list[T], score: pure (T) -> int) -> T? { none }\n";

#[test]
fn written_types_are_recorded() {
    let src = format!(
        "{DECLS}let RULE = best[int]\nlet EMPTY = Stack[int]()\nmain() {{\n    let foes: list[Enemy] = []\n    let b = best[Enemy](foes, (e) {{ e.hp }})\n    let s = Stack[int](items: [1])\n    let r = Outcome[int, string].Ok(value: 3)\n    let m = Maybe[int].Nothing\n    let f = best[Enemy]\n}}\n"
    );
    let (p, t) = clean(&src);
    let ty = |e: ExprId| t.ty_name(t.ty(e));

    let call = the(&p, type_call("best"));
    let fid = t.fn_by_name["best"];
    assert!(matches!(t.res.get(&call), Some(Res::Fn(f)) if *f == fid), "{:?}", t.res.get(&call));
    assert_eq!(type_args(&t, call), ["Enemy"]);
    assert_eq!(ty(call), "Enemy?");
    // The closure's parameter is an `Enemy`, so `e.hp` checked.
    let closure = find_where(&p, |e| matches!(e.kind, ExprKind::Closure(_)));
    assert_eq!(closure.len(), 1);
    assert_eq!(ty(closure[0]), "pure (Enemy) -> int");

    let construct = the(&p, type_call("Stack"));
    let Some(Res::Construct(sid)) = t.res.get(&construct) else { panic!("a construction") };
    assert_eq!(t.strukt(*sid).name, "Stack[int]");
    assert!(t.strukt(*sid).instance.is_some());
    assert_eq!(ty(construct), "Stack[int]");
    assert_eq!(type_args(&t, construct), ["int"]);

    let ok = the(&p, |e| matches!(&e.kind, ExprKind::MethodCall { name, .. } if name == "Ok"));
    assert_eq!(ty(ok), "Outcome[int, string]");
    assert!(matches!(t.res.get(&ok), Some(Res::Variant(_, 0))), "{:?}", t.res.get(&ok));
    assert_eq!(type_args(&t, ok), ["int", "string"]);

    let nothing = the(&p, |e| matches!(&e.kind, ExprKind::Field { name, .. } if name == "Nothing"));
    assert_eq!(ty(nothing), "Maybe[int]");
    assert!(matches!(t.res.get(&nothing), Some(Res::Variant(_, 0))));
    assert_eq!(type_args(&t, nothing), ["int"]);

    // Only `main` has a function value in its body; the constant `RULE` is `best[int]`.
    let f = the(&p, type_apply("best"));
    assert!(matches!(t.res.get(&f), Some(Res::FnValue(g)) if *g == fid), "{:?}", t.res.get(&f));
    assert_eq!(type_args(&t, f), ["Enemy"]);
    assert_eq!(ty(f), "pure (list[Enemy], pure (Enemy) -> int) -> Enemy?");

    let rule = t.consts.iter().find(|c| c.name == "RULE").expect("RULE");
    assert_eq!(t.ty_name(rule.ty), "pure (list[int], pure (int) -> int) -> int?");
    let empty = t.consts.iter().find(|c| c.name == "EMPTY").expect("EMPTY");
    assert_eq!(t.ty_name(empty.ty), "Stack[int]");
}

#[test]
fn a_constant_can_not_call_a_generic_function() {
    let src = "pure best[T](xs: list[T], score: pure (T) -> int) -> T? { none }\nlet XS: list[int] = []\nlet RULE = best[int]\nlet BAD = best[int](XS, (x) { x })\nmain() {}\n";
    let errs = errors(src);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].starts_with("constants can only use"), "{errs:?}");
    // A written construction and an enum value are constants.
    clean(&format!("{DECLS}let A = Stack[int]()\nlet B = Maybe[int].Nothing\nlet C = Outcome[int, string].Ok(value: 1)\nmain() {{}}\n"));
}

#[test]
fn type_calls_as_statements() {
    let decls = "struct Stack[T] {\n    items: list[T] = []\n    push(var self, v: T) { self.items.add(v) }\n}\nfill[T](var s: Stack[T], a: T, b: T) {\n    s.push(a)\n    s.push(b)\n}\npure size[T](xs: list[T]) -> int { xs.len }\n";
    clean(&format!("{decls}main() {{\n    var s = Stack[int]()\n    fill[int](var s, 1, 2)\n}}\n"));
    assert_eq!(
        errors(&format!("{decls}main() {{\n    var s = Stack[int]()\n    size[int](s.items)\n}}\n")),
        ["the result of pure function `size` is unused, so this line does nothing"]
    );
    assert_eq!(
        errors(&format!("{decls}main() {{\n    Stack[int]()\n}}\n")),
        ["this line has no effect; did you mean to continue the line above?"]
    );
    assert_eq!(
        errors(&format!("{decls}main() {{\n    list[int]()\n}}\n")),
        ["this line has no effect; did you mean to continue the line above?"]
    );
}

#[test]
fn counts_and_plain_names() {
    let errs = errors(&format!(
        "{DECLS}main() {{\n    let a = Outcome[int].Ok(value: 1)\n    let b = best[int, int]\n    let c = Enemy[int](hp: 1)\n    let d = Enemy[int]\n    let e = Stack[int, int]()\n    let f = Maybe[int, int].Nothing\n}}\n"
    ));
    assert_eq!(
        errs,
        [
            "`Outcome` takes 2 types, found 1",
            "`best` takes 1 type, found 2",
            "`Enemy` takes no types",
            "`Enemy` takes no types",
            "`Stack` takes 1 type, found 2",
            "`Maybe` takes 1 type, found 2",
        ]
    );
}

#[test]
fn a_type_is_not_a_value() {
    let errs = errors(&format!("{DECLS}main() {{\n    let a = Stack[int]\n    let b = Outcome[int, string]\n    let c = Maybe[int]\n}}\n"));
    // An enum is built from a variant: its first one is the example.
    assert_eq!(
        errs,
        [
            "`Stack[int]` is a type; build one with `Stack[int](...)`",
            "`Outcome[int, string]` is a type; build one with `Outcome[int, string].Ok(...)`",
            "`Maybe[int]` is a type; build one with `Maybe[int].Nothing`",
        ]
    );
}

#[test]
fn nothing_is_decided_without_written_types() {
    let errs = errors(&format!(
        "{DECLS}pure make_empty[T]() -> list[T] {{ list[T]() }}\npair[A, B](a: A) {{ }}\nmain() {{\n    let a = make_empty()\n    pair(1)\n    let s = Stack()\n    let m = Maybe.Nothing\n    let r = Outcome.Ok(value: 1)\n    let f = best\n}}\n"
    ));
    assert_eq!(
        errs,
        [
            "can't tell what `T` is in `make_empty`; write it, like `make_empty[int](...)`",
            "can't tell what `B` is in `pair`; write it, like `pair[int, int](...)`",
            "can't tell what `T` is in `Stack`; write it, like `Stack[int]()`",
            "can't tell what `T` is in `Maybe`; write it, like `Maybe[int].Nothing`",
            "can't tell what `E` is in `Outcome`; write it, like `Outcome[int, int].Ok(...)`",
            "`best` is generic; give its types to use it as a value, like `best[int]`",
        ]
    );
}

#[test]
fn a_generic_names_itself_inside_its_declaration() {
    // G2: `Stack` and `Maybe` in their own methods mean `Stack[T]` and `Maybe[T]`.
    clean("struct Stack[T] {\n    items: list[T] = []\n    fresh(self) -> Stack { Stack() }\n}\nenum Maybe[T] {\n    Nothing\n    Just { value: T }\n    none_of(self) -> Maybe { Maybe.Nothing }\n}\nmain() {}\n");
}

#[test]
fn written_types_meet_their_rules() {
    let decls = "interface Damageable {\n    pure hp(self) -> int\n}\nstruct Rock {\n    n: int\n}\nstruct Orc: Damageable {\n    n: int\n    pure hp(self) -> int { self.n }\n}\nstruct Pen[T: Damageable] {\n    xs: list[T] = []\n}\nenum Slot[T: Damageable] {\n    Empty\n    Full { v: T }\n}\npure total[T: Damageable](xs: list[T]) -> int { 0 }\n";
    let errs = errors(&format!(
        "{decls}main() {{\n    let rocks: list[Rock] = []\n    let a = Pen[Rock]()\n    let b = Slot[Rock].Empty\n    let c = Slot[Rock].Full(v: Rock(n: 1))\n    let d = total[Rock]\n    let e = total[Rock](rocks)\n    let f = Pen[int?]()\n}}\n"
    ));
    assert_eq!(
        errs,
        [
            "`Rock` isn't `Damageable`, so it can't be `T` in `Pen`",
            "`Rock` isn't `Damageable`, so it can't be `T` in `Slot`",
            "`Rock` isn't `Damageable`, so it can't be `T` in `Slot`",
            "`Rock` isn't `Damageable`, so it can't be `T` in `total`",
            "`Rock` isn't `Damageable`, so it can't be `T` in `total`",
            "`int?` is an optional, so it can't be `T` yet; hold it in a struct field instead",
        ]
    );
    clean(&format!("{decls}main() {{\n    let orcs: list[Orc] = []\n    let a = Pen[Orc]()\n    let b = Slot[Orc].Empty\n    let c = Slot[Damageable].Empty\n    let d = total[Orc]\n    let e = total[Orc](orcs)\n}}\n"));
}

#[test]
fn written_types_in_generic_code() {
    // A placeholder of the surrounding generic can be written, and meets only the rules it carries.
    let src = format!("{DECLS}pure first[T](x: T) -> int {{\n    let s = Stack[T]()\n    s.items.len\n}}\nmain() {{}}\n");
    let (p, t) = clean(&src);
    let s = the(&p, type_call("Stack"));
    assert_eq!(type_args(&t, s), ["T"]);
    let errs = errors(
        "interface D {\n    pure hp(self) -> int\n}\nstruct Pen[T: D] {\n    xs: list[T] = []\n}\npure first[T](x: T) -> int {\n    let s = Pen[T]()\n    s.xs.len\n}\nmain() {}\n"
    );
    assert_eq!(errs, ["`T` isn't `D`, so it can't be `T` in `Pen`; add the rule: `[T: D]`"]);
}

#[test]
fn field_defaults_with_written_types_form_one_loop() {
    let errs = errors("struct Node[T] {\n    kids: list[Node[int]] = [Node[int]()]\n}\nmain() {}\n");
    let loops: Vec<&String> = errs.iter().filter(|e| e.contains("field defaults build each other in a loop")).collect();
    assert_eq!(loops.len(), 1, "{errs:?}");
    assert!(loops[0].contains("Node.kids -> Node.kids"), "{loops:?}");
}

#[test]
fn a_written_call_can_be_used_where_a_value_goes() {
    let src = format!(
        "{DECLS}pure size[T](xs: list[T]) -> int {{ xs.len }}
main() {{
    let f = size[int]
    let xs: list[int] = [1, 2]
    let n: int = f(xs)
    let m: Maybe[int] = Maybe[int].Just(value: 2)
    let k: int = size[int](xs)
}}
"
    );
    let (p, t) = clean(&src);
    let m = the(&p, |e| matches!(&e.kind, ExprKind::MethodCall { name, .. } if name == "Just"));
    assert!(matches!(t.ty(m), Ty::Enum(_)));
    assert_eq!(t.ty_name(t.ty(m)), "Maybe[int]");
}

#[test]
fn a_function_value_points_into_no_arena() {
    // Like a plain function's name, `size[int]` has no environment: storing it past an arena is fine.
    clean("pure size[T](xs: list[T]) -> int { xs.len }\npure zero[T](xs: list[T]) -> int { 0 }\nmain() {\n    var f = size[int]\n    arena {\n        f = zero[int]\n    }\n}\n");
}

#[test]
fn a_written_function_value_in_generic_code_and_as_an_argument() {
    // A function value of a placeholder's type, made inside generic code, and handed to another
    // function: the argument is the substituted signature.
    let src = "pure size[T](xs: list[T]) -> int { xs.len }\npure apply[T](xs: list[T], f: pure (list[T]) -> int) -> int { f(xs) }\npure twice[U](xs: list[U]) -> int { apply[U](xs, size[U]) }\nmain() {\n    let xs: list[int] = [1]\n    let n: int = twice[int](xs)\n}\n";
    let (p, t) = clean(src);
    let inner = the(&p, type_apply("size"));
    assert_eq!(type_args(&t, inner), ["U"]);
    assert_eq!(t.ty_name(t.ty(inner)), "pure (list[U]) -> int");
}

#[test]
fn types_after_a_plain_function_suggest_calling_it() {
    // `scores[i]` of a plain function settles as types (it names a function): the help says how
    // to index what it returns.
    let src = "pure scores() -> list[int] { [1] }\npure nth(n: int) -> list[int] { [n] }\nmain() {\n    let i = 0\n    let a = scores[i]\n    let b = nth[i]\n}\n";
    let (p, _) = parse(src);
    let (_, d) = check_keeping_tables(&p, true);
    let got: Vec<(String, Option<String>)> = d.into_iter().map(|d| (d.message, d.help)).collect();
    assert_eq!(
        got,
        [
            ("`scores` takes no types".to_string(), Some("to index what `scores` returns, call it first: `scores()[...]`".to_string())),
            ("`nth` takes no types".to_string(), Some("to index what `nth` returns, call it first: `nth(...)[...]`".to_string())),
        ]
    );
}
