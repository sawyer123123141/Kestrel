//! `sort` and `sort_by`: `ListOp::Sort` with a generated compare function (strings, and `sort_by`
//! with a string key) or `ListOp::SortBy` with a generated fill function (numbers and bools, and a
//! number or bool key), a call site that passes a literal key as a stack environment, and a scratch
//! arena around each key only when it can build memory.

use kestrel_kir::*;
use kestrel_syntax::{parse, LineIndex};

fn lower_src(src: &str) -> Module {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = kestrel_check::check(&p);
    assert!(d.is_empty(), "check: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    lower(&p, &t.unwrap(), &LineIndex::new(src))
}

fn named<'m>(m: &'m Module, prefix: &str) -> Vec<&'m Func> {
    m.funcs.iter().filter(|f| f.name.starts_with(prefix)).collect()
}

fn func<'m>(m: &'m Module, name: &str) -> &'m Func {
    m.funcs.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}"))
}

/// The top-level statements of `f` that open or close an arena.
fn arenas(f: &Func) -> Vec<&Stmt> {
    f.body.iter().filter(|s| matches!(s, Stmt::PushArena(..) | Stmt::PopArena { .. })).collect()
}

/// The body of the loop of fill function `f`, its only statement.
fn fill_loop(f: &Func) -> &[Stmt] {
    match f.body.as_slice() {
        [Stmt::ForRange { body, .. }] => body,
        other => panic!("a fill function is one loop: {other:?}"),
    }
}

/// The statements of fill function `f`'s loop that open or close an arena.
fn loop_arenas(f: &Func) -> Vec<&Stmt> {
    fill_loop(f).iter().filter(|s| matches!(s, Stmt::PushArena(..) | Stmt::PopArena { .. })).collect()
}

/// Checks the shape of fill function `k`: `(env, xs, keys)`, `xs` and `keys` by pointer, `keys`
/// an array of `i64` or `f64`, pure, no result; its loop ends by writing the key. Returns that
/// key's value.
fn fill_shape<'f>(m: &Module, k: &'f Func) -> &'f Expr {
    assert_eq!(k.params.len(), 3);
    assert_eq!([k.locals[1].kind, k.locals[2].kind], [LocalKind::RefParam; 2]);
    assert!(k.is_pure);
    assert_eq!(k.ret, None);
    let KTy::Array(e) = k.locals[2].ty else { panic!("{k:?}") };
    assert!(matches!(m.elems[e.0 as usize], KTy::I64 | KTy::F64));
    let Some(Stmt::Assign(p, v)) = fill_loop(k).last() else { panic!("{k:?}") };
    assert_eq!(p.root, LocalIdx(2));
    assert_eq!(Some(v.ty), Some(m.elems[e.0 as usize]));
    v
}

fn sorts(f: &Func) -> Vec<usize> {
    let mut out = Vec::new();
    visit::walk(&f.body, &mut |n| {
        if let visit::Node::Stmt(Stmt::List(ListOp::Sort | ListOp::SortBy(_), _, args)) = n {
            out.push(args.len());
        }
    });
    out
}

const PRELUDE: &str = "struct E {
    name: string
    hp: int
}

pure shout(e: E) -> int { e.name.upper().len }

";

