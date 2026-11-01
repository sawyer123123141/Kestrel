mod aggregate;
mod arenas;
mod arith;
mod assume;
mod bytes;
mod call;
mod check;
mod checksum;
mod collections;
mod divide;
mod expr;
mod fnvalue;
mod globals;
mod headers;
mod iface;
mod lists;
mod maps;
mod math;
mod par;
mod place;
mod print;
mod select;
mod simd;
mod simd_check;
mod simd_math;
mod stmt;
mod strings;
mod wide;
mod wide_atan;
mod wide_exp;
mod wide_pick;
mod wide_trig;

use std::collections::HashMap;

use cranelift_codegen::ir::{
    types, AbiParam, Block, Function, InstBuilder, Signature, StackSlot, StackSlotData, StackSlotKind, TrapCode,
    Type, Value,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataId, FuncId, Module};
use kestrel_kir::{self as kir, KTy, LocalIdx, LocalKind};

use crate::rt::RtFuncs;
pub(crate) use check::Check;
pub(crate) use simd::mem_flags;
use crate::Options;

/// The register type of a number or a vector. Structs live in memory and have none.
pub(crate) fn cl_type(t: KTy) -> Type {
    match t {
        KTy::Bool | KTy::U8 => types::I8,
        KTy::I32 => types::I32,
        KTy::I64 => types::I64,
        KTy::F32 => types::F32,
        KTy::F64 => types::F64,
        KTy::F32X4 => types::F32X4,
        KTy::I32X4 => types::I32X4,
        KTy::Struct(_)
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
            unreachable!("aggregates live in memory, not registers")
        }
    }
}

/// A struct result is written through a pointer the caller passes first.
pub(crate) fn signature(f: &kir::Func, cc: CallConv, ptr: Type) -> Signature {
    let mut sig = Signature::new(cc);
    let ret = f.ret.filter(|r| !r.is_aggregate());
    if f.ret.is_some_and(|r| r.is_aggregate()) {
        sig.params.push(AbiParam::new(ptr));
    }
    for p in &f.params {
        let local = &f.locals[p.0 as usize];
        let ty = if local.kind == LocalKind::RefParam { ptr } else { cl_type(local.ty) };
        sig.params.push(AbiParam::new(ty));
    }
    if let Some(r) = ret {
        sig.returns.push(AbiParam::new(cl_type(r)));
    }
    sig
}

/// String literal data, shared across all functions of a module.
#[derive(Default)]
pub(crate) struct Strings {
    ids: HashMap<String, DataId>,
}

/// Where a local's value lives.
#[derive(Clone, Copy)]
enum Slot {
    Var(Variable),
    Stack(StackSlot),
    /// Variable holding a pointer to the value.
    Ptr(Variable),
    /// A local nothing uses (see `LocalKind::Unused`).
    Unused,
}

pub(crate) struct Shared<'a> {
    pub funcs: &'a [FuncId],
    /// The thunk of each `par for` body, by `FuncIdx`.
    pub par_thunks: &'a HashMap<u32, FuncId>,
    pub rt: &'a RtFuncs,
    /// The data of each global, by `GlobalIdx`.
    pub globals: &'a [DataId],
    /// Each source file's `FileName` record, by the file index in a KIR line.
    pub files: &'a [DataId],
    /// The data of each method table, by `TableIdx`.
    pub tables: &'a [DataId],
    pub opts: Options,
    /// The whole program: struct layouts and callees' signatures.
    pub kir: &'a kir::Module,
}

pub(crate) struct FuncLowerer<'a, 'b, M: Module> {
    module: &'a mut M,
    strings: &'a mut Strings,
    shared: &'a Shared<'a>,
    b: FunctionBuilder<'b>,
    f: &'a kir::Func,
    ptr: Type,
    slots: Vec<Slot>,
    /// Where a struct result goes, for functions that return one.
    sret: Option<Value>,
    /// (continue target, break target) of each enclosing loop.
    loops: Vec<(Block, Block)>,
    /// The list headers kept in variables for the loops being lowered (see `headers`).
    hdr_cache: Vec<headers::Cached>,
    /// Values known to be multiples of `2^k`, as `(value, k)`, in the code being lowered: the
    /// side of a passed `n % 2^k == 0` test (see `select`).
    exact: Vec<(Value, u32)>,
}

pub(crate) fn lower_func<M: Module>(
    module: &mut M,
    strings: &mut Strings,
    shared: &Shared,
    func: &mut Function,
    fb_ctx: &mut FunctionBuilderContext,
    f: &kir::Func,
) -> Result<(), String> {
    let ptr = module.target_config().pointer_type();
    let mut b = FunctionBuilder::new(func, fb_ctx);
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    b.seal_block(entry);
    let mut params: Vec<Value> = b.block_params(entry).to_vec();
    let sret = if f.ret.is_some_and(|r| r.is_aggregate()) { Some(params.remove(0)) } else { None };

    let mut slots = Vec::with_capacity(f.locals.len());
    for local in &f.locals {
        let slot = match local.kind {
            LocalKind::Value => {
                Slot::Var(b.declare_var(cl_type(local.ty)))
            }
            LocalKind::RefParam => {
                Slot::Ptr(b.declare_var(ptr))
            }
            LocalKind::Addressed => Slot::Stack(stack_slot(&mut b, shared.kir, local.ty)),
            LocalKind::Unused => Slot::Unused,
        };
        slots.push(slot);
    }
    for (p, &val) in f.params.iter().zip(&params) {
        match slots[p.0 as usize] {
            Slot::Var(v) | Slot::Ptr(v) => b.def_var(v, val),
            Slot::Stack(ss) => {
                b.ins().stack_store(ptr, val, ss, 0);
            }
            Slot::Unused => unreachable!("a parameter is used"),
        }
    }

    let mut fl = FuncLowerer { module, strings, shared, b, f, ptr, slots, sret, loops: Vec::new(), hdr_cache: Vec::new(), exact: Vec::new() };
    if fl.stmts(&f.body)? {
        if f.ret.is_some() {
            // The checker guarantees value-returning bodies don't fall off the end.
            fl.b.ins().trap(TrapCode::unwrap_user(1));
        } else {
            fl.b.ins().return_(&[]);
        }
    }
    fl.b.seal_all_blocks();
    let config = fl.module.target_config();
    fl.b.finalize(config);
    // Shape the IR so Cranelift hoists loop-invariant code and lays loops out well (see each module).
    crate::phis::remove_redundant_params(func);
    crate::loop_exits::put_loop_exits_first(func);
    Ok(())
}

/// A stack slot big enough and aligned enough for a value of `ty`.
fn stack_slot(b: &mut FunctionBuilder, kir: &kir::Module, ty: KTy) -> StackSlot {
    let (size, align) = kir.size_align(ty);
    let data = StackSlotData::new(StackSlotKind::ExplicitSlot, size.max(1), align.trailing_zeros() as u8);
    b.create_sized_stack_slot(data)
}

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    fn local_ty(&self, l: LocalIdx) -> KTy {
        self.f.locals[l.0 as usize].ty
    }

    /// A fresh stack slot for an intermediate struct value; returns its address.
    pub(crate) fn temp_addr(&mut self, ty: KTy) -> Value {
        let ss = stack_slot(&mut self.b, self.shared.kir, ty);
        self.b.ins().stack_addr(self.ptr, ss, 0)
    }
}
