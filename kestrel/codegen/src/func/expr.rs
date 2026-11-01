use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData, TrapCode, Value};
use cranelift_module::Module;
use kestrel_kir::{self as kir, ExprKind, KTy, UnOp};

use super::divide::pow2_rem_test;
use super::{cl_type, Check, FuncLowerer};

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// Lowers a scalar-valued expression. Struct values go through `lower_into`.
    pub(crate) fn expr(&mut self, e: &kir::Expr) -> Result<Value, String> {
        let ty = cl_type(e.ty);
        Ok(match &e.kind {
            ExprKind::Int(n) => self.iconst(e.ty, *n),
            ExprKind::Float(f) => {
                if e.ty == KTy::F32 {
                    self.b.ins().f32const(*f as f32)
                } else {
                    self.b.ins().f64const(*f)
                }
            }
            ExprKind::Bool(v) => self.b.ins().iconst(types::I8, *v as i64),
            ExprKind::Load(p) => self.load_place(p)?,
            ExprKind::Len(p) => self.len(p)?,
            ExprKind::Cap(p) => self.cap(p)?,
            ExprKind::ReadBytes(p, args) => self.read_bytes(p, args, e.ty)?,
            ExprKind::Map(op, p, args) => self.map_op(*op, p, args)?,
            ExprKind::Str(op, args) => self.str_scalar(*op, args)?,
            ExprKind::FieldOf(base, i) => {
                let addr = self.field_addr(base, *i)?;
                self.load_at(e.ty, addr, 0)
            }
            ExprKind::Seq(stmts, value) => {
                self.stmts(stmts)?;
                self.expr(value)?
            }
            ExprKind::FnEnv(p) => self.fn_env(p)?,
            ExprKind::EnvDup(env) => self.env_dup(env)?,
            ExprKind::BlockDup { src, size, align } => self.block_dup(src, *size, *align)?,
            ExprKind::VecLit(_) | ExprKind::Splat(_) | ExprKind::Shuffle(..) | ExprKind::LaneOf(..) => self.simd_expr(e)?,
            ExprKind::Tag(p) => {
                let addr = self.place_addr(p)?;
                self.b.ins().load(types::I8, MemFlagsData::trusted(), addr, 0)
            }
            ExprKind::StructLit(..)
            | ExprKind::VariantLit(..)
            | ExprKind::StrLit(_)
            | ExprKind::Format(_)
            | ExprKind::ArrayNew(_)
            | ExprKind::ArrayLit(_)
            | ExprKind::ListNew
            | ExprKind::MapNew
            | ExprKind::NewArena
            | ExprKind::StaticArena
            | ExprKind::Some(_)
            | ExprKind::NoneLit
            | ExprKind::FnRef(_)
            | ExprKind::MakeFn { .. }
            | ExprKind::MakeIface { .. }
            | ExprKind::Move(_) => unreachable!("aggregate values are built in memory"),
            ExprKind::Unary(UnOp::Not, inner) => {
                let v = self.expr(inner)?;
                self.b.ins().bxor_imm_s(v, 1)
            }
            ExprKind::Unary(UnOp::BitNot, inner) => {
                let v = self.expr(inner)?;
                self.b.ins().bnot(v)
            }
            ExprKind::Unary(UnOp::Neg, _) | ExprKind::Cast(_) => self.arith_expr(e, Check::Emit)?,
            ExprKind::Binary(op, l, r) if l.ty == KTy::Str => self.str_compare(*op, l, r)?,
            ExprKind::Binary(..) => match pow2_rem_test(e) {
                Some((n, k, is_eq)) => {
                    let v = self.expr(n)?;
                    self.mask_test(v, k, is_eq)
                }
                None => self.arith_expr(e, Check::Emit)?,
            },
            // Its checks were proven, so they are left out (or, verifying, kept as a compiler-bug check).
            // The operands carry their own wrappers.
            ExprKind::Proven(inner) => self.arith_expr(inner, self.proven_check())?,
            ExprKind::And(l, r) => self.short_circuit(l, r, true)?,
            ExprKind::Or(l, r) => self.short_circuit(l, r, false)?,
            ExprKind::Math(op, args) => self.math(*op, args, e.ty)?,
            ExprKind::Hash(op, args) => self.hash(*op, args)?,
            ExprKind::RandFloat { .. } => self.rand_float(e.ty),
            ExprKind::RandInt(lo, hi) => self.rand_int(lo, hi, e.ty)?,
            ExprKind::Call(fi, args) => self.call(*fi, args, None)?.expect("value-returning call"),
            ExprKind::CallIndirect { callee, args, sig } => self.call_indirect(callee, args, sig, None)?.expect("value-returning call"),
            ExprKind::CallVirtual { recv, slot, args, sig, .. } => self.call_virtual(recv, *slot, args, sig, None)?.expect("value-returning call"),
            ExprKind::If { cond, then_body, then_val, else_body, else_val } => {
                let sides = [(&then_body[..], then_val.as_deref()), (&else_body[..], else_val.as_deref())];
                if then_val.is_some() && else_val.is_some() {
                    if let Some(locals) = self.selectable(sides) {
                        let t = self.test(cond, true)?;
                        return Ok(self.select_sides(&t, sides, &locals)?.expect("both sides have a value"));
                    }
                }
                let t = self.test(cond, false)?;
                let (then_b, else_b, merge) = (self.b.create_block(), self.b.create_block(), self.b.create_block());
                let result = self.b.append_block_param(merge, ty);
                self.b.ins().brif(t.flag, then_b, &[], else_b, &[]);
                for (then, block, side) in [(true, then_b, sides[0]), (false, else_b, sides[1])] {
                    match self.branch_side(&t, then, block, side)? {
                        Some(Some(x)) => {
                            self.b.ins().jump(merge, &[x.into()]);
                        }
                        Some(None) => {
                            self.b.ins().trap(TrapCode::unwrap_user(1));
                        }
                        None => {}
                    }
                }
                self.b.switch_to_block(merge);
                result
            }
        })
    }

    /// An integer `-x`, binary operation or conversion (or a float one), with its integer
    /// checks behaving as `check` says. The operands are lowered with `Emit`.
    fn arith_expr(&mut self, e: &kir::Expr, check: Check) -> Result<Value, String> {
        Ok(match &e.kind {
            ExprKind::Unary(UnOp::Neg, inner) => {
                let v = self.expr(inner)?;
                if e.ty.is_vector() {
                    self.simd_neg(v, e.ty, e.line, check)
                } else if e.ty.is_float() {
                    self.b.ins().fneg(v)
                } else {
                    let zero = self.iconst(e.ty, 0);
                    self.int_arith(kir::BinOp::Sub, zero, v, e.ty, e.line, check)
                }
            }
            ExprKind::Binary(op, l, r) => {
                let a = self.expr(l)?;
                let b = self.expr(r)?;
                // A literal divisor, even inside `Proven`.
                let const_divisor = if let ExprKind::Int(c) = r.kind { Some(c) } else { None };
                self.binary(*op, (a, b), l.ty, e.line, const_divisor, check)
            }
            ExprKind::Cast(inner) => {
                let v = self.expr(inner)?;
                self.cast(v, inner.ty, e.ty, e.line, check)
            }
            _ => unreachable!("only `-`, binary operations and conversions are proven"),
        })
    }

    /// An integer constant, masked to the type's width as Cranelift expects.
    pub(crate) fn iconst(&mut self, ty: KTy, n: i64) -> Value {
        let masked = match ty {
            KTy::Bool | KTy::U8 => n & 0xFF,
            KTy::I32 => n & 0xFFFF_FFFF,
            _ => n,
        };
        self.b.ins().iconst(cl_type(ty), masked)
    }

    fn short_circuit(&mut self, l: &kir::Expr, r: &kir::Expr, is_and: bool) -> Result<Value, String> {
        let a = self.expr(l)?;
        let (rhs_b, merge) = (self.b.create_block(), self.b.create_block());
        let result = self.b.append_block_param(merge, types::I8);
        if is_and {
            self.b.ins().brif(a, rhs_b, &[], merge, &[a.into()]);
        } else {
            self.b.ins().brif(a, merge, &[a.into()], rhs_b, &[]);
        }
        self.b.switch_to_block(rhs_b);
        let b = self.expr(r)?;
        self.b.ins().jump(merge, &[b.into()]);
        self.b.switch_to_block(merge);
        Ok(result)
    }
}
