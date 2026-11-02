//! The scratch arenas of `sort_by`'s compare and fill functions: taken out again when the key
//! can't build memory (see `sort`).

use std::collections::HashSet;

use crate::ir::*;
use crate::visit::{walk, Node};

/// Takes the scratch arenas out of each `sort_by` compare or fill function whose key, a literal or
/// a named function, can't build anything: `idle` pairs a compare or fill function with the code
/// it calls (the literal's code, or the function's thunk).
pub(super) fn strip_idle_scratch(funcs: &mut [Func], idle: &[(FuncIdx, FuncIdx)]) {
    for &(compare, code) in idle {
        if !builds_memory(funcs, code, &mut HashSet::new()) {
            strip(&mut funcs[compare.0 as usize].body);
        }
    }
}

/// Takes the scratch arenas out of `body`: a compare function's around its body, a fill
/// function's inside its loop.
fn strip(body: &mut Vec<Stmt>) {
    body.retain(|s| !matches!(s, Stmt::PushArena(None, _) | Stmt::PopArena { .. }));
    for s in body {
        if let Stmt::ForRange { body, .. } = s {
            strip(body);
        }
    }
}

/// Whether running function `f` may build memory in the current arena: it makes a string, array,
/// list entry, map entry, copy or environment, or calls something that does (or something unknown,
/// like a function value or a built-in function).
fn builds_memory(funcs: &[Func], f: FuncIdx, seen: &mut HashSet<u32>) -> bool {
    if !seen.insert(f.0) {
        return false;
    }
    let func = &funcs[f.0 as usize];
    if func.native.is_some() {
        return true;
    }
    let mut calls = Vec::new();
    let mut builds = false;
    walk(&func.body, &mut |n| match n {
        Node::Stmt(s) => builds |= stmt_builds(s, &mut calls),
        Node::Expr(e) => builds |= expr_builds(e, &mut calls),
        Node::Place(..) => {}
    });
    builds || calls.into_iter().any(|g| builds_memory(funcs, g, seen))
}

/// Whether the statement itself builds memory in the current arena (the statements and
/// expressions inside it are walked on their own). A direct call goes into `calls`. Every kind is
/// named, so a new one has to be sorted here.
fn stmt_builds(s: &Stmt, calls: &mut Vec<FuncIdx>) -> bool {
    match s {
        Stmt::Dup(_) | Stmt::ListAdd(..) | Stmt::MapSet(..) | Stmt::CopyFnEnv(_) | Stmt::ParFor { .. } | Stmt::CallIndirect { .. } => true,
        // A call through a table may run any method; a copy allocates the concrete value.
        Stmt::CallVirtual { .. } | Stmt::CopyIface(_) => true,
        Stmt::List(op, ..) => match op {
            ListOp::Insert | ListOp::Reserve => true,
            // (The runtime sorts in buffers of its own.)
            ListOp::RemoveAt | ListOp::Clear | ListOp::Compact | ListOp::Sort | ListOp::SortBy(_) | ListOp::WriteBytes | ListOp::WriteCount => false,
        },
        Stmt::Call(g, _, _) => {
            calls.push(*g);
            false
        }
        Stmt::Let(..)
        | Stmt::Assign(..)
        | Stmt::If { .. }
        | Stmt::Assume { .. }
        | Stmt::Prove { .. }
        | Stmt::While { .. }
        | Stmt::ForRange { .. }
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Return(_)
        | Stmt::Print(_)
        | Stmt::Write(_)
        | Stmt::RandSeed(_)
        | Stmt::SetLen(..)
        // (It copies into a buffer that is already there.)
        | Stmt::CopyItems(..)
        | Stmt::Bind(..)
        | Stmt::BindGlobal(..)
        | Stmt::BindEnv { .. }
        | Stmt::PushArena(..)
        | Stmt::PopArena { .. }
        | Stmt::FreeArena(_)
        | Stmt::DropGpu(_)
        | Stmt::MapUnlock(..)
        | Stmt::SetLine(_)
        | Stmt::Trap
        | Stmt::Fail { .. }
        | Stmt::Exit { .. } => false,
    }
}

/// Whether the expression itself builds memory in the current arena, as `stmt_builds` does for a
/// statement.
fn expr_builds(e: &Expr, calls: &mut Vec<FuncIdx>) -> bool {
    match &e.kind {
        ExprKind::Format(_) | ExprKind::ArrayNew(_) | ExprKind::ArrayLit(_) | ExprKind::NewArena | ExprKind::StaticArena | ExprKind::EnvDup(_) | ExprKind::BlockDup { .. } | ExprKind::CallIndirect { .. } => {
            true
        }
        // Any method may build memory; a boxed value is allocated in the current arena.
        ExprKind::CallVirtual { .. } => true,
        ExprKind::MakeIface { data, .. } => match data {
            IfaceData::Boxed(_) => true,
            IfaceData::Borrowed(_) => false,
        },
        ExprKind::MakeFn { env, .. } => match env {
            EnvKind::Heap(_) => true,
            EnvKind::None | EnvKind::Stack(_) => false,
        },
        ExprKind::Str(op, _) => match op {
            StrOp::At | StrOp::Piece | StrOp::Slice | StrOp::Trim | StrOp::Upper | StrOp::Lower | StrOp::Join | StrOp::Split | StrOp::Hex | StrOp::BytesReason => true,
            StrOp::Chars | StrOp::CharLen | StrOp::Find | StrOp::Starts | StrOp::Cmp | StrOp::Ends | StrOp::ToInt | StrOp::ToFloat => false,
        },
        ExprKind::Map(op, ..) => match op {
            MapOp::Get | MapOp::Has | MapOp::Remove | MapOp::Used | MapOp::Live | MapOp::Lock | MapOp::CopyInto => false,
        },
        // A string read from bytes is made in the current arena.
        ExprKind::ReadBytes(..) => e.ty == KTy::Str,
        ExprKind::Call(g, _) => {
            calls.push(*g);
            false
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::StrLit(_)
        | ExprKind::ListNew
        | ExprKind::MapNew
        | ExprKind::Len(_)
        | ExprKind::Cap(_)
        | ExprKind::Some(_)
        | ExprKind::NoneLit
        | ExprKind::Move(_)
        | ExprKind::Load(_)
        | ExprKind::Unary(..)
        | ExprKind::Binary(..)
        | ExprKind::And(..)
        | ExprKind::Or(..)
        | ExprKind::Cast(_)
        | ExprKind::Proven(_)
        | ExprKind::Math(..)
        | ExprKind::Hash(..)
        | ExprKind::RandFloat { .. }
        | ExprKind::RandInt(..)
        | ExprKind::FnRef(_)
        | ExprKind::FnEnv(_)
        | ExprKind::StructLit(..)
        | ExprKind::VariantLit(..)
        | ExprKind::Tag(_)
        | ExprKind::FieldOf(..)
        | ExprKind::VecLit(_)
        | ExprKind::Splat(_)
        | ExprKind::Shuffle(..)
        | ExprKind::LaneOf(..)
        | ExprKind::Seq(..)
        | ExprKind::If { .. } => false,
    }
}
