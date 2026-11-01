//! Helpers shared by the tests of the passes after range marking (`recurse`, `hoist_checks`,
//! `reassoc`): a program optimised, marked and passed through `after_marking` as a build does it,
//! the checks left in a function, and an outline of its statements for failure messages.
#![allow(dead_code)]

use kestrel_kir::range::sites::{checks_of, is_bounds_site};
use kestrel_kir::range::{mark_proven, Arith};
use kestrel_kir::visit::{walk, Node};
use kestrel_kir::*;
use kestrel_syntax::{parse, LineIndex};

/// `src` (which needs no `main` unless `with_main`) lowered and optimised as the driver does.
pub fn optimized(src: &str) -> Module {
    let src = if src.contains("main()") { src.to_string() } else { format!("{src}\nmain() {{}}\n") };
    let (p, d) = parse(&src);
    assert!(d.is_empty(), "parse: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = kestrel_check::check(&p);
    assert!(d.is_empty(), "check: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let mut m = lower(&p, &t.unwrap(), &LineIndex::new(&src));
    optimize(&mut m);
    m
}

/// `src` as a build in `arith` compiles it: optimised, marked, then `after_marking`.
pub fn built(src: &str, arith: Arith) -> Module {
    let mut m = optimized(src);
    mark_proven(&mut m, arith);
    after_marking(&mut m, arith);
    m
}

/// `src` marked for `arith`, without the passes after marking.
pub fn marked(src: &str, arith: Arith) -> Module {
    let mut m = optimized(src);
    mark_proven(&mut m, arith);
    m
}

pub fn func<'m>(m: &'m Module, name: &str) -> &'m Func {
    m.funcs.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function `{name}`"))
}

/// The checks codegen emits for `stmts` in `arith`: unproven arithmetic nodes and checked indexes.
pub fn checks_in(stmts: &[Stmt], arith: Arith) -> usize {
    let mut n = 0;
    let mut proven_inner: Vec<*const Expr> = Vec::new();
    walk(stmts, &mut |node| match node {
        Node::Expr(e) => {
            if let ExprKind::Proven(x) = &e.kind {
                proven_inner.push(&**x as *const Expr);
            }
            if !proven_inner.contains(&(e as *const Expr)) {
                n += checks_of(e, arith).len();
            }
        }
        Node::Place(p, _) => n += p.path.iter().filter(|s| is_bounds_site(s)).count(),
        Node::Stmt(_) => {}
    });
    n
}

/// The body of the innermost loop on the path of first loops from `stmts`.
pub fn innermost(stmts: &[Stmt]) -> &[Stmt] {
    let mut body = stmts;
    while let Some(s) = body.iter().find(|s| is_loop(s)) {
        body = loop_body(s);
    }
    body
}

pub fn is_loop(s: &Stmt) -> bool {
    matches!(s, Stmt::While { .. } | Stmt::ForRange { .. })
}

pub fn loop_body(s: &Stmt) -> &[Stmt] {
    match s {
        Stmt::While { body, .. } | Stmt::ForRange { body, .. } => body,
        _ => panic!("not a loop"),
    }
}

/// An outline of `stmts`, one statement a line, with `!` after each check left.
pub fn outline(m: &Module, f: &Func, stmts: &[Stmt], arith: Arith) -> String {
    let mut s = String::new();
    for st in stmts {
        stmt(m, f, st, arith, 0, &mut s);
    }
    s
}

fn name(f: &Func, l: LocalIdx) -> String {
    format!("{}#{}", f.locals[l.0 as usize].name, l.0)
}

