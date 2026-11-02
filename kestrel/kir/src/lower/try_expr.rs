//! `try r` (the language round, decision 9), lowered as the hand-written `match` with a `return`
//! is: a tag test on `r` (read in place when it is a place, from a temporary otherwise), the `Ok`
//! value read where it is, and on an error a `return` of the function's `Err` built from the
//! payload, which moves (nothing is copied when `r` dies with the function), leaving every block
//! as `return` does. On an optional the value, or a `return` of `none`. And `let x = r else why
//! { ... }`, which reads a `Result` the same way.

use kestrel_check::Ty;
use kestrel_syntax::ast;

use super::helpers::owning;
use super::{kty, FnLowerer};
use crate::ir::*;

/// How an operand of `try` (or of `let ... else`) holds its value: the variant numbers of a
/// `Result`'s `Ok` and `Err`, or an optional.
#[derive(Clone, Copy)]
pub(crate) enum Unwrap {
    Result { ok: u32, err: u32 },
    Optional,
}

impl Unwrap {
    /// The place of the value inside `place`.
    pub(crate) fn value(self, place: &Place) -> Place {
        let mut p = place.clone();
        p.path.push(match self {
            Unwrap::Result { ok, .. } => Proj::Variant(ok, 0),
            Unwrap::Optional => Proj::Payload,
        });
        p
    }
}

impl<'a> FnLowerer<'a> {
    /// How a value of type `ty` (a `Result` or an optional) holds its value.
    pub(crate) fn unwrap_of(&self, ty: Ty) -> Unwrap {
        if let Ty::Enum(id) = ty {
            let variants = &self.t.enm(id).variants;
            let at = |name: &str| variants.iter().position(|v| v.name == name).expect("a `Result` has `Ok` and `Err`") as u32;
            return Unwrap::Result { ok: at("Ok"), err: at("Err") };
        }
        Unwrap::Optional
    }

    /// The test that `place` (unwrapped as `how`) has a value.
    pub(crate) fn has_value(&self, how: Unwrap, place: &Place, line: u32) -> Expr {
        match how {
            Unwrap::Result { ok, .. } => self.tag_is(place, ok, line),
            Unwrap::Optional => {
                let tag = Expr { kind: ExprKind::Tag(place.clone()), ty: KTy::U8, line };
                let zero = Expr { kind: ExprKind::Int(0), ty: KTy::U8, line };
                Expr { kind: ExprKind::Binary(BinOp::Ne, Box::new(tag), Box::new(zero)), ty: KTy::Bool, line }
            }
        }
    }

    /// The test that `place` (unwrapped as `how`) has no value: it is `none`, or an error.
    pub(crate) fn has_no_value(&self, how: Unwrap, place: &Place, line: u32) -> Expr {
        match how {
            Unwrap::Result { err, .. } => self.tag_is(place, err, line),
            Unwrap::Optional => self.tag_is(place, 0, line),
        }
    }

    /// `try inner` used as a value: the value inside `inner`, or a return.
    pub(crate) fn try_value(&mut self, e: &ast::Expr, inner: &ast::Expr) -> ExprKind {
        let (ty, line) = (kty(self.ty(e.id)), self.line(e.span));
        let mut pre = Vec::new();
        let (how, place, moved) = self.tried(inner, &mut pre);
        let at = how.value(&place);
        let value = Expr { kind: if moved { ExprKind::Move(at) } else { ExprKind::Load(at) }, ty, line };
        let cond = self.has_value(how, &place, line);
        let leave = self.try_leave(e, how, &place, line);
        let kind = ExprKind::If { cond: Box::new(cond), then_body: Vec::new(), then_val: Some(Box::new(value)), else_body: leave, else_val: None };
        Self::with_pre(pre, kind, ty, line)
    }

    /// `try inner` as a statement: the value inside `inner` is dropped (an owning one is freed).
    pub(crate) fn try_stmt(&mut self, e: &ast::Expr, inner: &ast::Expr, out: &mut Vec<Stmt>) {
        let (ty, line) = (kty(self.ty(e.id)), self.line(e.span));
        let (how, place, _) = self.tried(inner, out);
        let cond = self.has_value(how, &place, line);
        let then_body = if owning(ty, self.t) { self.helpers.drop_stmts(how.value(&place), ty, self.t) } else { Vec::new() };
        let else_body = self.try_leave(e, how, &place, line);
        out.push(Stmt::If { cond, then_body, else_body });
    }

    /// Where the operand of `try` is read: the place it is, or else a temporary it goes into
    /// first. Also whether the value inside moves out of a temporary: an owning one, which is then
    /// a new value for whoever takes it (see `temps`).
    fn tried(&mut self, inner: &ast::Expr, pre: &mut Vec<Stmt>) -> (Unwrap, Place, bool) {
        let ty = self.ty(inner.id);
        let how = self.unwrap_of(ty);
        let holds_owning = matches!(ty, Ty::Optional(elem) if owning(kty(self.t.elem(elem)), self.t));
        if !holds_owning {
            return (how, self.place_or_temp(inner, pre, true), false);
        }
        // An owning variable moved (the checker said so), or a new owning value: its value is
        // handed on, so nothing drops the temporary.
        let v = self.expr(inner);
        let tmp = self.temp(v.ty);
        pre.push(Stmt::Let(tmp, v));
        self.scratch_results.insert(tmp);
        (how, Place::local(tmp), true)
    }

    /// The statements that leave the function from a `try` whose operand, at `place`, has no
    /// value: a `return` of the function's `Err` holding the error, or of `none`.
    fn try_leave(&mut self, e: &ast::Expr, how: Unwrap, place: &Place, line: u32) -> Vec<Stmt> {
        let ret = self.sub(self.t.try_rets[&e.id]);
        let value = match how {
            Unwrap::Result { err, .. } => {
                let Ty::Enum(id) = ret else { unreachable!("the checker made `try` on a `Result` return a `Result`") };
                let (_, error_ty) = self.t.result_parts(ret).expect("a `Result`");
                let mut at = place.clone();
                at.path.push(Proj::Variant(err, 0));
                let error = Expr { kind: ExprKind::Load(at), ty: kty(error_ty), line };
                let error = self.owned(error, true);
                let Unwrap::Result { err: theirs, .. } = self.unwrap_of(ret) else { unreachable!("a `Result`") };
                Expr { kind: ExprKind::VariantLit(EnumIdx(id.0), theirs, vec![error]), ty: kty(ret), line }
            }
            Unwrap::Optional => Expr { kind: ExprKind::NoneLit, ty: kty(ret), line },
        };
        let mut out = Vec::new();
        self.return_lowered(Some(value), &mut out);
        out
    }
}
