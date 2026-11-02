use kestrel_check::{ConstId, Res, StrMethod, Ty};
use kestrel_syntax::ast;

use super::builtins::constant_value;
use super::consts::{decl, hoisted};
use super::{binop, kty, FnLowerer};
use crate::ir::*;

impl<'a> FnLowerer<'a> {
    /// Lowers `e` and applies any implicit conversion the checker recorded.
    pub(crate) fn expr(&mut self, e: &ast::Expr) -> Expr {
        let Some(to) = self.coerce(e.id) else { return self.expr_raw(e) };
        self.expr_as(e, to)
    }

    /// `e` converted to `to`: the conversion the checker recorded, or the `T` an `Ok` wrap
    /// converts to first.
    fn expr_as(&mut self, e: &ast::Expr, to: Ty) -> Expr {
        // A struct or enum value used as an interface value (or an optional of one).
        if let Some(v) = self.iface_coercion(e, to) {
            return v;
        }
        // A `T` used as a `T?`: wrap it (converting it to the inner type first).
        if let Ty::Optional(inner) = to {
            let inner = kty(self.t.elem(inner));
            let v = if kty(self.ty(e.id)) == inner { self.expr_raw(e) } else { self.converted(e, inner) };
            let line = v.line;
            return Expr { kind: ExprKind::Some(Box::new(v)), ty: kty(to), line };
        }
        if let Some(v) = self.ok_wrapped(e, to) {
            return v;
        }
        self.converted(e, kty(to))
    }

    /// A `T` used as a `Result[T, E]`: `Result.Ok(value: v)`, `v` converted to `T` first and
    /// stored as a variant's field is (`owned`), exactly as a written `Result.Ok(value: v)` lowers.
    fn ok_wrapped(&mut self, e: &ast::Expr, to: Ty) -> Option<Expr> {
        let (value, _) = self.t.result_parts(to)?;
        let Ty::Enum(id) = to else { return None };
        let ok = self.t.enm(id).variants.iter().position(|v| v.name == "Ok")? as u32;
        let v = if self.ty(e.id) == value { self.expr_raw(e) } else { self.expr_as(e, value) };
        let v = self.owned(v, false);
        let line = v.line;
        Some(Expr { kind: ExprKind::VariantLit(EnumIdx(id.0), ok, vec![v]), ty: kty(to), line })
    }

    /// `e` converted to the type `to`.
    fn converted(&mut self, e: &ast::Expr, to: KTy) -> Expr {
        // A literal (or `PI` / `TAU`) converted to a float is emitted at the target type,
        // read from the source, so an f64 never inherits the rounding of an f32 value.
        if to.is_float() {
            if let Some(v) = literal_as(self, e, to) {
                return Expr { kind: ExprKind::Float(v), ty: to, line: self.line(e.span) };
            }
        }
        let raw = self.expr_raw(e);
        Expr { ty: to, line: raw.line, kind: ExprKind::Cast(Box::new(raw)) }
    }

