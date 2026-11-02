//! Renames a function body's locals into another function, for inlining.

use crate::ir::*;

/// How a callee local maps into the caller.
#[derive(Clone)]
pub(crate) enum Subst {
    Local(LocalIdx),
    /// A by-pointer parameter: the argument's place.
    Place(Place),
}

fn rename_local(l: &mut LocalIdx, subst: &[Subst]) {
    match &subst[l.0 as usize] {
        Subst::Local(n) => *l = *n,
        Subst::Place(_) => unreachable!("a by-pointer parameter is only used as a place"),
    }
}

fn rename_place(p: &mut Place, subst: &[Subst]) {
    for at in p.path.iter_mut().filter_map(Proj::at_mut) {
        rename_expr(at, subst);
    }
    match &subst[p.root.0 as usize] {
        Subst::Local(n) => p.root = *n,
        Subst::Place(base) => {
            let mut path = base.path.clone();
            path.append(&mut p.path);
            *p = Place { root: base.root, path };
        }
    }
}

pub(crate) fn rename_stmts(stmts: &mut [Stmt], subst: &[Subst]) {
    for s in stmts {
        match s {
            Stmt::Let(l, e) => {
                rename_local(l, subst);
                rename_expr(e, subst);
            }
            Stmt::Return(Some(e)) | Stmt::RandSeed(e) | Stmt::Fail { message: e, .. } | Stmt::Exit { code: e, .. } => rename_expr(e, subst),
            Stmt::Assign(p, e) | Stmt::ListAdd(p, e) | Stmt::SetLen(p, e) => {
                rename_place(p, subst);
                rename_expr(e, subst);
            }
            Stmt::List(_, p, xs) => {
                rename_place(p, subst);
                for x in xs {
                    rename_expr(x, subst);
                }
            }
            Stmt::If { cond, then_body, else_body } => {
                rename_expr(cond, subst);
                rename_stmts(then_body, subst);
                rename_stmts(else_body, subst);
            }
            Stmt::While { cond, body } => {
                rename_expr(cond, subst);
                rename_stmts(body, subst);
            }
            Stmt::Assume { conds, body, .. } => {
                for (_, c) in conds {
                    rename_expr(c, subst);
                }
                rename_stmts(body, subst);
            }
            Stmt::Prove { body, .. } => rename_stmts(body, subst),
            Stmt::ForRange { var, start, end, body } => {
                rename_local(var, subst);
                rename_expr(start, subst);
                rename_expr(end, subst);
                rename_stmts(body, subst);
            }
            Stmt::Call(_, args, _) => rename_args(args, subst),
            Stmt::CallIndirect { callee, args, .. } => {
                rename_place(callee, subst);
                rename_args(args, subst);
            }
            Stmt::Print(parts) | Stmt::Write(parts) => rename_print(parts, subst),
            Stmt::Bind(l, p) => {
                rename_local(l, subst);
                rename_place(p, subst);
            }
            Stmt::BindGlobal(l, _) => rename_local(l, subst),
            Stmt::BindEnv { local, env, .. } => {
                rename_local(local, subst);
                rename_expr(env, subst);
            }
            Stmt::CopyFnEnv(p) | Stmt::CopyIface(p) => rename_place(p, subst),
            Stmt::CallVirtual { recv, args, .. } => {
                rename_place(recv, subst);
                rename_args(args, subst);
            }
            Stmt::Dup(p) | Stmt::FreeArena(p) | Stmt::DropGpu(p) | Stmt::PushArena(Some(p), _) => rename_place(p, subst),
            Stmt::CopyItems(dst, src) => {
                rename_place(dst, subst);
                rename_place(src, subst);
            }
            Stmt::MapSet(p, k, v, _) => {
                rename_place(p, subst);
                rename_expr(k, subst);
                rename_expr(v, subst);
            }
            Stmt::MapUnlock(p, saved) => {
                rename_place(p, subst);
                rename_expr(saved, subst);
            }
            Stmt::ParFor { args, count, .. } => {
                rename_args(args, subst);
                rename_expr(count, subst);
            }
            Stmt::PushArena(None, _) | Stmt::PopArena { .. } | Stmt::SetLine(_) | Stmt::Trap | Stmt::Break | Stmt::Continue | Stmt::Return(None) => {}
        }
    }
}

fn rename_args(args: &mut [Arg], subst: &[Subst]) {
    for a in args {
        match a {
            Arg::Value(e) => rename_expr(e, subst),
            Arg::Ref(p) => rename_place(p, subst),
        }
    }
}

fn rename_print(parts: &mut [PrintArg], subst: &[Subst]) {
    for p in parts {
        match p {
            PrintArg::Value(e) => rename_expr(e, subst),
            PrintArg::Interp(inner) => rename_print(inner, subst),
            PrintArg::Str(_) => {}
        }
    }
}

pub(crate) fn rename_expr(e: &mut Expr, subst: &[Subst]) {
    match &mut e.kind {
        ExprKind::Format(parts) => rename_print(parts, subst),
        ExprKind::Load(p) | ExprKind::Tag(p) | ExprKind::Len(p) | ExprKind::Cap(p) | ExprKind::Move(p) | ExprKind::FnEnv(p) => rename_place(p, subst),
        ExprKind::Map(_, p, xs) | ExprKind::ReadBytes(p, xs) => {
            rename_place(p, subst);
            for x in xs {
                rename_expr(x, subst);
            }
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
            rename_expr(x, subst)
        }
        ExprKind::MakeFn { env: EnvKind::Heap(env) | EnvKind::Stack(env), .. } => {
            for x in &mut env.values {
                rename_expr(x, subst);
            }
        }
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::RandInt(a, b) => {
            rename_expr(a, subst);
            rename_expr(b, subst);
        }
        ExprKind::Call(_, args) => rename_args(args, subst),
        ExprKind::CallIndirect { callee, args, .. } => {
            rename_place(callee, subst);
            rename_args(args, subst);
        }
        // A by-pointer parameter's place is spliced in front of the borrowed path.
        ExprKind::MakeIface { data: IfaceData::Borrowed(p), .. } => rename_place(p, subst),
        ExprKind::MakeIface { data: IfaceData::Boxed(x), .. } => rename_expr(x, subst),
        ExprKind::CallVirtual { recv, args, .. } => {
            rename_place(recv, subst);
            rename_args(args, subst);
        }
        ExprKind::StructLit(_, xs)
        | ExprKind::VariantLit(_, _, xs)
        | ExprKind::ArrayLit(xs)
        | ExprKind::VecLit(xs)
        | ExprKind::Math(_, xs)
        | ExprKind::Hash(_, xs)
        | ExprKind::Str(_, xs) => {
            for x in xs {
                rename_expr(x, subst);
            }
        }
        ExprKind::Seq(stmts, v) => {
            rename_stmts(stmts, subst);
            rename_expr(v, subst);
        }
        ExprKind::If { cond, then_body, then_val, else_body, else_val } => {
            rename_expr(cond, subst);
            rename_stmts(then_body, subst);
            rename_stmts(else_body, subst);
            for v in [then_val, else_val].into_iter().flatten() {
                rename_expr(v, subst);
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
