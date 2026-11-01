use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{types, InstBuilder, Value};
use cranelift_module::Module;
use kestrel_kir::{BinOp, KTy};
use kestrel_runtime::{PANIC_NARROW_INT, PANIC_NARROW_U8, PANIC_OVERFLOW, PANIC_SHIFT};

use super::{cl_type, Check, FuncLowerer};

fn bits(t: KTy) -> u32 {
    match t {
        KTy::Bool | KTy::U8 => 8,
        KTy::I32 | KTy::F32 => 32,
        KTy::I64 | KTy::F64 => 64,
        KTy::F32X4
        | KTy::I32X4
        | KTy::Struct(_)
        | KTy::Enum(_)
        | KTy::Str
        | KTy::Array(_)
        | KTy::List(_)
        | KTy::Map(..)
        | KTy::Arena
        | KTy::Gpu
        | KTy::Optional(_)
        | KTy::Fn
        | KTy::Iface => {
            unreachable!("vectors, aggregates and arenas have no one width")
        }
    }
}

/// The float compare of the comparison `op`: ordered, so false when either value is NaN, but
/// for `!=`, which is true.
pub(super) fn float_cc(op: BinOp) -> FloatCC {
    match op {
        BinOp::Eq => FloatCC::Equal,
        BinOp::Ne => FloatCC::NotEqual,
        BinOp::Lt => FloatCC::LessThan,
        BinOp::Gt => FloatCC::GreaterThan,
        BinOp::Le => FloatCC::LessThanOrEqual,
        _ => FloatCC::GreaterThanOrEqual,
    }
}

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// `a op b` (`operands`) where both operands have type `t`. `const_divisor` is `b`'s
    /// value when it is an integer literal (used only by `/` and `%`). `check` says what
    /// the integer checks do (see `Check`).
    pub(crate) fn binary(
        &mut self,
        op: BinOp,
        (a, b): (Value, Value),
        t: KTy,
        line: u32,
        const_divisor: Option<i64>,
        check: Check,
    ) -> Value {
        // Here, not in `expr`, so a `Proven` vector operation takes this path too.
        if t.is_vector() {
            return self.simd_binary(op, (a, b), t, line, check);
        }
        if op.is_comparison() {
            return self.compare(op, a, b, t);
        }
        if t.is_float() {
            return match op {
                BinOp::Add => self.b.ins().fadd(a, b),
                BinOp::Sub => self.b.ins().fsub(a, b),
                BinOp::Mul => self.b.ins().fmul(a, b),
                BinOp::Div => self.b.ins().fdiv(a, b),
                // A `float` `%` is inline (`wide_exp`); an `f64` one calls `kestrel_rt_fmod_f64`.
                BinOp::Rem if t == KTy::F32 => self.wide_fmod(a, b),
                BinOp::Rem => self.call_id(self.shared.rt.fmod_f64, &[a, b]).expect("fmod returns a value"),
                _ => unreachable!("comparisons handled above"),
            };
        }
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => self.int_arith(op, a, b, t, line, check),
            // `*% +% -%`: plain wrapping instructions on the operand type, in every build.
            BinOp::AddWrap => self.b.ins().iadd(a, b),
            BinOp::SubWrap => self.b.ins().isub(a, b),
            BinOp::MulWrap => self.b.ins().imul(a, b),
            BinOp::Div | BinOp::Rem => self.int_div(op, (a, b), t, line, const_divisor, check),
            BinOp::BitAnd => self.b.ins().band(a, b),
            BinOp::BitOr => self.b.ins().bor(a, b),
            BinOp::BitXor => self.b.ins().bxor(a, b),
            BinOp::Shl | BinOp::Shr => self.shift(op, a, b, t, line, check),
            _ => unreachable!("comparisons handled above"),
        }
    }

    /// `a << b` / `a >> b` (arithmetic for signed types). With checks on, an
    /// amount that's negative or at least the width panics; otherwise the
    /// machine masks it.
    fn shift(&mut self, op: BinOp, a: Value, b: Value, t: KTy, line: u32, check: Check) -> Value {
        if check.arith(self.shared.opts.checked_arith) {
            // Unsigned compare: a negative amount looks huge.
            let bad = self.b.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, b, bits(t) as i64);
            self.fail_if(bad, PANIC_SHIFT, line, check);
        }
        match (op, t.is_signed()) {
            (BinOp::Shl, _) => self.b.ins().ishl(a, b),
            (_, true) => self.b.ins().sshr(a, b),
            _ => self.b.ins().ushr(a, b),
        }
    }

    /// Integer `+ - *`, overflow-checked when `checked_arith` is on.
    pub(crate) fn int_arith(&mut self, op: BinOp, a: Value, b: Value, t: KTy, line: u32, check: Check) -> Value {
        let plain = |fl: &mut Self, x: Value, y: Value| match op {
            BinOp::Add => fl.b.ins().iadd(x, y),
            BinOp::Sub => fl.b.ins().isub(x, y),
            _ => fl.b.ins().imul(x, y),
        };
        if !check.arith(self.shared.opts.checked_arith) {
            return plain(self, a, b);
        }
        if t == KTy::U8 {
            // Compute in 32 bits; anything outside 0..=255 (including
            // negative results, which look huge unsigned) overflowed.
            let x = self.b.ins().uextend(types::I32, a);
            let y = self.b.ins().uextend(types::I32, b);
            let r = plain(self, x, y);
            let bad = self.b.ins().icmp_imm_s(IntCC::UnsignedGreaterThan, r, 255);
            self.fail_if(bad, PANIC_OVERFLOW, line, check);
            return self.b.ins().ireduce(types::I8, r);
        }
        // The value comes from the plain operation, not from the `_overflow` instruction: that one
        // has two results, so Cranelift's e-graph treats it as a side effect and leaves it where it
        // is, and a value taken from it couldn't be hoisted out of a loop with everything made
        // from it. Only the check stays in the loop.
        let r = plain(self, a, b);
        let (_, overflowed) = match op {
            BinOp::Add => self.b.ins().sadd_overflow(a, b),
            BinOp::Sub => self.b.ins().ssub_overflow(a, b),
            _ => self.b.ins().smul_overflow(a, b),
        };
        self.fail_if(overflowed, PANIC_OVERFLOW, line, check);
        r
    }

    fn compare(&mut self, op: BinOp, a: Value, b: Value, t: KTy) -> Value {
        if t.is_float() {
            return self.b.ins().fcmp(float_cc(op), a, b);
        }
        let signed = t.is_signed();
        let cc = match op {
            BinOp::Eq => IntCC::Equal,
            BinOp::Ne => IntCC::NotEqual,
            BinOp::Lt if signed => IntCC::SignedLessThan,
            BinOp::Lt => IntCC::UnsignedLessThan,
            BinOp::Gt if signed => IntCC::SignedGreaterThan,
            BinOp::Gt => IntCC::UnsignedGreaterThan,
            BinOp::Le if signed => IntCC::SignedLessThanOrEqual,
            BinOp::Le => IntCC::UnsignedLessThanOrEqual,
            _ if signed => IntCC::SignedGreaterThanOrEqual,
            _ => IntCC::UnsignedGreaterThanOrEqual,
        };
        self.b.ins().icmp(cc, a, b)
    }

    /// Converts `v` from `from` to `to`: int widening/narrowing (narrowing
    /// panics on a value that doesn't fit when `checked_arith` is on, and
    /// truncates otherwise), int<->float, float->int saturating (NaN -> 0).
    pub(crate) fn cast(&mut self, v: Value, from: KTy, to: KTy, line: u32, check: Check) -> Value {
        if from == to {
            return v;
        }
        if from.is_vector() {
            return self.simd_cast(v, to);
        }
        let to_ty = cl_type(to);
        match (from.is_float(), to.is_float()) {
            (false, false) => {
                let (fb, tb) = (bits(from), bits(to));
                if tb > fb {
                    if from.is_signed() {
                        self.b.ins().sextend(to_ty, v)
                    } else {
                        self.b.ins().uextend(to_ty, v)
                    }
                } else if tb < fb {
                    let r = self.b.ins().ireduce(to_ty, v);
                    if check.arith(self.shared.opts.checked_arith) {
                        // It fit if widening it back gives the original value.
                        let back = if to.is_signed() {
                            self.b.ins().sextend(cl_type(from), r)
                        } else {
                            self.b.ins().uextend(cl_type(from), r)
                        };
                        let bad = self.b.ins().icmp(IntCC::NotEqual, back, v);
                        let kind = if to == KTy::U8 { PANIC_NARROW_U8 } else { PANIC_NARROW_INT };
                        self.fail_if(bad, kind, line, check);
                    }
                    r
                } else {
                    v
                }
            }
            (false, true) => {
                if from.is_signed() {
                    self.b.ins().fcvt_from_sint(to_ty, v)
                } else {
                    let wide = self.b.ins().uextend(types::I32, v);
                    self.b.ins().fcvt_from_sint(to_ty, wide)
                }
            }
            (true, false) => {
                if to == KTy::U8 {
                    let wide = self.b.ins().fcvt_to_sint_sat(types::I32, v);
                    let zero = self.b.ins().iconst(types::I32, 0);
                    let max = self.b.ins().iconst(types::I32, 255);
                    let lo = self.b.ins().smax(wide, zero);
                    let clamped = self.b.ins().smin(lo, max);
                    self.b.ins().ireduce(types::I8, clamped)
                } else {
                    self.b.ins().fcvt_to_sint_sat(to_ty, v)
                }
            }
            (true, true) => {
                if to == KTy::F64 {
                    self.b.ins().fpromote(types::F64, v)
                } else {
                    self.b.ins().fdemote(types::F32, v)
                }
            }
        }
    }
}
