//! Walks every expression of a block or expression, so a pass that only needs to see them
//! (find the type applications, count the calls) doesn't repeat the traversal.

use crate::ast::*;

/// Calls `f` on every expression in `block`, in source order: each statement's expressions, and
/// inside each expression its own node first, then its parts. Closure bodies, interpolation
/// parts, `if` and `match` blocks, `let ... else` blocks and `@assume` conditions are included.
pub fn each_expr(block: &Block, f: &mut dyn FnMut(&Expr)) {
    block.stmts.iter().for_each(|s| stmt(s, f));
}

/// Calls `f` on `e` and then on every expression inside it, in source order.
pub fn each_expr_in(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) | ExprKind::Ident(_) => {}
        ExprKind::TypeApply { .. } => {}
        ExprKind::Interp(parts) => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    each_expr_in(x, f);
                }
            }
        }
        ExprKind::Unary { expr, .. } | ExprKind::Try(expr) => each_expr_in(expr, f),
        ExprKind::Binary { lhs, rhs, .. } => {
            each_expr_in(lhs, f);
            each_expr_in(rhs, f);
        }
        ExprKind::Call { args, .. } | ExprKind::TypeCall { args, .. } => each_arg(args, f),
        ExprKind::If { cond, then_block, else_block } => {
            each_expr_in(cond, f);
            each_expr(then_block, f);
            if let Some(b) = else_block {
                each_expr(b, f);
            }
        }
        ExprKind::Field { base, .. } => each_expr_in(base, f),
        ExprKind::MethodCall { receiver, args, .. } => {
            each_expr_in(receiver, f);
            each_arg(args, f);
        }
        ExprKind::Match { scrutinee, arms } => {
            each_expr_in(scrutinee, f);
            arms.iter().for_each(|a| each_expr(&a.body, f));
        }
        ExprKind::Index { base, index } => {
            each_expr_in(base, f);
            each_expr_in(index, f);
        }
        ExprKind::Brackets { base, index, .. } => {
            each_expr_in(base, f);
            if let Some(i) = index {
                each_expr_in(i, f);
            }
        }
        ExprKind::ArrayLit(items) => items.iter().for_each(|i| each_expr_in(i, f)),
        ExprKind::MapLit(entries) => {
            for (k, v) in entries {
                each_expr_in(k, f);
                each_expr_in(v, f);
            }
        }
        ExprKind::CallValue { callee, args } => {
            each_expr_in(callee, f);
            each_arg(args, f);
        }
        ExprKind::Closure(c) => each_expr(&c.body, f),
    }
}

fn each_arg(args: &[Arg], f: &mut dyn FnMut(&Expr)) {
    args.iter().for_each(|a| each_expr_in(&a.value, f));
}

fn stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match s {
        Stmt::Let { value, else_block, .. } => {
            each_expr_in(value, f);
            if let Some(b) = else_block {
                each_expr(b, f);
            }
        }
        Stmt::Assign { target, value, .. } => {
            each_expr_in(target, f);
            each_expr_in(value, f);
        }
        Stmt::While { cond, body, .. } => {
            each_expr_in(cond, f);
            each_expr(body, f);
        }
        Stmt::ForRange { start, end, body, .. } => {
            each_expr_in(start, f);
            each_expr_in(end, f);
            each_expr(body, f);
        }
        Stmt::ForIn { iter, body, .. } => {
            each_expr_in(iter, f);
            each_expr(body, f);
        }
        Stmt::Arena { body, .. } => each_expr(body, f),
        Stmt::In { arena, body, .. } => {
            each_expr_in(arena, f);
            each_expr(body, f);
        }
        Stmt::Return { value: Some(v), .. } | Stmt::Expr(v) => each_expr_in(v, f),
        Stmt::Assume { cond, .. } => each_expr_in(cond, f),
        Stmt::Return { value: None, .. } | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Remove { .. } => {}
    }
}
