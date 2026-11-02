//! `from_bytes[T](b)` itself: a generated function per type, `__kestrel_from_bytes_N`, that checks
//! the 13-byte header (`KESB`, the version, the type's `fingerprint`), reads the value with the
//! type's read function (see `bytes_read`), and gives it when the value ends exactly where the
//! bytes do. Otherwise it gives the reason (the language round, decision 13), made by one runtime
//! call on the way out: a valid read makes nothing it didn't make before.

use kestrel_check::Tables;
use kestrel_syntax::ast;

use super::bytes::{fingerprint, helper, int, load, local, number, wrapping, HEADER, MAGIC, VERSION};
use super::bytes_read::{cmp, len, why, CODES};
use super::helpers::{expr, Helpers};
use super::{kty, FnLowerer};
use crate::ir::*;

// The locals of a `from_bytes` function: the bytes, the byte after the value, the value, the
// reason code (0 while nothing is wrong) and the byte it names.
const F_SRC: LocalIdx = LocalIdx(0);
const F_AT: LocalIdx = LocalIdx(1);
const F_OUT: LocalIdx = LocalIdx(2);
const F_CODE: LocalIdx = LocalIdx(3);
const F_WHERE: LocalIdx = LocalIdx(4);

/// `if cond { then_body } else { else_body }`
fn either(cond: Expr, then_body: Vec<Stmt>, else_body: Vec<Stmt>) -> Stmt {
    Stmt::If { cond, then_body, else_body }
}

/// The reason code `code`, naming the byte `at`.
fn fails(code: i64, at: Expr) -> Vec<Stmt> {
    vec![Stmt::Assign(Place::local(F_CODE), int(code, KTy::I64)), Stmt::Assign(Place::local(F_WHERE), at)]
}

impl Helpers {
    /// `__kestrel_from_bytes_N(src: list[u8]) -> Result[T, string]`: the `T` that `src` (of type
    /// `list`) holds with its header, or why it doesn't; `ret` is the `Result[T, string]` and
    /// `name` how `T` reads in the reason.
    pub(crate) fn decode_fn(&mut self, ty: KTy, list: KTy, ret: KTy, name: &str, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.from_bytes.get(&ty) {
            return f;
        }
        let idx = self.reserve();
        self.from_bytes.insert(ty, idx);
        let read_fn = self.bytes_read_fn(ty, list, t);
        let KTy::Enum(result) = ret else { unreachable!("`from_bytes` gives a `Result`") };
        let variant = |name: &str| t.enums[result.0 as usize].variants.iter().position(|v| v.name == name).expect("a `Result` variant") as u32;
        let (ok, err) = (variant("Ok"), variant("Err"));
        let src = Place::local(F_SRC);
        let header = |at: i64, ty: KTy| expr(ExprKind::ReadBytes(src.clone(), vec![int(at, KTy::I64)]), ty);
        let args = vec![Arg::Value(load(src.clone(), list)), Arg::Value(int(HEADER, KTy::I64)), Arg::Ref(Place::local(F_OUT))];
        let n64 = |n: i64| int(n, KTy::I64);
        // A failed read gives `-(at * 16 + code)` (see `bytes_read::failed`).
        let failed = {
            let lost = || wrapping(BinOp::SubWrap, n64(0), local(F_AT));
            vec![
                Stmt::Assign(Place::local(F_CODE), expr(ExprKind::Binary(BinOp::BitAnd, Box::new(lost()), Box::new(n64(CODES - 1))), KTy::I64)),
                Stmt::Assign(Place::local(F_WHERE), expr(ExprKind::Binary(BinOp::Shr, Box::new(lost()), Box::new(n64(4))), KTy::I64)),
            ]
        };
        let read = vec![
            Stmt::Assign(Place::local(F_AT), expr(ExprKind::Call(read_fn, args), KTy::I64)),
            either(cmp(BinOp::Lt, local(F_AT), n64(0)), failed, vec![either(cmp(BinOp::Ne, local(F_AT), len(&src)), fails(why::TRAILING, local(F_AT)), Vec::new())]),
        ];
        // Each header check reads only bytes the ones before it found there.
        let version = expr(ExprKind::Cast(Box::new(header(4, KTy::U8))), KTy::I64);
        let shape = either(cmp(BinOp::Ne, header(5, KTy::I64), n64(fingerprint(ty, t))), fails(why::SHAPE, n64(0)), read);
        let version = either(cmp(BinOp::Ne, header(4, KTy::U8), int(VERSION, KTy::U8)), fails(why::VERSION, version), vec![shape]);
        let magic = either(cmp(BinOp::Ne, header(0, KTy::I32), int(MAGIC, KTy::I32)), fails(why::MAGIC, n64(0)), vec![version]);
        let reason = expr(ExprKind::Str(StrOp::BytesReason, vec![local(F_CODE), local(F_WHERE), len(&src), expr(ExprKind::StrLit(name.to_string()), KTy::Str)]), KTy::Str);
        let made = |v: u32, field: Expr| Stmt::Return(Some(expr(ExprKind::VariantLit(result, v, vec![field]), ret)));
        let body = vec![
            Stmt::Let(F_CODE, n64(0)),
            Stmt::Let(F_WHERE, n64(0)),
            Stmt::Let(F_AT, n64(0)),
            either(cmp(BinOp::Lt, len(&src), n64(HEADER)), fails(why::TOO_SHORT, n64(0)), vec![magic]),
            either(cmp(BinOp::Ne, local(F_CODE), n64(0)), vec![made(err, reason)], Vec::new()),
            made(ok, load(Place::local(F_OUT), ty)),
        ];
        let locals = vec![
            Local { name: "src".into(), ty: list, kind: LocalKind::RefParam },
            number("at"),
            Local { name: "out".into(), ty, kind: LocalKind::Addressed },
            number("code"),
            number("where"),
        ];
        self.funcs[(idx.0 - self.base) as usize] = helper(format!("__kestrel_from_bytes_{}", idx.0), vec![F_SRC], ret, locals, body, false);
        idx
    }
}

impl FnLowerer<'_> {
    /// `from_bytes[T](b)`, the expression `e` (a `Result[T, string]`) reading the bytes `arg`: a
    /// call to the type's `from_bytes` function.
    pub(crate) fn decode_call(&mut self, e: &ast::Expr, arg: &ast::Expr) -> ExprKind {
        let ret = self.ty(e.id);
        let (read, _) = self.t.result_parts(ret).expect("`from_bytes` gives a `Result`");
        let name = self.t.ty_name(read);
        let list = kty(self.ty(arg.id));
        let b = self.expr(arg);
        ExprKind::Call(self.helpers.decode_fn(kty(read), list, kty(ret), &name, self.t), vec![Arg::Value(b)])
    }
}
