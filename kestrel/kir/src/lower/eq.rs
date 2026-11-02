//! `==` and `!=` on two values of one struct or enum (decision 19), and the item compare behind
//! `has` and `find`. Fields compare in declaration order, each as `==` compares it: numbers by
//! value (a NaN is unequal to everything, `-0.0 == 0.0`), bools, `vec4` and `ivec4` by value,
//! strings by their text, structs field by field, and an enum by its tag first, then only that
//! variant's fields, so another variant's bytes are never read.
//!
//! A struct of numbers and bools (nested structs too, the vectors among them) compares inline:
//! one `==` per field, joined by `and`, as it would be written by hand. Any other type calls
//! `__kestrel_eq_N(a: T, b: T) -> bool`, one per type, generated as the `checksum` helpers are:
//! pure, with no checks and no allocation, and small enough to inline. The checker refuses `==`
//! on values holding a collection, an optional, an interface or function value, an arena or a
//! GPU object, so none reaches here.

use kestrel_check::Tables;
use kestrel_syntax::ast;

use super::helpers::{expr, tag_is, with, Helpers};
use super::vector::{may_write, rereadable};
use super::{kty, FnLowerer};
use crate::ir::*;

/// The two values' locals in an equality function.
const A: LocalIdx = LocalIdx(0);
const B: LocalIdx = LocalIdx(1);

/// A field inside a struct, as the field numbers that lead to it, and its type.
type Leaf = (Vec<u32>, KTy);

/// Whether one `==` compares a value of `ty` whole.
fn is_word(ty: KTy) -> bool {
    matches!(ty, KTy::Bool | KTy::U8 | KTy::I32 | KTy::I64 | KTy::F32 | KTy::F64 | KTy::F32X4 | KTy::I32X4 | KTy::Str)
}

/// Adds the number and bool fields of a value of `ty`, at `path`, to `out`. False when some
/// field is anything else (a string, a SIMD vector, an enum).
fn scalar_leaves(ty: KTy, t: &Tables, path: &mut Vec<u32>, out: &mut Vec<Leaf>) -> bool {
    match ty {
        KTy::Bool | KTy::U8 | KTy::I32 | KTy::I64 | KTy::F32 | KTy::F64 => {
            out.push((path.clone(), ty));
            true
        }
        KTy::Struct(s) => t.structs[s.0 as usize].fields.iter().enumerate().all(|(i, f)| {
            path.push(i as u32);
            let ok = scalar_leaves(kty(f.ty), t, path, out);
            path.pop();
            ok
        }),
        _ => false,
    }
}

/// The fields of `ty` when it is a struct of numbers and bools, which compares inline.
fn inline_leaves(ty: KTy, t: &Tables) -> Option<Vec<Leaf>> {
    let mut out = Vec::new();
    (matches!(ty, KTy::Struct(_)) && scalar_leaves(ty, t, &mut Vec::new(), &mut out)).then_some(out)
}

/// `a == b` for two places holding a struct of numbers and bools: each field's `==`, joined by
/// `and` in declaration order (`true` for a struct with no fields).
fn leaves_equal(a: &Place, b: &Place, leaves: &[Leaf], line: u32) -> Expr {
    let at = |p: &Place, path: &[u32]| path.iter().fold(p.clone(), |p, &i| with(&p, Proj::Field(i)));
    let node = |kind: ExprKind| Expr { kind, ty: KTy::Bool, line };
    leaves
        .iter()
        .map(|(path, ty)| {
            let l = Expr { kind: ExprKind::Load(at(a, path)), ty: *ty, line };
            let r = Expr { kind: ExprKind::Load(at(b, path)), ty: *ty, line };
            node(ExprKind::Binary(BinOp::Eq, Box::new(l), Box::new(r)))
        })
        .reduce(|all, eq| node(ExprKind::And(Box::new(all), Box::new(eq))))
        .unwrap_or_else(|| node(ExprKind::Bool(true)))
}

/// `if !same { return false }`
fn unless(same: Expr) -> Stmt {
    let not = expr(ExprKind::Unary(UnOp::Not, Box::new(same)), KTy::Bool);
    let no = Stmt::Return(Some(expr(ExprKind::Bool(false), KTy::Bool)));
    Stmt::If { cond: not, then_body: vec![no], else_body: Vec::new() }
}

impl Helpers {
    /// `a == b` for two places holding a value of type `ty`: one `==` for a word, the fields
    /// inline for a struct of numbers and bools, else a call of `ty`'s equality function.
    pub(super) fn places_equal(&mut self, a: &Place, b: &Place, ty: KTy, t: &Tables) -> Expr {
        let load = |p: &Place| expr(ExprKind::Load(p.clone()), ty);
        if is_word(ty) {
            return expr(ExprKind::Binary(BinOp::Eq, Box::new(load(a)), Box::new(load(b))), KTy::Bool);
        }
        if let Some(leaves) = inline_leaves(ty, t) {
            return leaves_equal(a, b, &leaves, 0);
        }
        let f = self.eq_fn(ty, t);
        expr(ExprKind::Call(f, vec![Arg::Value(load(a)), Arg::Value(load(b))]), KTy::Bool)
    }

