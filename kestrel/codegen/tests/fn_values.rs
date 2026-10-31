//! Function values in codegen: the environment word, and calls that pass it on.

use std::collections::HashMap;

use kestrel_codegen::{JitProgram, Options};
use kestrel_kir::*;

mod common;

use common::{call0, jit};

fn expr(kind: ExprKind, ty: KTy) -> Expr {
    Expr { kind, ty, line: 0 }
}

fn load(l: u32, ty: KTy) -> Expr {
    expr(ExprKind::Load(Place::local(LocalIdx(l))), ty)
}

fn local(name: &str, ty: KTy, kind: LocalKind) -> Local {
    Local { name: name.into(), ty, kind }
}

fn func(name: &str, params: u32, locals: Vec<Local>, ret: Option<KTy>, body: Vec<Stmt>) -> Func {
    Func {
        name: name.into(),
        params: (0..params).map(LocalIdx).collect(),
        ret,
        locals,
        body,
        is_pure: true,
        io: false,
        mutates_params: false,
        exported: false,
        lines: (0, 0),
        native: None,
    }
}

/// `add_env(env: i64, x: int) -> int { x + int(env) }`, `call_it(f: fn, x: int) -> int { f(x) }`
/// and `env_of(f: fn) -> i64 { f.env }`: the pieces of a function value whose environment isn't 0.
fn module() -> Module {
    let int_sig = KSig { params: vec![(KTy::I32, false)], ret: Some(KTy::I32), pure: false };
    let env_int = expr(ExprKind::Cast(Box::new(load(0, KTy::I64))), KTy::I32);
    let sum = expr(ExprKind::Binary(BinOp::Add, Box::new(load(1, KTy::I32)), Box::new(env_int)), KTy::I32);
    let add_env = func(
        "add_env",
        2,
        vec![local("env", KTy::I64, LocalKind::Value), local("x", KTy::I32, LocalKind::Value)],
        Some(KTy::I32),
        vec![Stmt::Return(Some(sum))],
    );
    let call = ExprKind::CallIndirect {
        callee: Place::local(LocalIdx(0)),
        args: vec![Arg::Value(load(1, KTy::I32))],
        sig: int_sig,
    };
    let call_it = func(
        "call_it",
        2,
        vec![local("f", KTy::Fn, LocalKind::RefParam), local("x", KTy::I32, LocalKind::Value)],
        Some(KTy::I32),
        vec![Stmt::Return(Some(expr(call, KTy::I32)))],
    );
    let env_of = func(
        "env_of",
        1,
        vec![local("f", KTy::Fn, LocalKind::RefParam)],
        Some(KTy::I64),
        vec![Stmt::Return(Some(expr(ExprKind::FnEnv(Place::local(LocalIdx(0))), KTy::I64)))],
    );
    Module {
        funcs: vec![add_env, call_it, env_of],
        structs: Vec::new(),
        enums: Vec::new(),
        elems: Vec::new(),
        print_fns: HashMap::new(),
        par_bodies: Vec::new(),
        globals: Vec::new(),
        init: None,
        main: None,
        files: Vec::new(),
        tables: Vec::new(),
        not_inlined: Vec::new(),
        assumptions: Vec::new(),
        contracts: Vec::new(),
        copies: HashMap::new(),
        uncopied: Vec::new(),
        hoisted: HashMap::new(),
        analysed: None,
        reload: None,
    }
}

#[test]
fn an_indirect_call_passes_the_environment_and_fn_env_reads_it() {
    let p = JitProgram::compile(&module(), Options::default()).expect("codegen");
    let code = p.fn_ptr("add_env").expect("add_env") as usize as u64;
    // A function value `{code, env}` as memory holds it; the function takes the pointer.
    let value: [u64; 2] = [code, 40];
    let call_it: extern "C" fn(*const [u64; 2], i32) -> i32 = unsafe { std::mem::transmute(p.fn_ptr("call_it").expect("call_it")) };
    let env_of: extern "C" fn(*const [u64; 2]) -> i64 = unsafe { std::mem::transmute(p.fn_ptr("env_of").expect("env_of")) };
    assert_eq!(call_it(&value, 2), 42);
    assert_eq!(env_of(&value), 40);
}

#[test]
fn a_named_function_value_has_a_null_environment() {
    // `f` is `{thunk, 0}`: calling it ignores the environment, and its result is the function's.
    let p = jit("inc(x: int) -> int { x + 1 }\nrun(n: int) -> int {\n let f = inc\n f(n) * 10\n}");
    assert_eq!(common::call1(&p, "run", 4), 50);
    assert_eq!(call0::<i32>(&jit("inc(x: int) -> int { x + 1 }\nr() -> int {\n let f = inc\n let g = f\n g(1) + f(2)\n}"), "r"), 5);
}
