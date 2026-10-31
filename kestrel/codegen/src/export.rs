//! Export thunks: one entry point of a single shape per exported (`pub`)
//! function, `fn(args: *mut u8, ret: *mut u8)`, so a host like the Godot
//! add-on can call any of them. Arguments sit in a byte buffer at the
//! offsets `ExportInfo` gives; the result is written at `ret`. A parameter
//! passed by pointer (`var`, or an aggregate) is passed as its place in that
//! buffer, so a `var` parameter's changes are written back into it.

use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, MemFlagsData, Signature};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module};
use kestrel_kir::{self as kir, KTy, LocalKind, StructIdx};

use crate::func::{cl_type, mem_flags};

/// A parameter or result type, as a host sees it.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportTy {
    Bool,
    U8,
    Int,
    I64,
    Float,
    F64,
    /// `{ ptr, len: i64 }` of UTF-8 bytes.
    Str,
    /// Two or three packed `f32`s.
    Vec2,
    Vec3,
    /// `[T]` (`{ ptr, len }`) or, with `list`, `list[T]` (`{ ptr, len, cap, arena }`).
    Seq { list: bool, elem: Box<ExportTy>, elem_size: u32 },
    /// Anything else, by name: a host can't build or read it.
    Other(String),
}

#[derive(Clone, Debug)]
pub struct ExportInfo {
    pub name: String,
    pub params: Vec<ExportTy>,
    pub ret: Option<ExportTy>,
    /// Where each argument goes in the buffer.
    pub arg_offsets: Vec<u32>,
    /// Bytes the argument buffer needs (8-aligned).
    pub args_size: u32,
    /// Bytes the result needs.
    pub ret_size: u32,
}

pub(crate) fn export_ty(m: &kir::Module, ty: KTy) -> ExportTy {
    match ty {
        KTy::Bool => ExportTy::Bool,
        KTy::U8 => ExportTy::U8,
        KTy::I32 => ExportTy::Int,
        KTy::I64 => ExportTy::I64,
        KTy::F32 => ExportTy::Float,
        KTy::F64 => ExportTy::F64,
        KTy::Str => ExportTy::Str,
        KTy::F32X4 => ExportTy::Other("vec4".into()),
        KTy::I32X4 => ExportTy::Other("ivec4".into()),
        KTy::Struct(StructIdx(0)) => ExportTy::Vec2,
        KTy::Struct(StructIdx(1)) => ExportTy::Vec3,
        KTy::Array(e) | KTy::List(e) => {
            let elem = m.elem(e);
            ExportTy::Seq { list: matches!(ty, KTy::List(_)), elem: Box::new(export_ty(m, elem)), elem_size: m.size_align(elem).0 }
        }
        KTy::Struct(s) => ExportTy::Other(m.layout(s).name.clone()),
        KTy::Enum(e) => ExportTy::Other(m.enum_layout(e).name.clone()),
        KTy::Arena => ExportTy::Other("Arena".into()),
        KTy::Gpu => ExportTy::Other("GpuHandle".into()),
        KTy::Optional(_) => ExportTy::Other("an optional".into()),
        KTy::Map(..) => ExportTy::Other("a map".into()),
        KTy::Fn => ExportTy::Other("a function".into()),
        KTy::Iface => ExportTy::Other("an interface value".into()),
    }
}

/// The symbol of the constants thunk. Kestrel items are `k_<name>` (and the generated thunks
/// `k_export_<name>` and `k_thunk_<name>`), natives are `kestrel_<module>_<name>` and the runtime
/// is `kestrel_rt_*`; no symbol made from a Kestrel name starts with `__`, so this can't collide.
const INIT_THUNK_SYMBOL: &str = "__kestrel_init_thunk";

/// The signature every thunk has: `fn(args: *mut u8, ret: *mut u8)`.
fn thunk_signature<M: Module>(module: &M) -> Signature {
    let ptr = module.target_config().pointer_type();
    let mut sig = Signature::new(module.isa().default_call_conv());
    sig.params.extend([AbiParam::new(ptr), AbiParam::new(ptr)]);
    sig
}

