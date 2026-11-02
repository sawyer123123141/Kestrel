use cranelift_codegen::ir::{types, AbiParam, Signature, Type};
use cranelift_module::{DataId, FuncId, Linkage, Module};
use kestrel_kir::MathOp;

/// Runtime functions, declared as imports in a module.
pub(crate) struct RtFuncs {
    pub print_i64: FuncId,
    pub print_f32: FuncId,
    pub print_f64: FuncId,
    pub print_bool: FuncId,
    pub print_str: FuncId,
    pub print_space: FuncId,
    pub print_newline: FuncId,
    pub panic: FuncId,
    pub proof_failed: FuncId,
    pub fail: FuncId,
    pub exit: FuncId,
    pub assume_failed: FuncId,
    pub fmod_f32: FuncId,
    pub fmod_f64: FuncId,
    pub alloc: FuncId,
    pub dup: FuncId,
    pub list_grow: FuncId,
    pub list_remove: FuncId,
    pub list_insert: FuncId,
    pub list_clear: FuncId,
    pub list_compact: FuncId,
    /// Gives a list room for exactly the bytes `to_bytes` writes.
    pub list_reserve: FuncId,
    /// Writes a string's length and bytes into a `to_bytes` result.
    pub bytes_write_str: FuncId,
    /// Makes a string of bytes `from_bytes` reads, when they are UTF-8.
    pub bytes_read_str: FuncId,
    pub copy_items: FuncId,
    pub current_arena: FuncId,
    pub panic_index: FuncId,
    pub arena_new: FuncId,
    pub arena_new_static: FuncId,
    pub arena_free: FuncId,
    pub arena_push: FuncId,
    pub arena_pop: FuncId,
    pub map_find: FuncId,
    pub map_insert: FuncId,
    pub map_remove: FuncId,
    pub map_dup: FuncId,
    pub map_copy_into: FuncId,
    pub str_eq: FuncId,
    pub str_chars: FuncId,
    pub str_at: FuncId,
    pub str_char_len: FuncId,
    pub str_piece: FuncId,
    pub str_find: FuncId,
    pub str_starts: FuncId,
    pub str_ends: FuncId,
    pub str_cmp: FuncId,
    pub str_slice: FuncId,
    pub str_trim: FuncId,
    pub str_case: FuncId,
    pub str_hex: FuncId,
    pub str_join: FuncId,
    pub str_split: FuncId,
    pub str_to_int: FuncId,
    pub str_to_float: FuncId,
    pub par_for: FuncId,
    pub sort: FuncId,
    /// The sorts by number keys that a generated fill function writes (`ListOp::SortBy`).
    pub sort_keyed_int: FuncId,
    pub sort_keyed_float: FuncId,
    /// The function for each float operation the runtime does (see `MATH`).
    pub math: Vec<(MathOp, FuncId)>,
    /// The same operations over `f32`, which `float` code calls (see `MATHF`).
    pub mathf: Vec<(MathOp, FuncId)>,
    pub rand_float: FuncId,
    pub rand_int: FuncId,
    pub rand_seed: FuncId,
    pub str_begin: FuncId,
    pub str_end: FuncId,
    pub gpu_drop: FuncId,
    /// Sets the float mode back after a call into the window library.
    pub fpu_pin: FuncId,
    /// Mixes a string's bytes into a `checksum` fold.
    pub hash_bytes: FuncId,
    /// The runtime's `kestrel_rt_call_line`, the line of the latest call into a built-in function.
    pub call_line: DataId,
    /// The runtime's `kestrel_rt_call_file`, the file of that call.
    pub call_file: DataId,
}