    pub(crate) fn expr_raw(&mut self, e: &ast::Expr) -> Expr {
        let ty = kty(self.ty(e.id));
        let line = self.line(e.span);
        let kind = match &e.kind {
            ast::ExprKind::Int(n) => {
                if ty.is_float() {
                    ExprKind::Float(*n as f64)
                } else {
                    // A hex literal above its type's positive range is its bit pattern.
                    ExprKind::Int(self.t.wrapped_literals.get(&e.id).copied().unwrap_or(*n as i64))
                }
            }
            ast::ExprKind::Float(f) => ExprKind::Float(*f),
            ast::ExprKind::Bool(b) => ExprKind::Bool(*b),
            ast::ExprKind::None => ExprKind::NoneLit,
            ast::ExprKind::Str(s) => ExprKind::StrLit(s.clone()),
            ast::ExprKind::Interp(parts) => ExprKind::Format(self.interp_parts(parts)),
            ast::ExprKind::Ident(_) => match self.resolved(e.id) {
                Res::Local(_) if self.t.moves.contains(&e.id) => ExprKind::Move(self.place(e).expect("a local is a place")),
                Res::Local(_) => ExprKind::Load(self.place(e).expect("a local is a place")),
                // Built once, before `main`: a use reads the global in place (see `consts`).
                Res::Const(c) if hoisted(self.t, c) => ExprKind::Load(Place::local(self.const_local(c))),
                Res::Const(c) => return self.const_value(c),
                Res::Builtin(b) => Self::builtin_constant(b),
                Res::FnValue(f) => self.fn_ref(f),
                other => unreachable!("name resolved to {other:?}"),
            },
            ast::ExprKind::Unary { op: op @ (ast::UnaryOp::Neg | ast::UnaryOp::BitNot), expr } if self.vec_ty(e).is_some() => {
                let v = self.expr(expr);
                let op = if *op == ast::UnaryOp::Neg { UnOp::Neg } else { UnOp::BitNot };
                self.vec_unary(op, v, ty, line)
            }
            ast::ExprKind::Unary { op, expr } => {
                let op = match op {
                    ast::UnaryOp::Neg => UnOp::Neg,
                    ast::UnaryOp::Not => UnOp::Not,
                    ast::UnaryOp::BitNot => UnOp::BitNot,
                };
                ExprKind::Unary(op, Box::new(self.expr(expr)))
            }
            ast::ExprKind::Binary { op: ast::BinaryOp::Add, lhs, rhs } if ty == KTy::Str => self.str_concat(lhs, rhs),
            ast::ExprKind::Binary { op: op @ (ast::BinaryOp::Eq | ast::BinaryOp::Ne), lhs, rhs }
                if self.vec_ty(lhs).is_some() || self.vec_ty(rhs).is_some() =>
            {
                let (l, r) = (self.expr(lhs), self.expr(rhs));
                self.vec_equal(*op == ast::BinaryOp::Ne, l, r, line)
            }
            // Two values of one struct or enum, field by field (decision 19).
            ast::ExprKind::Binary { op: op @ (ast::BinaryOp::Eq | ast::BinaryOp::Ne), lhs, rhs } if self.is_record(lhs) => {
                self.record_equal(*op == ast::BinaryOp::Ne, lhs, rhs, line)
            }
            // Two strings or two bools ordered (G5, P5).
            ast::ExprKind::Binary { op: op @ (ast::BinaryOp::Lt | ast::BinaryOp::Le | ast::BinaryOp::Gt | ast::BinaryOp::Ge), lhs, rhs }
                if matches!(self.ty(lhs.id), Ty::Str | Ty::Bool) =>
            {
                self.ordered_compare(*op, lhs, rhs, line)
            }
            ast::ExprKind::Binary { op, lhs, rhs } if self.vec_ty(e).is_some() => {
                let (l, r) = (self.expr(lhs), self.expr(rhs));
                self.vec_combine(binop(*op), l, r, ty, line)
            }
            ast::ExprKind::Binary { op, lhs, rhs } => {
                let l = Box::new(self.expr(lhs));
                match op {
                    ast::BinaryOp::And => ExprKind::And(l, Box::new(self.conditional(rhs))),
                    ast::BinaryOp::Or => ExprKind::Or(l, Box::new(self.conditional(rhs))),
                    other => {
                        let r = Box::new(self.expr(rhs));
                        let op = binop(*other);
                        let r = Box::new(super::shift_amount(op, *r, l.ty));
                        ExprKind::Binary(op, l, r)
                    }
                }
            }
            // A call through a function value: a local or constant by name, a field, or any expression.
            ast::ExprKind::Call { .. } | ast::ExprKind::MethodCall { .. } | ast::ExprKind::CallValue { .. }
                if matches!(self.res(e.id), Some(Res::CallValue(_))) =>
            {
                self.call_value(e, ty, line)
            }
            ast::ExprKind::Call { args, .. } => self.call(e, args, ty, line),
            // A user `TypeCall` (`best[int](xs)`, `Stack[int]()`) is the call it stands for (P28).
            ast::ExprKind::TypeCall { args, .. } if !builtin_type_call(e) => self.call(e, args, ty, line),
            ast::ExprKind::MethodCall { receiver, name, args, .. }
                if matches!(self.res(e.id), Some(Res::MapRead | Res::MapRemove)) =>
            {
                self.map_method(receiver, name, args, ty, line)
            }
            ast::ExprKind::MethodCall { receiver, args, .. } => match self.resolved(e.id) {
                Res::Fn(f) => {
                    let (pre, fi, a) = self.call_parts(e, f, args, Some(receiver));
                    Self::with_pre(pre, ExprKind::Call(fi, a), ty, line)
                }
                Res::IfaceCall { iface, slot } => self.iface_call_value(e, iface, slot, ty, line),
                Res::Variant(id, v) => {
                    let (pre, kind) = self.variant_construct(e, id, v, args);
                    Self::with_pre(pre, kind, ty, line)
                }
                Res::Builtin(b) => self.vector_method(b, receiver, args, ty, line),
                Res::List(method) => self.list_value(method, receiver, args, ty, line),
                Res::Str(method) => self.str_method(method, receiver, args, ty, line),
                other => unreachable!("method call resolved to {other:?}"),
            },
            ast::ExprKind::Field { base, .. } if matches!(self.ty(base.id), Ty::Array(_) | Ty::List(_) | Ty::Map(..)) => {
                self.len_value(base, line)
            }
            ast::ExprKind::Field { base, .. } if matches!(self.res(e.id), Some(Res::Str(StrMethod::Len))) => {
                self.str_len(base, line)
            }
            ast::ExprKind::Index { base, index } if matches!(self.ty(base.id), Ty::Map(..)) => {
                self.map_get(base, index, ty, line)
            }
            ast::ExprKind::Index { base, index } if matches!(self.res(e.id), Some(Res::Str(StrMethod::At))) => {
                self.str_at(base, index)
            }
            ast::ExprKind::Index { base, index } => self.index_value(e, base, index, ty, line),
            ast::ExprKind::ArrayLit(items) => ExprKind::ArrayLit(items.iter().map(|i| self.value(i)).collect()),
            ast::ExprKind::TypeCall { name, args, .. } if name == "from_bytes" => self.decode_call(&args[0].value, ty),
            ast::ExprKind::TypeCall { args, .. } => match ty {
                KTy::Array(_) => ExprKind::ArrayNew(Box::new(self.expr(&args[0].value))),
                KTy::Map(..) => ExprKind::MapNew,
                _ => ExprKind::ListNew,
            },
            ast::ExprKind::Field { base, .. } if self.t.lanes.contains_key(&e.id) => self.lane_read(e, base, self.t.lanes[&e.id]),
            ast::ExprKind::Field { base, .. } if self.t.swizzles.contains_key(&e.id) => {
                let comps = self.t.swizzles[&e.id].clone();
                self.swizzle(base, &comps, ty, line)
            }
            ast::ExprKind::Field { base, .. } => match (self.res(e.id), self.place(e)) {
                (Some(Res::Variant(id, v)), _) => ExprKind::VariantLit(EnumIdx(id.0), v, Vec::new()),
                (_, Some(place)) => ExprKind::Load(place),
                (_, None) => {
                    let base = self.expr(base);
                    self.field_of(base, self.t.field_index[&e.id], ty, line)
                }
            },
            ast::ExprKind::Match { .. } => self.match_value(e),
            ast::ExprKind::If { cond, then_block, else_block } => {
                let else_block = else_block.as_ref().expect("checker requires `else` for a value `if`");
                let cond = Box::new(self.expr(cond));
                let (then_body, then_val) = self.block_value(then_block);
                let (else_body, else_val) = self.block_value(else_block);
                ExprKind::If {
                    cond,
                    then_body,
                    then_val: then_val.map(Box::new),
                    else_body,
                    else_val: else_val.map(Box::new),
                }
            }
            ast::ExprKind::MapLit(entries) => self.map_lit(entries, ty, line),
            ast::ExprKind::CallValue { .. } => unreachable!("the checker resolves a call of a value to `Res::CallValue`"),
            ast::ExprKind::Closure(lit) => self.make_closure(e, lit),
            // `best[Enemy]` as a value: the copy's function value. (A struct or enum with its types
            // written is a type, never a value; its variants are the `Field` or `MethodCall` around it.)
            ast::ExprKind::TypeApply { .. } => match self.resolved(e.id) {
                Res::FnValue(f) => self.fn_ref(f),
                other => unreachable!("a type application resolved to {other:?}"),
            },
            ast::ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
        };
        Expr { kind, ty, line }
    }