#[test]
fn sort_is_one_runtime_sort_with_a_compare_or_fill_function_per_type() {
    let m = lower_src("main() {
    var xs: list[int] = [3, 1]
    var ys: list[int] = [2, 1]
    var ws: list[string] = [\"b\", \"a\"]
    var fs: list[float] = [2.0, 1.0]
    xs.sort()
    ys.sort()
    ws.sort()
    fs.sort()
}
");
    // Strings have a compare function; numbers a fill function per list type, which the two int
    // lists share.
    let compares = named(&m, "__kestrel_compare_");
    assert_eq!(compares.len(), 1);
    assert_eq!(sorts(func(&m, "main")), vec![1, 1, 1, 1]);
    for c in compares {
        // (env, a, b) -> u8, a and b by pointer; nothing to free.
        assert_eq!(c.params.len(), 3);
        assert_eq!(c.ret, Some(KTy::U8));
        assert_eq!([c.locals[1].kind, c.locals[2].kind], [LocalKind::RefParam; 2]);
        assert!(arenas(c).is_empty());
        assert!(c.is_pure);
    }
    let fills = named(&m, "__kestrel_sort_key_");
    assert_eq!(fills.len(), 2);
    for f in fills {
        // The item itself, widened: an `int` converted, a `float` too.
        assert!(matches!(fill_shape(&m, f).kind, ExprKind::Cast(_)));
        assert!(loop_arenas(f).is_empty());
    }
    let mut ops = Vec::new();
    visit::walk(&func(&m, "main").body, &mut |n| {
        if let visit::Node::Stmt(Stmt::List(op, _, _)) = n {
            ops.push(*op);
        }
    });
    assert_eq!(ops, [ListOp::SortBy(SortKey::Int), ListOp::SortBy(SortKey::Int), ListOp::Sort, ListOp::SortBy(SortKey::Float)]);
    assert_eq!(check_stack_envs(&m), Ok(()));
}

#[test]
fn a_literal_key_is_a_stack_environment_passed_straight_to_the_sorting_function() {
    let m = lower_src(&format!(
        "{PRELUDE}main() {{
    var es: list[E] = []
    let k = 2
    es.sort_by((e) {{ e.hp + k }})
}}
"
    ));
    assert_eq!(check_stack_envs(&m), Ok(()));
    let calls = named(&m, "__kestrel_sort_by_");
    assert_eq!(calls.len(), 1);
    assert_eq!(sorts(calls[0]), vec![2], "the fill function, then the key to pass on by address");
    // `main` calls it with the list by reference and the literal itself.
    let mut found = false;
    visit::walk(&func(&m, "main").body, &mut |n| {
        if let visit::Node::Stmt(Stmt::Call(f, args, _)) = n {
            if m.funcs[f.0 as usize].name.starts_with("__kestrel_sort_by_") {
                found = true;
                assert!(matches!(&args[0], Arg::Ref(_)));
                assert!(matches!(&args[1], Arg::Value(Expr { kind: ExprKind::MakeFn { env: EnvKind::Stack(_), .. }, .. })));
            }
        }
    });
    assert!(found);
}

#[test]
fn a_key_that_cant_build_memory_gets_no_scratch_arena() {
    let m = lower_src(&format!(
        "{PRELUDE}main() {{
    var es: list[E] = []
    es.sort_by((e) {{ e.hp * 2 }})
    es.sort_by((e) {{ e.name.len }})
    es.sort_by(hp)
}}

pure hp(e: E) -> int {{ e.hp }}
"
    ));
    // (A named function's thunk is the code, so it is judged like a literal.)
    let keys = named(&m, "__kestrel_sort_key_");
    assert_eq!(keys.len(), 3);
    for c in keys {
        assert!(loop_arenas(c).is_empty(), "{c:?}");
    }
}

#[test]
fn a_key_that_builds_strings_is_run_in_a_scratch_arena_freed_before_the_answer() {
    let m = lower_src(&format!(
        "{PRELUDE}main() {{
    var es: list[E] = []
    es.sort_by((e) {{ e.name.lower() }})
    es.sort_by((e) {{ shout(e) }})
    es.sort_by(shout)
    let by = (e: E) -> int {{ e.hp }}
    es.sort_by(by)
}}
"
    ));
    // The first builds a string itself (a string key: a compare function), the second calls a
    // function that does, the third is a named function that does, and the fourth is a stored
    // closure (anything may be inside it, so it keeps the arena even though this one builds
    // nothing); those three have number keys, so fill functions.
    let compares = named(&m, "__kestrel_compare_");
    assert_eq!(compares.len(), 1);
    let keys = named(&m, "__kestrel_sort_key_");
    assert_eq!(keys.len(), 3);
    // A compare function's arena is open first and closed just before the answer is returned; a
    // fill function's is open first in each turn of the loop once the item is bound, and closed
    // before the key is written.
    for (body, last) in compares.iter().map(|c| (c.body.as_slice(), "answer")).chain(keys.iter().map(|k| (&fill_loop(k)[1..], "key"))) {
        let a: Vec<&Stmt> = body.iter().filter(|s| matches!(s, Stmt::PushArena(..) | Stmt::PopArena { .. })).collect();
        assert_eq!(a.len(), 2, "{body:?}");
        assert!(matches!(body.first(), Some(Stmt::PushArena(None, _))));
        assert!(matches!(&body[body.len() - 2], Stmt::PopArena { free: true }));
        match last {
            "answer" => assert!(matches!(body.last(), Some(Stmt::Return(Some(_))))),
            _ => assert!(matches!(body.last(), Some(Stmt::Assign(..)))),
        }
    }
}