impl RtFuncs {
    pub fn declare<M: Module>(module: &mut M) -> Result<Self, String> {
        let cc = module.isa().default_call_conv();
        let ptr = module.target_config().pointer_type();
        let call_line = module.declare_data("kestrel_rt_call_line", Linkage::Import, true, false).map_err(|e| e.to_string())?;
        let call_file = module.declare_data("kestrel_rt_call_file", Linkage::Import, true, false).map_err(|e| e.to_string())?;
        let mut decl = |name: &str, params: &[Type], ret: Option<Type>| -> Result<FuncId, String> {
            let mut sig = Signature::new(cc);
            // The C ABI leaves an i8 argument's upper bits undefined unless the caller extends it.
            sig.params.extend(params.iter().map(|&p| {
                if p == types::I8 {
                    AbiParam::new(p).uext()
                } else {
                    AbiParam::new(p)
                }
            }));
            sig.returns.extend(ret.map(AbiParam::new));
            module.declare_function(name, Linkage::Import, &sig).map_err(|e| e.to_string())
        };
        use types::{F32, F64, I32, I64, I8};
        Ok(RtFuncs {
            print_i64: decl("kestrel_rt_print_i64", &[I64], None)?,
            print_f32: decl("kestrel_rt_print_f32", &[F32], None)?,
            print_f64: decl("kestrel_rt_print_f64", &[F64], None)?,
            print_bool: decl("kestrel_rt_print_bool", &[I8], None)?,
            print_str: decl("kestrel_rt_print_str", &[ptr, I64], None)?,
            print_space: decl("kestrel_rt_print_space", &[], None)?,
            print_newline: decl("kestrel_rt_print_newline", &[], None)?,
            panic: decl("kestrel_rt_panic", &[I32, I32, ptr], None)?,
            proof_failed: decl("kestrel_rt_proof_failed", &[I32, I32, ptr], None)?,
            fail: decl("kestrel_rt_fail", &[ptr, I64, I32, ptr], None)?,
            exit: decl("kestrel_rt_exit", &[I32, I32, ptr], None)?,
            assume_failed: decl("kestrel_rt_assume_failed", &[ptr, I64, I32, ptr], None)?,
            fmod_f32: decl("kestrel_rt_fmod_f32", &[F32, F32], Some(F32))?,
            fmod_f64: decl("kestrel_rt_fmod_f64", &[F64, F64], Some(F64))?,
            alloc: decl("kestrel_rt_alloc", &[I64, I64, I64, I32, ptr], Some(ptr))?,
            dup: decl("kestrel_rt_dup", &[ptr, I64, I64], Some(ptr))?,
            list_grow: decl("kestrel_rt_list_grow", &[ptr, I64, I64], None)?,
            list_remove: decl("kestrel_rt_list_remove", &[ptr, I64, I64, I32, ptr], None)?,
            list_insert: decl("kestrel_rt_list_insert", &[ptr, I64, I64, I64, I32, ptr], Some(ptr))?,
            list_clear: decl("kestrel_rt_list_clear", &[ptr, I64], None)?,
            list_compact: decl("kestrel_rt_list_compact", &[ptr, I64, I64, I64], None)?,
            list_reserve: decl("kestrel_rt_list_reserve", &[ptr, I64, I64, I64], None)?,
            bytes_write_str: decl("kestrel_rt_bytes_write_str", &[ptr, ptr, I64], None)?,
            bytes_read_str: decl("kestrel_rt_bytes_read_str", &[ptr, I64, ptr], Some(I8))?,
            copy_items: decl("kestrel_rt_copy_items", &[ptr, ptr, I64, I64], None)?,
            current_arena: decl("kestrel_rt_current_arena", &[], Some(ptr))?,
            panic_index: decl("kestrel_rt_panic_index", &[I64, I64, I32, ptr], None)?,
            arena_new: decl("kestrel_rt_arena_new", &[], Some(ptr))?,
            arena_new_static: decl("kestrel_rt_arena_new_static", &[], Some(ptr))?,
            arena_free: decl("kestrel_rt_arena_free", &[ptr], None)?,
            arena_push: decl("kestrel_rt_arena_push", &[ptr], None)?,
            arena_pop: decl("kestrel_rt_arena_pop", &[], Some(ptr))?,
            map_find: decl("kestrel_rt_map_find", &[ptr, ptr, I32, I64], Some(ptr))?,
            map_insert: decl("kestrel_rt_map_insert", &[ptr, ptr, I32, I64, I64, I32, ptr], Some(ptr))?,
            map_remove: decl("kestrel_rt_map_remove", &[ptr, ptr, I32, I64], Some(I8))?,
            map_dup: decl("kestrel_rt_map_dup", &[ptr, I64, I64], None)?,
            map_copy_into: decl("kestrel_rt_map_copy_into", &[ptr, ptr, I64, I64], Some(I8))?,
            str_eq: decl("kestrel_rt_str_eq", &[ptr, I64, ptr, I64], Some(I8))?,
            str_chars: decl("kestrel_rt_str_chars", &[ptr, I64], Some(I64))?,
            str_at: decl("kestrel_rt_str_at", &[ptr, I64, I64, I32, ptr, ptr], None)?,
            str_char_len: decl("kestrel_rt_str_char_len", &[ptr, I64, I64], Some(I64))?,
            str_piece: decl("kestrel_rt_str_piece", &[ptr, I64, I64, I64, ptr], None)?,
            str_find: decl("kestrel_rt_str_find", &[ptr, I64, ptr, I64], Some(I64))?,
            str_starts: decl("kestrel_rt_str_starts", &[ptr, I64, ptr, I64], Some(I8))?,
            str_ends: decl("kestrel_rt_str_ends", &[ptr, I64, ptr, I64], Some(I8))?,
            str_cmp: decl("kestrel_rt_str_cmp", &[ptr, I64, ptr, I64], Some(I8))?,
            str_slice: decl("kestrel_rt_str_slice", &[ptr, I64, I64, I64, I32, ptr, ptr], None)?,
            str_trim: decl("kestrel_rt_str_trim", &[ptr, I64, ptr], None)?,
            str_case: decl("kestrel_rt_str_case", &[ptr, I64, I8, ptr], None)?,
            str_hex: decl("kestrel_rt_str_hex", &[I64, I64, ptr], None)?,
            str_join: decl("kestrel_rt_str_join", &[ptr, I64, ptr, I64, ptr], None)?,
            str_split: decl("kestrel_rt_str_split", &[ptr, I64, ptr, I64, I32, ptr, ptr], None)?,
            str_to_int: decl("kestrel_rt_str_to_int", &[ptr, I64, ptr], Some(I8))?,
            str_to_float: decl("kestrel_rt_str_to_float", &[ptr, I64, ptr], Some(I8))?,
            par_for: decl("kestrel_rt_par_for", &[I64, ptr, ptr], None)?,
            sort: decl("kestrel_rt_sort", &[ptr, I64, I64, ptr, ptr], None)?,
            sort_keyed_int: decl("kestrel_rt_sort_keyed_int", &[ptr, I64, I64, ptr, ptr, ptr], None)?,
            sort_keyed_float: decl("kestrel_rt_sort_keyed_float", &[ptr, I64, I64, ptr, ptr, ptr], None)?,
            math: MATH.iter().map(|&(op, name, args)| Ok((op, decl(name, &[F64, F64][..args], Some(F64))?))).collect::<Result<_, String>>()?,
            mathf: MATHF.iter().map(|&(op, name, args)| Ok((op, decl(name, &[F32, F32][..args], Some(F32))?))).collect::<Result<_, String>>()?,
            rand_float: decl("kestrel_rt_rand_float", &[], Some(F64))?,
            rand_int: decl("kestrel_rt_rand_int", &[I64, I64], Some(I64))?,
            rand_seed: decl("kestrel_rt_rand_seed", &[I64], None)?,
            str_begin: decl("kestrel_rt_str_begin", &[], None)?,
            str_end: decl("kestrel_rt_str_end", &[ptr], None)?,
            gpu_drop: decl("kestrel_rt_gpu_drop", &[I64], None)?,
            fpu_pin: decl("kestrel_rt_fpu_pin", &[], None)?,
            hash_bytes: decl("kestrel_rt_hash_bytes", &[I64, ptr, I64], Some(I64))?,
            call_line,
            call_file,
        })
    }
}

