//! The descent that finds calls to inline inside statements, expressions and places.

use super::Inliner;
use crate::ir::*;

impl Inliner<'_> {
    pub(super) fn stmt(&mut self, s: &mut Stmt) {
        match s {
            Stmt::Let(_, e) | Stmt::Return(Some(e)) | Stmt::RandSeed(e) | Stmt::Fail { message: e, .. } | Stmt::Exit { code: e, .. } => self.expr(e),
            Stmt::Assign(p, e) | Stmt::ListAdd(p, e) | Stmt::SetLen(p, e) => {
                self.place(p);
                self.expr(e);
            }
            Stmt::List(_, p, xs) => {
                self.place(p);
                for x in xs {
                    self.expr(x);
                }
            }
            Stmt::If { cond, then_body, else_body } => {
                self.expr(cond);
                self.stmts(then_body);
                self.stmts(else_body);
            }
            // The condition runs every time round.
            Stmt::While { cond, body } => {
                self.depth += 1;
                self.expr(cond);
                self.stmts(body);
                self.depth -= 1;
            }
            Stmt::Assume { conds, body, .. } => {
                for (_, c) in conds {
                    self.expr(c);
                }
                self.stmts(body);
            }
            Stmt::Prove { body, .. } => self.stmts(body),
            Stmt::ForRange { start, end, body, .. } => {
                self.expr(start);
                self.expr(end);
                self.depth += 1;
                self.stmts(body);
                self.depth -= 1;
            }
            Stmt::Call(_, args, _) => self.args(args),
            Stmt::CallIndirect { callee, args, .. } => {
                self.place(callee);
                self.args(args);
            }
            Stmt::Print(parts) | Stmt::Write(parts) => self.print(parts),
            Stmt::Bind(_, p) | Stmt::Dup(p) | Stmt::FreeArena(p) | Stmt::DropGpu(p) | Stmt::PushArena(Some(p), _) | Stmt::CopyFnEnv(p) | Stmt::CopyIface(p) => {
                self.place(p)
            }
            Stmt::CopyItems(dst, src) => {
                self.place(src);
                self.place(dst);
            }
            Stmt::CallVirtual { recv, args, .. } => {
                self.place(recv);
                self.args(args);
            }
            Stmt::BindEnv { env, .. } => self.expr(env),
            Stmt::MapSet(p, k, v, _) => {
                self.place(p);
                self.expr(k);
                self.expr(v);
            }
            Stmt::MapUnlock(p, saved) => {
                self.place(p);
                self.expr(saved);
            }
            Stmt::ParFor { args, count, .. } => {
                self.args(args);
                self.expr(count);
            }
            Stmt::BindGlobal(..) | Stmt::PushArena(None, _) | Stmt::PopArena { .. } | Stmt::SetLine(_) | Stmt::Trap | Stmt::Break | Stmt::Continue | Stmt::Return(None) => {}
        }
    }

    pub(super) fn place(&mut self, p: &mut Place) {
        for at in p.path.iter_mut().filter_map(Proj::at_mut) {
            self.expr(at);
        }
    }

    fn args(&mut self, args: &mut [Arg]) {
        for a in args {
            match a {
                Arg::Value(e) => self.expr(e),
                Arg::Ref(p) => self.place(p),
            }
        }
    }

    fn print(&mut self, parts: &mut [PrintArg]) {
        for p in parts {
            match p {
                PrintArg::Value(e) => self.expr(e),
                PrintArg::Interp(inner) => self.print(inner),
                PrintArg::Str(_) => {}
            }
        }
    }

    pub(super) fn expr(&mut self, e: &mut Expr) {
        if e.line != 0 {
            self.line = e.line;
        }
        match &mut e.kind {
            ExprKind::Call(fi, args) if self.take(*fi, e.line) => {
                let (fi, args) = (*fi, std::mem::take(args));
                let (body, value) = self.expand(fi, args, e.line);
                let value = value.expect("a value call's callee ends in `return value`");
                e.kind = if body.is_empty() { value.kind } else { ExprKind::Seq(body, Box::new(value)) };
            }
            ExprKind::Format(parts) => self.print(parts),
            ExprKind::Load(p) | ExprKind::Tag(p) | ExprKind::Len(p) | ExprKind::Cap(p) | ExprKind::Move(p) | ExprKind::FnEnv(p) => self.place(p),
            ExprKind::Map(_, p, xs) | ExprKind::ReadBytes(p, xs) => {
                self.place(p);
                for x in xs {
                    self.expr(x);
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
            | ExprKind::LaneOf(x, _) => self.expr(x),
            ExprKind::MakeFn { env: EnvKind::Heap(env) | EnvKind::Stack(env), .. } => {
                for x in &mut env.values {
                    self.expr(x);
                }
            }
            ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::RandInt(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Call(_, args) => self.args(args),
            ExprKind::CallIndirect { callee, args, .. } => {
                self.place(callee);
                self.args(args);
            }
            ExprKind::MakeIface { data: IfaceData::Borrowed(p), .. } => self.place(p),
            ExprKind::MakeIface { data: IfaceData::Boxed(x), .. } => self.expr(x),
            ExprKind::CallVirtual { recv, args, .. } => {
                self.place(recv);
                self.args(args);
            }
            ExprKind::StructLit(_, xs)
            | ExprKind::VariantLit(_, _, xs)
            | ExprKind::ArrayLit(xs)
            | ExprKind::VecLit(xs)
            | ExprKind::Math(_, xs)
            | ExprKind::Hash(_, xs)
            | ExprKind::Str(_, xs) => {
                for x in xs {
                    self.expr(x);
                }
            }
            ExprKind::Seq(stmts, v) => {
                self.stmts(stmts);
                self.expr(v);
            }
            ExprKind::If { cond, then_body, then_val, else_body, else_val } => {
                self.expr(cond);
                self.stmts(then_body);
                self.stmts(else_body);
                for v in [then_val, else_val].into_iter().flatten() {
                    self.expr(v);
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
}