#[test]
fn a_function_value_key_is_called_indirectly_and_a_literal_directly() {
    let m = lower_src(&format!(
        "{PRELUDE}main() {{
    var es: list[E] = []
    let by = (e: E) -> int {{ e.hp }}
    es.sort_by((e) {{ e.hp }})
    es.sort_by(by)
}}
"
    ));
    let mut counts = Vec::new();
    for c in named(&m, "__kestrel_sort_key_") {
        let (mut direct, mut indirect) = (0, 0);
        visit::walk(&c.body, &mut |n| match n {
            visit::Node::Expr(Expr { kind: ExprKind::Call(..), .. }) => direct += 1,
            visit::Node::Expr(Expr { kind: ExprKind::CallIndirect { .. }, .. }) => indirect += 1,
            _ => {}
        });
        counts.push((direct, indirect));
    }
    counts.sort();
    // One call each, in the loop over the items.
    assert_eq!(counts, vec![(0, 1), (1, 0)]);
}

/// The kinds of sort in the `__kestrel_sort_by_*` functions of `m`, in order of the functions.
fn sort_ops(m: &Module) -> Vec<ListOp> {
    let mut out = Vec::new();
    for f in named(m, "__kestrel_sort_by_") {
        visit::walk(&f.body, &mut |n| {
            if let visit::Node::Stmt(Stmt::List(op, _, _)) = n {
                out.push(*op);
            }
        });
    }
    out
}

#[test]
fn a_number_or_bool_key_is_a_fill_function_and_a_string_key_a_compare_function() {
    let m = lower_src(&format!(
        "{PRELUDE}main() {{
    var es: list[E] = []
    es.sort_by((e) {{ e.hp }})
    es.sort_by((e) {{ i64(e.hp) }})
    es.sort_by((e) {{ u8(e.hp & 7) }})
    es.sort_by((e) {{ e.hp > 3 }})
    es.sort_by((e) {{ float(e.hp) }})
    es.sort_by((e) {{ f64(e.hp) }})
    es.sort_by((e) {{ e.name }})
}}
"
    ));
    use SortKey::{Float, Int};
    let expected = [ListOp::SortBy(Int), ListOp::SortBy(Int), ListOp::SortBy(Int), ListOp::SortBy(Int), ListOp::SortBy(Float), ListOp::SortBy(Float), ListOp::Sort];
    assert_eq!(sort_ops(&m), expected);
    let keys = named(&m, "__kestrel_sort_key_");
    assert_eq!(keys.len(), 6);
    for k in &keys {
        let v = fill_shape(&m, k);
        let wide = if k.locals[4].ty.is_float() { KTy::F64 } else { KTy::I64 };
        assert_eq!(v.ty, wide, "{k:?}");
        // A bool key is 0 or 1; a narrower number is converted; an `i64` or `f64` is as it is.
        match k.locals[4].ty {
            KTy::Bool => assert!(matches!(v.kind, ExprKind::If { .. }), "{v:?}"),
            KTy::I64 | KTy::F64 => assert!(matches!(v.kind, ExprKind::Load(_)), "{v:?}"),
            _ => assert!(matches!(v.kind, ExprKind::Cast(_)), "{v:?}"),
        }
    }
    assert_eq!(named(&m, "__kestrel_compare_").len(), 1);
}

#[test]
fn an_array_sorts_like_a_list() {
    let m = lower_src("main() {
    var a = array[int](3)
    a.sort()
    a.sort_by((n) { -n })
}
");
    assert_eq!(sorts(func(&m, "main")).len() + named(&m, "__kestrel_sort_by_").iter().map(|f| sorts(f).len()).sum::<usize>(), 2);
    assert_eq!(check_stack_envs(&m), Ok(()));
}
