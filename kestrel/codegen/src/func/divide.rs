use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, InstBuilder, Value};
use cranelift_module::Module;
use kestrel_kir::{BinOp, KTy};
use kestrel_runtime::{PANIC_DIV_ZERO, PANIC_OVERFLOW};

use super::{Check, FuncLowerer};

/// A literal divisor as the value `iconst` will actually produce at type `t`.
fn wrap_literal(t: KTy, c: i64) -> i64 {
    match t {
        KTy::U8 => c & 0xFF,
        KTy::I32 => c as i32 as i64,
        _ => c,
    }
}

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// Integer `/` and `%`. `const_divisor` is the right operand's value when
    /// it is a literal, which lets most of the checks (and the `idiv`) go away.
    pub(super) fn int_div(
        &mut self,
        op: BinOp,
        (a, b): (Value, Value),
        t: KTy,
        line: u32,
        const_divisor: Option<i64>,
        check: Check,
    ) -> Value {
        if let Some(c) = const_divisor {
            if let Some(v) = self.div_by_const(op, a, t, wrap_literal(t, c)) {
                return v;
            }
        }
        self.div_checked(op, a, b, t, line, check)
    }

    /// `a op c` for a literal `c`, or `None` when `c` is 0 or (signed) -1 and
    /// the checked path has to run to panic or wrap.
    fn div_by_const(&mut self, op: BinOp, a: Value, t: KTy, c: i64) -> Option<Value> {
        let is_div = op == BinOp::Div;
        let signed = t.is_signed();
        if c == 0 || (signed && c == -1) {
            return None;
        }
        if c == 1 {
            return Some(if is_div { a } else { self.iconst(t, 0) });
        }
        if c > 1 && c & (c - 1) == 0 {
            let k = c.trailing_zeros() as i64;
            return Some(if signed { self.signed_pow2(is_div, a, t, k) } else { self.unsigned_pow2(is_div, a, k) });
        }
        // Neither the zero check nor MIN / -1 can fire for any other literal. Cranelift turns a
        // division of an `i32` or `i64` by a constant into a multiply by its magic number, but
        // leaves an `i8` one a `div`, so a `u8` is divided as an `i32`.
        if t == KTy::U8 {
            let wide = self.b.ins().uextend(types::I32, a);
            let d = self.b.ins().iconst(types::I32, c);
            let v = if is_div { self.b.ins().udiv(wide, d) } else { self.b.ins().urem(wide, d) };
            return Some(self.b.ins().ireduce(types::I8, v));
        }
        let d = self.iconst(t, c);
        Some(match (signed, is_div) {
            (true, true) => self.b.ins().sdiv(a, d),
            (true, false) => self.b.ins().srem(a, d),
            (false, true) => self.b.ins().udiv(a, d),
            (false, false) => self.b.ins().urem(a, d),
        })
    }

    /// Unsigned `a / 2^k` and `a % 2^k` (k >= 1).
    fn unsigned_pow2(&mut self, is_div: bool, a: Value, k: i64) -> Value {
        if is_div {
            self.b.ins().ushr_imm_s(a, k)
        } else {
            self.b.ins().band_imm_s(a, (1i64 << k) - 1)
        }
    }

    /// Signed `a / 2^k` and `a % 2^k` (1 <= k < bits - 1), truncating toward
    /// zero like `idiv`. A bare arithmetic shift floors, which is one too low
    /// for negative `a` that isn't a multiple of 2^k; adding 2^k - 1 first
    /// (only when `a` is negative) rounds those up instead. The bias can't
    /// overflow: it is only added to negative values.
    fn signed_pow2(&mut self, is_div: bool, a: Value, t: KTy, k: i64) -> Value {
        let bits = if t == KTy::I32 { 32 } else { 64 };
        let sign = self.b.ins().sshr_imm_s(a, bits - 1);
        let bias = self.b.ins().ushr_imm_s(sign, bits - k);
        let biased = self.b.ins().iadd(a, bias);
        let q = self.b.ins().sshr_imm_s(biased, k);
        if is_div {
            return q;
        }
        // Remainder is what's left after removing the truncated quotient.
        let back = self.b.ins().ishl_imm_s(q, k);
        self.b.ins().isub(a, back)
    }

    /// Divisor unknown at compile time: zero check, then (signed) `MIN / -1`. A `Skip` check
    /// (the divisor is proven never 0, and never -1 with the dividend at the minimum) divides
    /// plainly; `Verify` keeps both checks but fails as a compiler bug.
    fn div_checked(&mut self, op: BinOp, a: Value, b: Value, t: KTy, line: u32, check: Check) -> Value {
        if check == Check::Skip {
            return self.plain_div(op, a, b, t);
        }
        let is_zero = self.b.ins().icmp_imm_s(IntCC::Equal, b, 0);
        self.fail_if(is_zero, PANIC_DIV_ZERO, line, check);
        if !t.is_signed() {
            return self.plain_div(op, a, b, t);
        }
        // MIN / -1 overflows (and traps in hardware).
        let min = if t == KTy::I32 { i32::MIN as i64 } else { i64::MIN };
        let min_v = self.iconst(t, min);
        let neg_one = self.iconst(t, -1);
        let a_is_min = self.b.ins().icmp(IntCC::Equal, a, min_v);
        let b_is_neg1 = self.b.ins().icmp(IntCC::Equal, b, neg_one);
        let both = self.b.ins().band(a_is_min, b_is_neg1);
        let divisor = if check == Check::Verify || self.shared.opts.checked_arith {
            self.fail_if(both, PANIC_OVERFLOW, line, check);
            b
        } else {
            // Wrapping: MIN / 1 == MIN and MIN % 1 == 0, matching wrapping semantics.
            let one = self.iconst(t, 1);
            self.b.ins().select(both, one, b)
        };
        self.plain_div(op, a, divisor, t)
    }

    /// `a / b` or `a % b` with no check at all.
    fn plain_div(&mut self, op: BinOp, a: Value, b: Value, t: KTy) -> Value {
        match (t.is_signed(), op == BinOp::Div) {
            (true, true) => self.b.ins().sdiv(a, b),
            (true, false) => self.b.ins().srem(a, b),
            (false, true) => self.b.ins().udiv(a, b),
            (false, false) => self.b.ins().urem(a, b),
        }
    }
}
