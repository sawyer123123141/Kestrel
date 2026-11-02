//! Rewrites KIR in place. A pass implements `VisitMut`, overrides the methods for the nodes it
//! changes, and calls the matching `walk_*` function to carry on into everything else.

use crate::ir::*;

pub trait VisitMut {
    /// A statement list. Override it to replace a statement with several.
    fn visit_stmts(&mut self, list: &mut Vec<Stmt>) {
        for s in list.iter_mut() {
            self.visit_stmt(s);
        }
    }

    fn visit_stmt(&mut self, s: &mut Stmt) {
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &mut Expr) {
        walk_expr(self, e);
    }

    /// A place: its index expressions.
    fn visit_place(&mut self, p: &mut Place) {
        walk_place(self, p);
    }
}

/// Visits what `s` holds, in the order codegen evaluates it.
pub fn walk_stmt<V: VisitMut + ?Sized>(v: &mut V, s: &mut Stmt) {
    match s {
        Stmt::Let(_, e) | Stmt::Return(Some(e)) | Stmt::RandSeed(e) | Stmt::Fail { message: e, .. } | Stmt::Exit { code: e, .. } => v.visit_expr(e),
        Stmt::Assign(p, e) | Stmt::ListAdd(p, e) | Stmt::SetLen(p, e) => {
            v.visit_expr(e);
            v.visit_place(p);
        }
        Stmt::List(_, p, xs) => {
            for x in xs {
                v.visit_expr(x);
            }
            v.visit_place(p);
        }
        Stmt::If { cond, then_body, else_body } => {
            v.visit_expr(cond);
            v.visit_stmts(then_body);
            v.visit_stmts(else_body);
        }
        Stmt::While { cond, body } => {
            v.visit_expr(cond);
            v.visit_stmts(body);
        }
        Stmt::Assume { conds, body, .. } => {
            for (_, c) in conds {
                v.visit_expr(c);
            }
            v.visit_stmts(body);
        }
        Stmt::Prove { body, .. } => v.visit_stmts(body),
        Stmt::ForRange { start, end, body, .. } => {
            v.visit_expr(start);
            v.visit_expr(end);
            v.visit_stmts(body);
        }
        Stmt::Call(_, args, _) => walk_args(v, args),
        Stmt::CallIndirect { callee, args, .. } => {
            v.visit_place(callee);
            walk_args(v, args);
        }
        Stmt::Print(parts) | Stmt::Write(parts) => walk_print(v, parts),
        Stmt::Bind(_, p) | Stmt::Dup(p) | Stmt::FreeArena(p) | Stmt::DropGpu(p) | Stmt::PushArena(Some(p), _) | Stmt::CopyFnEnv(p) | Stmt::CopyIface(p) => {
            v.visit_place(p)
        }
        Stmt::CallVirtual { recv, args, .. } => {
            v.visit_place(recv);
            walk_args(v, args);
        }
        Stmt::CopyItems(dst, src) => {
            v.visit_place(src);
            v.visit_place(dst);
        }
        Stmt::BindEnv { env, .. } => v.visit_expr(env),
        Stmt::MapSet(p, k, val, _) => {
            v.visit_expr(k);
            v.visit_expr(val);
            v.visit_place(p);
        }
        Stmt::MapUnlock(p, saved) => {
            v.visit_expr(saved);
            v.visit_place(p);
        }
        Stmt::ParFor { args, count, .. } => {
            walk_args(v, args);
            v.visit_expr(count);
        }
        Stmt::BindGlobal(..) | Stmt::PushArena(None, _) | Stmt::PopArena { .. } | Stmt::SetLine(_) | Stmt::Trap | Stmt::Break | Stmt::Continue | Stmt::Return(None) => {}
    }
}

/// Visits the expressions, statement lists and places directly inside `e`.
pub fn walk_expr<V: VisitMut + ?Sized>(v: &mut V, e: &mut Expr) {
    match &mut e.kind {
        ExprKind::Format(parts) => walk_print(v, parts),
        ExprKind::Load(p) | ExprKind::Tag(p) | ExprKind::Len(p) | ExprKind::Cap(p) | ExprKind::Move(p) | ExprKind::FnEnv(p) => v.visit_place(p),
        ExprKind::Map(_, p, xs) | ExprKind::ReadBytes(p, xs) => {
            for x in xs {
                v.visit_expr(x);
            }
            v.visit_place(p);
        }
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
            v.visit_expr(x)
        }
        ExprKind::MakeFn { env: EnvKind::Heap(env) | EnvKind::Stack(env), .. } => {
            for x in &mut env.values {
                v.visit_expr(x);
            }
        }
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::RandInt(a, b) => {
            v.visit_expr(a);
            v.visit_expr(b);
        }
        ExprKind::Call(_, args) => walk_args(v, args),
        ExprKind::CallIndirect { callee, args, .. } => {
            v.visit_place(callee);
            walk_args(v, args);
        }
        ExprKind::MakeIface { data: IfaceData::Borrowed(p), .. } => v.visit_place(p),
        ExprKind::MakeIface { data: IfaceData::Boxed(x), .. } => v.visit_expr(x),
        ExprKind::CallVirtual { recv, args, .. } => {
            v.visit_place(recv);
            walk_args(v, args);
        }
        ExprKind::StructLit(_, xs)
        | ExprKind::VariantLit(_, _, xs)
        | ExprKind::ArrayLit(xs)
        | ExprKind::VecLit(xs)
        | ExprKind::Math(_, xs)
        | ExprKind::Hash(_, xs)
        | ExprKind::Str(_, xs) => {
            for x in xs {
                v.visit_expr(x);
            }
        }
        ExprKind::Seq(stmts, value) => {
            v.visit_stmts(stmts);
            v.visit_expr(value);
        }
        ExprKind::If { cond, then_body, then_val, else_body, else_val } => {
            v.visit_expr(cond);
            v.visit_stmts(then_body);
            if let Some(x) = then_val {
                v.visit_expr(x);
            }
            v.visit_stmts(else_body);
            if let Some(x) = else_val {
                v.visit_expr(x);
            }
        }
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
    }
}

pub fn walk_place<V: VisitMut + ?Sized>(v: &mut V, p: &mut Place) {
    for at in p.path.iter_mut().filter_map(Proj::at_mut) {
        v.visit_expr(at);
    }
}

fn walk_args<V: VisitMut + ?Sized>(v: &mut V, args: &mut [Arg]) {
    for a in args {
        match a {
            Arg::Value(e) => v.visit_expr(e),
            Arg::Ref(p) => v.visit_place(p),
        }
    }
}

fn walk_print<V: VisitMut + ?Sized>(v: &mut V, parts: &mut [PrintArg]) {
    for p in parts {
        match p {
            PrintArg::Value(e) => v.visit_expr(e),
            PrintArg::Interp(inner) => walk_print(v, inner),
            PrintArg::Str(_) => {}
        }
    }
}