/// The float operations the runtime does: each one's function and how many `f64`s it takes.
pub(crate) const MATH: [(MathOp, &str, usize); 10] = [
    (MathOp::Sin, "kestrel_rt_sin", 1),
    (MathOp::Cos, "kestrel_rt_cos", 1),
    (MathOp::Tan, "kestrel_rt_tan", 1),
    (MathOp::Asin, "kestrel_rt_asin", 1),
    (MathOp::Acos, "kestrel_rt_acos", 1),
    (MathOp::Atan, "kestrel_rt_atan", 1),
    (MathOp::Atan2, "kestrel_rt_atan2", 2),
    (MathOp::Exp, "kestrel_rt_exp", 1),
    (MathOp::Log, "kestrel_rt_log", 1),
    (MathOp::Pow, "kestrel_rt_pow", 2),
];

/// The same operations over `f32`: the wide functions `float` code calls. The compiler passes the
/// `float` straight to the wide function (or its inline copy), which computes in `f64` and rounds
/// once. Each one's function and how many `f32`s it takes.
pub(crate) const MATHF: [(MathOp, &str, usize); 10] = [
    (MathOp::Sin, "kestrel_rt_sinf", 1),
    (MathOp::Cos, "kestrel_rt_cosf", 1),
    (MathOp::Tan, "kestrel_rt_tanf", 1),
    (MathOp::Asin, "kestrel_rt_asinf", 1),
    (MathOp::Acos, "kestrel_rt_acosf", 1),
    (MathOp::Atan, "kestrel_rt_atanf", 1),
    (MathOp::Atan2, "kestrel_rt_atan2f", 2),
    (MathOp::Exp, "kestrel_rt_expf", 1),
    (MathOp::Log, "kestrel_rt_logf", 1),
    (MathOp::Pow, "kestrel_rt_powf", 2),
];