    /// A call by name (or a user `TypeCall`) as a value of type `ty`.
    fn call(&mut self, e: &ast::Expr, args: &[ast::Arg], ty: KTy, line: u32) -> ExprKind {
        match self.resolved(e.id) {
            Res::Convert(_) => ExprKind::Cast(Box::new(self.expr(&args[0].value))),
            Res::Construct(sid) if self.t.vec_makes.contains(&e.id) => self.vec_make(sid, args, ty, line),
            Res::SimdMake(_) => self.simd_make(e, args, ty, line),
            Res::NewArena => ExprKind::NewArena,
            Res::Builtin(b) => self.builtin_call(b, args, ty, line),
            Res::Fn(f) if self.wants_scratch(f, args) => self.scratch_call_value(e, f, args, ty, line),
            Res::Fn(f) => {
                let (pre, fi, a) = self.call_parts(e, f, args, None);
                Self::with_pre(pre, ExprKind::Call(fi, a), ty, line)
            }
            Res::Construct(sid) => {
                let (pre, kind) = self.construct(e, sid, args);
                Self::with_pre(pre, kind, ty, line)
            }
            other => unreachable!("{other:?} used as a value"),
        }
    }

    /// The initializer of a constant that holds no array, list or map, written out at each use.
    pub(super) fn const_value(&mut self, c: ConstId) -> Expr {
        let (t, program) = (self.t, self.program);
        self.expr(&decl(program, t, c).value)
    }
}

