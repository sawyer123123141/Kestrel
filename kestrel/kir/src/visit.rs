//! Walks KIR, reporting every statement, expression and place in it.

use crate::ir::*;

pub enum Node<'a> {
    Stmt(&'a Stmt),
    Expr(&'a Expr),
    /// A place and whether it's written (assigned, passed as `var`, moved from).
    Place(&'a Place, bool),
}

/// Calls `f` for each statement and expression in `stmts`, outermost
/// first, and for each place (after the index expressions inside it).
pub fn walk<'a>(stmts: &'a [Stmt], f: &mut impl FnMut(Node<'a>)) {
    for s in stmts {
        stmt(s, f);
    }
}

/// Calls `f(place, written)` for each place in `stmts`. A place passed as
/// a `var` argument counts as written.
pub fn places(stmts: &[Stmt], f: &mut impl FnMut(&Place, bool)) {
    walk(stmts, &mut |n| {
        if let Node::Place(p, written) = n {
            f(p, written);
        }
    });
}

fn stmt<'a>(s: &'a Stmt, f: &mut impl FnMut(Node<'a>)) {
    f(Node::Stmt(s));
    match s {
        Stmt::Let(_, e) | Stmt::Return(Some(e)) | Stmt::RandSeed(e) | Stmt::Fail { message: e, .. } | Stmt::Exit { code: e, .. } => walk_expr(e, f),
        Stmt::Assign(p, e) | Stmt::ListAdd(p, e) | Stmt::SetLen(p, e) => {
            walk_expr(e, f);
            place(p, true, f);
        }
        Stmt::List(_, p, xs) => {
            for x in xs {
                walk_expr(x, f);
            }
            place(p, true, f);
        }
        Stmt::Bind(_, p) => place(p, false, f),
        // A capture's place is in the environment, which no local owns.
        Stmt::BindEnv { env, .. } => walk_expr(env, f),
        Stmt::CopyFnEnv(p) | Stmt::CopyIface(p) => place(p, true, f),
        Stmt::MapSet(p, k, v, _) => {
            walk_expr(k, f);
            walk_expr(v, f);
            place(p, true, f);
        }
        Stmt::MapUnlock(p, saved) => {
            walk_expr(saved, f);
            place(p, true, f);
        }
        Stmt::ParFor { args: a, count, .. } => {
            walk_expr(count, f);
            args(a, f);
        }
        Stmt::Dup(p) | Stmt::FreeArena(p) | Stmt::DropGpu(p) => place(p, true, f),
        // The source is read, then the target written (its items and its length).
        Stmt::CopyItems(dst, src) => {
            place(src, false, f);
            place(dst, true, f);
        }
        Stmt::PushArena(Some(p), _) => place(p, true, f),
        Stmt::PushArena(None, _) | Stmt::PopArena { .. } => {}
        Stmt::If { cond, then_body, else_body } => {
            walk_expr(cond, f);
            walk(then_body, f);
            walk(else_body, f);
        }
        Stmt::While { cond, body } => {
            walk_expr(cond, f);
            walk(body, f);
        }
        Stmt::Assume { conds, body, .. } => {
            for (_, c) in conds {
                walk_expr(c, f);
            }
            walk(body, f);
        }
        Stmt::Prove { body, .. } => walk(body, f),
        Stmt::ForRange { start, end, body, .. } => {
            walk_expr(start, f);
            walk_expr(end, f);
            walk(body, f);
        }
        Stmt::Call(_, args, _) => self::args(args, f),
        Stmt::CallIndirect { callee, args, .. } => {
            place(callee, false, f);
            self::args(args, f);
        }
        // The interface value itself isn't changed (a `var self` method changes what it points at).
        Stmt::CallVirtual { recv, args, .. } => {
            place(recv, false, f);
            self::args(args, f);
        }
        Stmt::Print(parts) | Stmt::Write(parts) => print_args(parts, f),
        // None reads or writes a place. (`BindGlobal` only points a local at a global.)
        Stmt::BindGlobal(..) | Stmt::SetLine(_) | Stmt::Trap | Stmt::Break | Stmt::Continue | Stmt::Return(None) => {}
    }
}

fn place<'a>(p: &'a Place, written: bool, f: &mut impl FnMut(Node<'a>)) {
    for at in p.path.iter().filter_map(Proj::at) {
        walk_expr(at, f);
    }
    f(Node::Place(p, written));
}

fn args<'a>(args: &'a [Arg], f: &mut impl FnMut(Node<'a>)) {
    for a in args {
        match a {
            Arg::Value(e) => walk_expr(e, f),
            Arg::Ref(p) => place(p, true, f),
        }
    }
}

fn print_args<'a>(parts: &'a [PrintArg], f: &mut impl FnMut(Node<'a>)) {
    for p in parts {
        match p {
            PrintArg::Value(e) => walk_expr(e, f),
            PrintArg::Interp(inner) => print_args(inner, f),
            PrintArg::Str(_) => {}
        }
    }
}

/// Calls `f` for `e` and each statement, expression and place inside it, as `walk` does.
pub fn walk_expr<'a>(e: &'a Expr, f: &mut impl FnMut(Node<'a>)) {
    f(Node::Expr(e));
    match &e.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::StrLit(_)
        | ExprKind::ListNew
        | ExprKind::MapNew
        | ExprKind::NewArena
        | ExprKind::StaticArena
        | ExprKind::NoneLit
        | ExprKind::FnRef(_)
        | ExprKind::MakeFn { env: EnvKind::None, .. }
        | ExprKind::RandFloat { .. } => {}
        ExprKind::Move(p) => place(p, true, f),
        ExprKind::Map(op, p, xs) => {
            for x in xs {
                walk_expr(x, f);
            }
            place(p, matches!(op, MapOp::Remove | MapOp::Lock | MapOp::CopyInto), f);
        }
        ExprKind::ReadBytes(p, xs) => {
            for x in xs {
                walk_expr(x, f);
            }
            place(p, false, f);
        }
        ExprKind::Format(parts) => print_args(parts, f),
        ExprKind::Load(p) | ExprKind::Tag(p) | ExprKind::Len(p) | ExprKind::Cap(p) | ExprKind::FnEnv(p) => place(p, false, f),
        ExprKind::Unary(_, x)
        | ExprKind::Cast(x)
        | ExprKind::Proven(x)
        | ExprKind::FieldOf(x, _)
        | ExprKind::ArrayNew(x)
        | ExprKind::Some(x)
        | ExprKind::EnvDup(x)
        | ExprKind::BlockDup { src: x, .. }
        | ExprKind::Splat(x)
        | ExprKind::Shuffle(x, _)
        | ExprKind::LaneOf(x, _) => {
            walk_expr(x, f)
        }
        ExprKind::MakeFn { env: EnvKind::Heap(env) | EnvKind::Stack(env), .. } => {
            for v in &env.values {
                walk_expr(v, f);
            }
        }
        // A borrowed value lends its place to the call, which may change it (as for `Arg::Ref`).
        ExprKind::MakeIface { data: IfaceData::Borrowed(p), .. } => place(p, true, f),
        ExprKind::MakeIface { data: IfaceData::Boxed(v), .. } => walk_expr(v, f),
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::RandInt(a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        ExprKind::Call(_, a) => args(a, f),
        ExprKind::CallVirtual { recv, args: a, .. } => {
            place(recv, false, f);
            args(a, f);
        }
        ExprKind::CallIndirect { callee, args: a, .. } => {
            place(callee, false, f);
            args(a, f);
        }
        ExprKind::StructLit(_, xs)
        | ExprKind::VariantLit(_, _, xs)
        | ExprKind::ArrayLit(xs)
        | ExprKind::VecLit(xs)
        | ExprKind::Math(_, xs)
        | ExprKind::Hash(_, xs)
        | ExprKind::Str(_, xs) => {
            for x in xs {
                walk_expr(x, f);
            }
        }
        ExprKind::Seq(stmts, v) => {
            walk(stmts, f);
            walk_expr(v, f);
        }
        ExprKind::If { cond, then_body, then_val, else_body, else_val } => {
            walk_expr(cond, f);
            walk(then_body, f);
            walk(else_body, f);
            for v in [then_val, else_val].into_iter().flatten() {
                walk_expr(v, f);
            }
        }
    }
}

/// The packed line of the first expression in `args` that has one (inside a `var` argument's
/// indexes too), or 0.
pub fn args_line(args: &[Arg]) -> u32 {
    let mut line = 0;
    for a in args {
        let mut look = |n: Node| {
            if let Node::Expr(e) = n {
                if line == 0 {
                    line = e.line;
                }
            }
        };
        match a {
            Arg::Value(e) => walk_expr(e, &mut look),
            Arg::Ref(p) => p.path.iter().filter_map(Proj::at).for_each(|at| walk_expr(at, &mut look)),
        }
    }
    line
}