/// Addresses of every runtime export, for registering with the JIT.
pub(crate) fn jit_symbols() -> Vec<(&'static str, *const u8)> {
    use kestrel_runtime::{arena, bytes, checksum, fpu, gpu, list, map, map_copy, math, panic, par, print, sort, start, string, string_find, string_make, string_parse};
    vec![
        ("kestrel_rt_print_i64", print::kestrel_rt_print_i64 as *const u8),
        ("kestrel_rt_print_f32", print::kestrel_rt_print_f32 as *const u8),
        ("kestrel_rt_print_f64", print::kestrel_rt_print_f64 as *const u8),
        ("kestrel_rt_print_bool", print::kestrel_rt_print_bool as *const u8),
        ("kestrel_rt_print_str", print::kestrel_rt_print_str as *const u8),
        ("kestrel_rt_print_space", print::kestrel_rt_print_space as *const u8),
        ("kestrel_rt_print_newline", print::kestrel_rt_print_newline as *const u8),
        ("kestrel_rt_panic", panic::kestrel_rt_panic as *const u8),
        ("kestrel_rt_proof_failed", panic::kestrel_rt_proof_failed as *const u8),
        ("kestrel_rt_fail", panic::kestrel_rt_fail as *const u8),
        ("kestrel_rt_exit", panic::kestrel_rt_exit as *const u8),
        ("kestrel_rt_assume_failed", panic::kestrel_rt_assume_failed as *const u8),
        ("kestrel_rt_fmod_f32", panic::kestrel_rt_fmod_f32 as *const u8),
        ("kestrel_rt_fmod_f64", panic::kestrel_rt_fmod_f64 as *const u8),
        ("kestrel_rt_start", start::kestrel_rt_start as *const u8),
        ("kestrel_rt_alloc", arena::kestrel_rt_alloc as *const u8),
        ("kestrel_rt_dup", arena::kestrel_rt_dup as *const u8),
        ("kestrel_rt_list_grow", arena::kestrel_rt_list_grow as *const u8),
        ("kestrel_rt_list_remove", list::kestrel_rt_list_remove as *const u8),
        ("kestrel_rt_list_insert", list::kestrel_rt_list_insert as *const u8),
        ("kestrel_rt_list_clear", list::kestrel_rt_list_clear as *const u8),
        ("kestrel_rt_list_compact", list::kestrel_rt_list_compact as *const u8),
        ("kestrel_rt_copy_items", list::kestrel_rt_copy_items as *const u8),
        ("kestrel_rt_list_reserve", bytes::kestrel_rt_list_reserve as *const u8),
        ("kestrel_rt_bytes_write_str", bytes::kestrel_rt_bytes_write_str as *const u8),
        ("kestrel_rt_bytes_read_str", bytes::kestrel_rt_bytes_read_str as *const u8),
        ("kestrel_rt_current_arena", arena::kestrel_rt_current_arena as *const u8),
        ("kestrel_rt_panic_index", panic::kestrel_rt_panic_index as *const u8),
        ("kestrel_rt_arena_new", arena::kestrel_rt_arena_new as *const u8),
        ("kestrel_rt_arena_new_static", arena::kestrel_rt_arena_new_static as *const u8),
        ("kestrel_rt_arena_free", arena::kestrel_rt_arena_free as *const u8),
        ("kestrel_rt_arena_push", arena::kestrel_rt_arena_push as *const u8),
        ("kestrel_rt_arena_pop", arena::kestrel_rt_arena_pop as *const u8),
        ("kestrel_rt_map_find", map::kestrel_rt_map_find as *const u8),
        ("kestrel_rt_map_insert", map::kestrel_rt_map_insert as *const u8),
        ("kestrel_rt_map_remove", map::kestrel_rt_map_remove as *const u8),
        ("kestrel_rt_map_dup", map::kestrel_rt_map_dup as *const u8),
        ("kestrel_rt_map_copy_into", map_copy::kestrel_rt_map_copy_into as *const u8),
        ("kestrel_rt_str_eq", map::kestrel_rt_str_eq as *const u8),
        ("kestrel_rt_str_chars", string::kestrel_rt_str_chars as *const u8),
        ("kestrel_rt_str_at", string::kestrel_rt_str_at as *const u8),
        ("kestrel_rt_str_char_len", string::kestrel_rt_str_char_len as *const u8),
        ("kestrel_rt_str_piece", string::kestrel_rt_str_piece as *const u8),
        ("kestrel_rt_str_find", string_find::kestrel_rt_str_find as *const u8),
        ("kestrel_rt_str_starts", string_find::kestrel_rt_str_starts as *const u8),
        ("kestrel_rt_str_ends", string_find::kestrel_rt_str_ends as *const u8),
        ("kestrel_rt_str_cmp", sort::kestrel_rt_str_cmp as *const u8),
        ("kestrel_rt_str_slice", string_find::kestrel_rt_str_slice as *const u8),
        ("kestrel_rt_str_trim", string_make::kestrel_rt_str_trim as *const u8),
        ("kestrel_rt_str_case", string_make::kestrel_rt_str_case as *const u8),
        ("kestrel_rt_str_hex", string_make::kestrel_rt_str_hex as *const u8),
        ("kestrel_rt_str_join", string_make::kestrel_rt_str_join as *const u8),
        ("kestrel_rt_str_split", string_make::kestrel_rt_str_split as *const u8),
        ("kestrel_rt_str_to_int", string_parse::kestrel_rt_str_to_int as *const u8),
        ("kestrel_rt_str_to_float", string_parse::kestrel_rt_str_to_float as *const u8),
        ("kestrel_rt_par_for", par::kestrel_rt_par_for as *const u8),
        ("kestrel_rt_sort", sort::kestrel_rt_sort as *const u8),
        ("kestrel_rt_sort_keyed_int", sort::kestrel_rt_sort_keyed_int as *const u8),
        ("kestrel_rt_sort_keyed_float", sort::kestrel_rt_sort_keyed_float as *const u8),
        ("kestrel_rt_sin", math::kestrel_rt_sin as *const u8),
        ("kestrel_rt_cos", math::kestrel_rt_cos as *const u8),
        ("kestrel_rt_tan", math::kestrel_rt_tan as *const u8),
        ("kestrel_rt_asin", math::kestrel_rt_asin as *const u8),
        ("kestrel_rt_acos", math::kestrel_rt_acos as *const u8),
        ("kestrel_rt_atan", math::kestrel_rt_atan as *const u8),
        ("kestrel_rt_atan2", math::kestrel_rt_atan2 as *const u8),
        ("kestrel_rt_exp", math::kestrel_rt_exp as *const u8),
        ("kestrel_rt_log", math::kestrel_rt_log as *const u8),
        ("kestrel_rt_pow", math::kestrel_rt_pow as *const u8),
        ("kestrel_rt_sinf", math::kestrel_rt_sinf as *const u8),
        ("kestrel_rt_cosf", math::kestrel_rt_cosf as *const u8),
        ("kestrel_rt_tanf", math::kestrel_rt_tanf as *const u8),
        ("kestrel_rt_asinf", math::kestrel_rt_asinf as *const u8),
        ("kestrel_rt_acosf", math::kestrel_rt_acosf as *const u8),
        ("kestrel_rt_atanf", math::kestrel_rt_atanf as *const u8),
        ("kestrel_rt_atan2f", math::kestrel_rt_atan2f as *const u8),
        ("kestrel_rt_expf", math::kestrel_rt_expf as *const u8),
        ("kestrel_rt_logf", math::kestrel_rt_logf as *const u8),
        ("kestrel_rt_powf", math::kestrel_rt_powf as *const u8),
        ("kestrel_rt_rand_float", math::kestrel_rt_rand_float as *const u8),
        ("kestrel_rt_rand_int", math::kestrel_rt_rand_int as *const u8),
        ("kestrel_rt_rand_seed", math::kestrel_rt_rand_seed as *const u8),
        ("kestrel_rt_str_begin", print::kestrel_rt_str_begin as *const u8),
        ("kestrel_rt_str_end", print::kestrel_rt_str_end as *const u8),
        ("kestrel_rt_gpu_drop", gpu::kestrel_rt_gpu_drop as *const u8),
        ("kestrel_rt_fpu_pin", fpu::kestrel_rt_fpu_pin as *const u8),
        ("kestrel_rt_hash_bytes", checksum::kestrel_rt_hash_bytes as *const u8),
        ("kestrel_rt_call_line", std::ptr::addr_of!(panic::kestrel_rt_call_line).cast::<u8>()),
        ("kestrel_rt_call_file", std::ptr::addr_of!(panic::kestrel_rt_call_file).cast::<u8>()),
    ]
}