    /// Statements that return `false` from an equality function when the values at `a` and `b`,
    /// of type `ty`, differ.
    fn differ_returns(&mut self, a: Place, b: Place, ty: KTy, t: &Tables) -> Vec<Stmt> {
        match ty {
            _ if is_word(ty) => {
                let (l, r) = (expr(ExprKind::Load(a), ty), expr(ExprKind::Load(b), ty));
                vec![unless(expr(ExprKind::Binary(BinOp::Eq, Box::new(l), Box::new(r)), KTy::Bool))]
            }
            KTy::Struct(s) => {
                let mut out = Vec::new();
                for (i, f) in t.structs[s.0 as usize].fields.iter().enumerate() {
                    let field = Proj::Field(i as u32);
                    out.extend(self.differ_returns(with(&a, field.clone()), with(&b, field), kty(f.ty), t));
                }
                out
            }
            KTy::Enum(e) => {
                let tag = |p: &Place| expr(ExprKind::Tag(p.clone()), KTy::U8);
                let mut out = vec![unless(expr(ExprKind::Binary(BinOp::Eq, Box::new(tag(&a)), Box::new(tag(&b))), KTy::Bool))];
                for (v, variant) in t.enums[e.0 as usize].variants.iter().enumerate() {
                    let mut then_body = Vec::new();
                    for (i, f) in variant.fields.iter().enumerate() {
                        let field = Proj::Variant(v as u32, i as u32);
                        then_body.extend(self.differ_returns(with(&a, field.clone()), with(&b, field), kty(f.ty), t));
                    }
                    if !then_body.is_empty() {
                        out.push(Stmt::If { cond: tag_is(&a, v as u32), then_body, else_body: Vec::new() });
                    }
                }
                out
            }
            _ => unreachable!("the checker refuses `==` on a value holding a {ty:?}"),
        }
    }

    /// `__kestrel_eq_N(a, b) -> bool`: whether two values of struct or enum type `ty` are equal.
    pub(crate) fn eq_fn(&mut self, ty: KTy, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.eq.get(&ty) {
            return f;
        }
        let idx = self.reserve();
        self.eq.insert(ty, idx);
        let mut body = self.differ_returns(Place::local(A), Place::local(B), ty, t);
        body.push(Stmt::Return(Some(expr(ExprKind::Bool(true), KTy::Bool))));
        let value = |name: &str| Local { name: name.into(), ty, kind: LocalKind::RefParam };
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_eq_{}", idx.0),
            params: vec![A, B],
            ret: Some(KTy::Bool),
            locals: vec![value("a"), value("b")],
            body,
            is_pure: true,
            io: false,
            mutates_params: false,
            exported: false,
            lines: (0, 0),
            native: None,
        };
        idx
    }
}

impl FnLowerer<'_> {
    /// Whether `e` is a struct other than a vector, or an enum: its `==` is `record_equal`.
    pub(crate) fn is_record(&self, e: &ast::Expr) -> bool {
        matches!(kty(self.ty(e.id)), KTy::Struct(_) | KTy::Enum(_)) && self.vec_ty(e).is_none()
    }

    /// `lhs == rhs`, or with `ne` `lhs != rhs`, on two values of one struct or enum. Each side
    /// is evaluated once, left to right.
    pub(crate) fn record_equal(&mut self, ne: bool, lhs: &ast::Expr, rhs: &ast::Expr, line: u32) -> ExprKind {
        let ty = kty(self.ty(lhs.id));
        let (l, r) = (self.expr(lhs), self.expr(rhs));
        let mut pre = Vec::new();
        let same = match inline_leaves(ty, self.t) {
            Some(leaves) => {
                // The fields are read after both sides run, so a left side that the right side
                // may change (a call with a `var` argument) is copied first, as for numbers.
                let reread = |e: &Expr| matches!(&e.kind, ExprKind::Load(p) if rereadable(p));
                let l = if reread(&l) && !may_write(&r) { l } else { self.spill(l, &mut pre) };
                let r = if reread(&r) { r } else { self.spill(r, &mut pre) };
                let (ExprKind::Load(lp), ExprKind::Load(rp)) = (l.kind, r.kind) else { unreachable!("both sides are places") };
                leaves_equal(&lp, &rp, &leaves, line)
            }
            None => {
                let f = self.helpers.eq_fn(ty, self.t);
                Expr { kind: ExprKind::Call(f, vec![Arg::Value(l), Arg::Value(r)]), ty: KTy::Bool, line }
            }
        };
        let kind = if ne { ExprKind::Unary(UnOp::Not, Box::new(same)) } else { same.kind };
        Self::with_pre(pre, kind, KTy::Bool, line)
    }
}
