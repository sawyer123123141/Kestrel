//! `from_bytes[T](b)` itself: a generated function per type, `__kestrel_from_bytes_N`, that checks
//! the 13-byte header (`KESB`, the version, the type's `fingerprint`), reads the value with the
//! type's read function (see `bytes_read`), and gives it when the value ends exactly where the
//! bytes do.

use kestrel_check::Tables;
use kestrel_syntax::ast;

use super::bytes::{fingerprint, helper, int, load, local, number, HEADER, MAGIC, VERSION};
use super::bytes_read::{cmp, len};
use super::helpers::{expr, Helpers};
use super::{kty, FnLowerer};
use crate::ir::*;

// The locals of a `from_bytes` function: the bytes, the byte after the value, and the value.
const F_SRC: LocalIdx = LocalIdx(0);
const F_AT: LocalIdx = LocalIdx(1);
const F_OUT: LocalIdx = LocalIdx(2);

impl Helpers {
    /// `__kestrel_from_bytes_N(src: list[u8]) -> T?`: the `T` that `src` (of type `list`) holds
    /// with its header, or `none`; `ret` is the `T?`.
    pub(crate) fn decode_fn(&mut self, ty: KTy, list: KTy, ret: KTy, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.from_bytes.get(&ty) {
            return f;
        }
        let idx = self.reserve();
        self.from_bytes.insert(ty, idx);
        let read_fn = self.bytes_read_fn(ty, list, t);
        let src = Place::local(F_SRC);
        let none = |cond: Expr| Stmt::If { cond, then_body: vec![Stmt::Return(Some(expr(ExprKind::NoneLit, ret)))], else_body: Vec::new() };
        let header = |at: i64, ty: KTy| expr(ExprKind::ReadBytes(src.clone(), vec![int(at, KTy::I64)]), ty);
        let args = vec![Arg::Value(load(src.clone(), list)), Arg::Value(int(HEADER, KTy::I64)), Arg::Ref(Place::local(F_OUT))];
        let body = vec![
            none(cmp(BinOp::Lt, len(&src), int(HEADER, KTy::I64))),
            none(cmp(BinOp::Ne, header(0, KTy::I32), int(MAGIC, KTy::I32))),
            none(cmp(BinOp::Ne, header(4, KTy::U8), int(VERSION, KTy::U8))),
            none(cmp(BinOp::Ne, header(5, KTy::I64), int(fingerprint(ty, t), KTy::I64))),
            Stmt::Let(F_AT, expr(ExprKind::Call(read_fn, args), KTy::I64)),
            // -1 or short of the end: wrong bytes, or bytes after the value.
            none(cmp(BinOp::Ne, local(F_AT), len(&src))),
            Stmt::Return(Some(expr(ExprKind::Some(Box::new(load(Place::local(F_OUT), ty))), ret))),
        ];
        let locals = vec![
            Local { name: "src".into(), ty: list, kind: LocalKind::RefParam },
            number("at"),
            Local { name: "out".into(), ty, kind: LocalKind::Addressed },
        ];
        self.funcs[(idx.0 - self.base) as usize] = helper(format!("__kestrel_from_bytes_{}", idx.0), vec![F_SRC], ret, locals, body, false);
        idx
    }
}

impl FnLowerer<'_> {
    /// `from_bytes[T](b)`, whose result has type `ret` (`T?`): a call to the type's `from_bytes`
    /// function.
    pub(crate) fn decode_call(&mut self, arg: &ast::Expr, ret: KTy) -> ExprKind {
        let KTy::Optional(e) = ret else { unreachable!("`from_bytes` gives an optional") };
        let ty = kty(self.t.elems[e.0 as usize]);
        let list = kty(self.ty(arg.id));
        let b = self.expr(arg);
        ExprKind::Call(self.helpers.decode_fn(ty, list, ret, self.t), vec![Arg::Value(b)])
    }
}
