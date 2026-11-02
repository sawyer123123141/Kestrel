//! String operations (`ExprKind::Str`): the runtime (`runtime/src/string.rs`) counts and cuts
//! characters; a string is a pointer and a byte length, like an array of bytes.

use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData, Value};
use cranelift_module::Module;
use kestrel_kir::{self as kir, KTy, StrOp};

use super::collections::LEN;
use super::FuncLowerer;

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// The pointer and byte length of the string value `e`, read now (before any later operand
    /// is evaluated, which could change the place it came from).
    pub(super) fn str_parts(&mut self, e: &kir::Expr) -> Result<(Value, Value), String> {
        let addr = self.materialize(e)?;
        let ptr = self.b.ins().load(self.ptr, MemFlagsData::trusted(), addr, 0);
        let len = self.b.ins().load(types::I64, MemFlagsData::trusted(), addr, LEN);
        Ok((ptr, len))
    }

    /// An integer operand as an i64.
    fn str_int(&mut self, e: &kir::Expr) -> Result<Value, String> {
        let v = self.expr(e)?;
        Ok(self.widen_i64(v, e.ty))
    }

    /// A string operation with a number as its result.
    pub(crate) fn str_scalar(&mut self, op: StrOp, args: &[kir::Expr]) -> Result<Value, String> {
        let rt = self.shared.rt;
        let (ptr, len) = self.str_parts(&args[0])?;
        Ok(match op {
            StrOp::Find | StrOp::Starts | StrOp::Ends | StrOp::Cmp => {
                let (sub, sub_len) = self.str_parts(&args[1])?;
                let f = match op {
                    StrOp::Find => rt.str_find,
                    StrOp::Starts => rt.str_starts,
                    StrOp::Cmp => rt.str_cmp,
                    _ => rt.str_ends,
                };
                self.call_id(f, &[ptr, len, sub, sub_len])
            }
            StrOp::Chars => self.call_id(rt.str_chars, &[ptr, len]),
            StrOp::CharLen => {
                let pos = self.str_int(&args[1])?;
                self.call_id(rt.str_char_len, &[ptr, len, pos])
            }
            _ => unreachable!("{op:?} builds a value in memory"),
        }
        .expect("returns a number"))
    }

    /// A string operation whose result is a value in memory (a new string, a list or an optional),
    /// written at `dest`.
    pub(crate) fn str_into(&mut self, op: StrOp, args: &[kir::Expr], ty: KTy, line: u32, dest: Value) -> Result<(), String> {
        let rt = self.shared.rt;
        if op == StrOp::Hex {
            let bits = self.str_int(&args[0])?;
            let digits = self.str_int(&args[1])?;
            self.call_id(rt.str_hex, &[bits, digits, dest]);
            return Ok(());
        }
        if op == StrOp::BytesReason {
            let code = self.str_int(&args[0])?;
            let at = self.str_int(&args[1])?;
            let n = self.str_int(&args[2])?;
            let (name, name_len) = self.str_parts(&args[3])?;
            self.call_id(rt.bytes_reason, &[code, at, n, name, name_len, dest]);
            return Ok(());
        }
        let (ptr, len) = self.str_parts(&args[0])?;
        match op {
            StrOp::At => {
                let i = self.str_int(&args[1])?;
                let (l, f) = self.loc_args(line);
                self.call_id(rt.str_at, &[ptr, len, i, l, f, dest]);
            }
            StrOp::Piece => {
                let pos = self.str_int(&args[1])?;
                let n = self.str_int(&args[2])?;
                self.call_id(rt.str_piece, &[ptr, len, pos, n, dest]);
            }
            StrOp::Slice => {
                let from = self.str_int(&args[1])?;
                let to = self.str_int(&args[2])?;
                let (l, f) = self.loc_args(line);
                self.call_id(rt.str_slice, &[ptr, len, from, to, l, f, dest]);
            }
            StrOp::Trim => {
                self.call_id(rt.str_trim, &[ptr, len, dest]);
            }
            StrOp::Upper | StrOp::Lower => {
                let upper = self.b.ins().iconst(types::I8, (op == StrOp::Upper) as i64);
                self.call_id(rt.str_case, &[ptr, len, upper, dest]);
            }
            StrOp::Join => {
                // The first operand is a list or array: its items pointer and count, then the separator.
                let (sep, sep_len) = self.str_parts(&args[1])?;
                self.call_id(rt.str_join, &[ptr, len, sep, sep_len, dest]);
            }
            StrOp::Split => {
                let (sep, sep_len) = self.str_parts(&args[1])?;
                let (l, f) = self.loc_args(line);
                self.call_id(rt.str_split, &[ptr, len, sep, sep_len, l, f, dest]);
            }
            StrOp::ToInt | StrOp::ToFloat => {
                // The runtime writes the number to a slot; the optional gets it only when it parsed.
                let KTy::Optional(el) = ty else { unreachable!("{op:?} gives an optional") };
                let (slot_ty, num_ty, parse) = match op {
                    StrOp::ToInt => (KTy::I32, types::I32, rt.str_to_int),
                    _ => (KTy::F32, types::F32, rt.str_to_float),
                };
                let slot = self.temp_addr(slot_ty);
                let zero = self.b.ins().iconst(types::I32, 0);
                self.b.ins().store(MemFlagsData::trusted(), zero, slot, 0);
                let ok = self.call_id(parse, &[ptr, len, slot]).expect("returns whether it parsed");
                let value = self.b.ins().load(num_ty, MemFlagsData::trusted(), slot, 0);
                self.b.ins().store(MemFlagsData::trusted(), ok, dest, 0);
                let at = self.shared.kir.payload_offset(el) as i32;
                self.b.ins().store(MemFlagsData::trusted(), value, dest, at);
            }
            StrOp::Chars | StrOp::CharLen | StrOp::Find | StrOp::Starts | StrOp::Ends | StrOp::Cmp => unreachable!("{op:?} is a number"),
            StrOp::Hex | StrOp::BytesReason => unreachable!("{op:?} is written above"),
        }
        Ok(())
    }
}