#[cfg(test)]
mod tests {
    use cranelift_codegen::ir::ArgumentExtension;
    use cranelift_module::{default_libcall_names, Module};
    use cranelift_object::{ObjectBuilder, ObjectModule};

    use super::RtFuncs;
    use crate::isa;

    #[test]
    fn i8_runtime_arguments_are_zero_extended() {
        let builder = ObjectBuilder::new(isa::target_isa().unwrap(), "t", default_libcall_names()).unwrap();
        let mut module = ObjectModule::new(builder);
        let rt = RtFuncs::declare(&mut module).unwrap();
        let sig = &module.declarations().get_function_decl(rt.print_bool).signature;
        assert_eq!(sig.params[0].extension, ArgumentExtension::Uext);
        // Only the i8 parameter is extended.
        let sig = &module.declarations().get_function_decl(rt.print_i64).signature;
        assert_eq!(sig.params[0].extension, ArgumentExtension::None);
    }

    #[test]
    fn every_float_operation_has_a_float_runtime_function() {
        use super::{jit_symbols, MATH, MATHF};
        assert_eq!(MATHF.len(), MATH.len());
        for &(op, name, args) in &MATH {
            let found: Vec<_> = MATHF.iter().filter(|&&(o, _, _)| o == op).collect();
            assert_eq!(found.len(), 1, "{op:?} appears once in MATHF");
            assert_eq!(found[0].2, args, "{op:?} takes {args} arguments");
            assert_eq!(found[0].1, format!("{name}f"));
        }
        let symbols = jit_symbols();
        for &(_, name, _) in &MATHF {
            assert!(symbols.iter().any(|(s, _)| *s == name), "{name} is registered with the JIT");
        }
    }
}