/// Defines a thunk for each exported function; `ids` are the functions'.
pub(crate) fn define_thunks<M: Module>(module: &mut M, m: &kir::Module, ids: &[FuncId]) -> Result<Vec<(ExportInfo, FuncId)>, String> {
    let sig = thunk_signature(module);
    let mut out = Vec::new();
    let mut fb_ctx = FunctionBuilderContext::new();
    for (i, f) in m.funcs.iter().enumerate().filter(|(_, f)| f.exported) {
        let param_tys: Vec<KTy> = f.params.iter().map(|p| f.locals[p.0 as usize].ty).collect();
        let (mut arg_offsets, mut end) = (Vec::new(), 0u32);
        for &t in &param_tys {
            let (size, align) = m.size_align(t);
            let at = end.next_multiple_of(align.max(1));
            arg_offsets.push(at);
            end = at + size;
        }
        // A reloadable program's generated `__kestrel_reload_drop` is found as `reload_drop` (the
        // checker refuses a `pub reload_drop` of the program's own); its thunk's symbol keeps the
        // whole name. Every other export keeps its name.
        let name = if f.name == kir::RELOAD_DROP { "reload_drop" } else { &f.name };
        let info = ExportInfo {
            name: name.to_string(),
            params: param_tys.iter().map(|&t| export_ty(m, t)).collect(),
            ret: f.ret.map(|t| export_ty(m, t)),
            arg_offsets,
            args_size: end.next_multiple_of(8),
            ret_size: f.ret.map_or(0, |t| m.size_align(t).0),
        };
        let id = module
            .declare_function(&format!("k_export_{}", f.name), Linkage::Local, &sig)
            .map_err(|e| e.to_string())?;
        let mut func = Function::with_name_signature(Default::default(), sig.clone());
        {
            let mut b = FunctionBuilder::new(&mut func, &mut fb_ctx);
            let block = b.create_block();
            b.append_block_params_for_function_params(block);
            b.switch_to_block(block);
            b.seal_block(block);
            let (args, ret) = (b.block_params(block)[0], b.block_params(block)[1]);
            let mut vals = Vec::new();
            if f.ret.is_some_and(|r| r.is_aggregate()) {
                vals.push(ret);
            }
            for ((p, &t), &at) in f.params.iter().zip(&param_tys).zip(&info.arg_offsets) {
                vals.push(if f.locals[p.0 as usize].kind == LocalKind::RefParam {
                    b.ins().iadd_imm_s(args, at as i64)
                } else {
                    b.ins().load(cl_type(t), mem_flags(t), args, at as i32)
                });
            }
            let fref = module.declare_func_in_func(ids[i], b.func);
            let call = b.ins().call(fref, &vals);
            if f.ret.is_some_and(|r| !r.is_aggregate()) {
                let v = b.inst_results(call)[0];
                b.ins().store(f.ret.map_or(MemFlagsData::trusted(), mem_flags), v, ret, 0);
            }
            b.ins().return_(&[]);
            b.finalize(module.target_config());
        }
        let mut ctx = module.make_context();
        ctx.func = func;
        module.define_function(id, &mut ctx).map_err(|e| format!("export thunk for `{}`: {e:?}", f.name))?;
        out.push((info, id));
    }
    Ok(out)
}

/// Defines the thunk that runs the function building the heap constants (`kir::Module::init`),
/// if the module has one. It has the shape of an export thunk, and takes no arguments and
/// gives no result, so a host passes null for both. It is kept apart from the exports.
pub(crate) fn define_init_thunk<M: Module>(module: &mut M, m: &kir::Module, ids: &[FuncId]) -> Result<Option<FuncId>, String> {
    let Some(init) = m.init else { return Ok(None) };
    let sig = thunk_signature(module);
    let id = module.declare_function(INIT_THUNK_SYMBOL, Linkage::Local, &sig).map_err(|e| e.to_string())?;
    let mut func = Function::with_name_signature(Default::default(), sig);
    let mut fb_ctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut func, &mut fb_ctx);
        let block = b.create_block();
        b.append_block_params_for_function_params(block);
        b.switch_to_block(block);
        b.seal_block(block);
        let fref = module.declare_func_in_func(ids[init.0 as usize], b.func);
        b.ins().call(fref, &[]);
        b.ins().return_(&[]);
        b.finalize(module.target_config());
    }
    let mut ctx = module.make_context();
    ctx.func = func;
    module.define_function(id, &mut ctx).map_err(|e| format!("constants thunk: {e:?}"))?;
    Ok(Some(id))
}