/// Whether `e` is a `TypeCall` of a built-in (`array[int](n)`, `list[T]()`, `map[K, V]()`,
/// `from_bytes[T](b)`). Every other `TypeCall` names a user generic and is lowered like the `Call`
/// it stands for (P28).
pub(crate) fn builtin_type_call(e: &ast::Expr) -> bool {
    matches!(&e.kind, ast::ExprKind::TypeCall { name, .. } if matches!(name.as_str(), "array" | "list" | "map" | "from_bytes"))
}

/// The value of a numeric literal or `PI` / `TAU` (possibly negated) as float type `to`,
/// or `None` if `e` is anything else.
fn literal_as(cx: &FnLowerer, e: &ast::Expr, to: KTy) -> Option<f64> {
    match &e.kind {
        // A hex literal above its type's positive range is its bit pattern (`0xFFFFFFFF` is -1).
        ast::ExprKind::Int(n) => {
            let v = cx.t.wrapped_literals.get(&e.id).copied();
            Some(match (v, to) {
                (Some(w), KTy::F32) => w as f32 as f64,
                (Some(w), _) => w as f64,
                (None, KTy::F32) => *n as f32 as f64,
                (None, _) => *n as f64,
            })
        }
        ast::ExprKind::Float(f) => Some(*f),
        ast::ExprKind::Ident(_) => match cx.res(e.id) {
            Some(Res::Builtin(b)) => constant_value(b),
            _ => None,
        },
        ast::ExprKind::Unary { op: ast::UnaryOp::Neg, expr } => literal_as(cx, expr, to).map(|v| -v),
        _ => None,
    }
}