fn stmt(m: &Module, f: &Func, st: &Stmt, a: Arith, depth: usize, s: &mut String) {
    let pad = "  ".repeat(depth);
    let body = |b: &[Stmt], s: &mut String| b.iter().for_each(|x| stmt(m, f, x, a, depth + 1, s));
    match st {
        Stmt::Let(l, e) => s.push_str(&format!("{pad}let {} = {}\n", name(f, *l), expr(m, f, e, a))),
        Stmt::Assign(p, e) => s.push_str(&format!("{pad}{} = {}\n", place(m, f, p, a), expr(m, f, e, a))),
        Stmt::If { cond, then_body, else_body } => {
            s.push_str(&format!("{pad}if {} {{\n", expr(m, f, cond, a)));
            body(then_body, s);
            s.push_str(&format!("{pad}}} else {{\n"));
            body(else_body, s);
            s.push_str(&format!("{pad}}}\n"));
        }
        Stmt::While { cond, body: b } => {
            s.push_str(&format!("{pad}while {} {{\n", expr(m, f, cond, a)));
            body(b, s);
            s.push_str(&format!("{pad}}}\n"));
        }
        Stmt::ForRange { var, start, end, body: b } => {
            s.push_str(&format!("{pad}for {} from {} to {} {{\n", name(f, *var), expr(m, f, start, a), expr(m, f, end, a)));
            body(b, s);
            s.push_str(&format!("{pad}}}\n"));
        }
        Stmt::Return(Some(e)) => s.push_str(&format!("{pad}return {}\n", expr(m, f, e, a))),
        Stmt::Prove { body: b, .. } | Stmt::Assume { body: b, .. } => {
            s.push_str(&format!("{pad}block {{\n"));
            body(b, s);
            s.push_str(&format!("{pad}}}\n"));
        }
        Stmt::Call(fi, args, _) => s.push_str(&format!("{pad}{}\n", call(m, f, *fi, args, a))),
        other => {
            let text = format!("{other:?}");
            s.push_str(&format!("{pad}{}\n", &text[..text.len().min(100)]));
        }
    }
}

fn place(m: &Module, f: &Func, p: &Place, a: Arith) -> String {
    let mut s = name(f, p.root);
    for step in &p.path {
        match step {
            Proj::Field(i) => s.push_str(&format!(".{i}")),
            Proj::Index { at, check } => {
                s.push_str(&format!("[{}]", expr(m, f, at, a)));
                if *check == IndexCheck::Checked {
                    s.push('!');
                }
            }
            other => s.push_str(&format!(".{other:?}")),
        }
    }
    s
}

fn call(m: &Module, f: &Func, fi: FuncIdx, args: &[Arg], a: Arith) -> String {
    let args: Vec<String> = args
        .iter()
        .map(|x| match x {
            Arg::Value(v) => expr(m, f, v, a),
            Arg::Ref(p) => format!("&{}", place(m, f, p, a)),
        })
        .collect();
    format!("{}({})", m.funcs[fi.0 as usize].name, args.join(", "))
}

pub fn expr(m: &Module, f: &Func, e: &Expr, a: Arith) -> String {
    let mark = if checks_of(e, a).is_empty() { "" } else { "!" };
    match &e.kind {
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Float(x) => format!("{x:?}"),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Load(p) | ExprKind::Move(p) => place(m, f, p, a),
        ExprKind::Len(p) => format!("{}.len", place(m, f, p, a)),
        ExprKind::Proven(x) => match &x.kind {
            ExprKind::Binary(op, l, r) => format!("({} {op:?} {})", expr(m, f, l, a), expr(m, f, r, a)),
            ExprKind::Cast(y) => format!("{:?}({})", x.ty, expr(m, f, y, a)),
            _ => format!("proven({})", expr(m, f, x, a)),
        },
        ExprKind::Binary(op, l, r) => format!("({} {op:?}{mark} {})", expr(m, f, l, a), expr(m, f, r, a)),
        ExprKind::Unary(op, x) => format!("{op:?}{mark}({})", expr(m, f, x, a)),
        ExprKind::Cast(x) => format!("{:?}{mark}({})", e.ty, expr(m, f, x, a)),
        ExprKind::Math(op, xs) => format!("{op:?}({})", xs.iter().map(|x| expr(m, f, x, a)).collect::<Vec<_>>().join(", ")),
        ExprKind::Call(fi, args) => call(m, f, *fi, args, a),
        ExprKind::If { cond, then_body, then_val, else_body, else_val } => {
            let mut s = format!("if {} {{ ", expr(m, f, cond, a));
            s.push_str(&outline(m, f, then_body, a).replace('\n', "; "));
            if let Some(v) = then_val {
                s.push_str(&expr(m, f, v, a));
            }
            s.push_str(" } else { ");
            s.push_str(&outline(m, f, else_body, a).replace('\n', "; "));
            if let Some(v) = else_val {
                s.push_str(&expr(m, f, v, a));
            }
            s.push_str(" }");
            s
        }
        ExprKind::Seq(stmts, v) => format!("seq {{ {}{} }}", outline(m, f, stmts, a).replace('\n', "; "), expr(m, f, v, a)),
        other => {
            let text = format!("{other:?}");
            text[..text.len().min(60)].to_string()
        }
    }
}
