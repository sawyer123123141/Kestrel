//! Interface values in codegen: method tables are data holding function addresses, and a call
//! through a value loads its method from the table and passes the data pointer as `self`.

use std::collections::HashMap;

use kestrel_codegen::{JitProgram, Options};
use kestrel_kir::*;

fn expr(kind: ExprKind, ty: KTy) -> Expr {
    Expr { kind, ty, line: 0 }
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

/// One table (type id 7) whose only method is `get(var self: i64, add: i64) -> i64 { self + add }`;
/// `call_get(d: iface, add: i64)` calls it through an interface value; `lend(var x: i64, var out:
/// iface)` writes an interface value borrowing `x` into `out` (a test of the layout only: in a
/// program, a borrowed value is only ever a call's argument).
fn module() -> Module {
    let self_plus = expr(
        ExprKind::Binary(
            BinOp::Add,
            Box::new(expr(ExprKind::Load(Place::local(LocalIdx(0))), KTy::I64)),
            Box::new(expr(ExprKind::Load(Place::local(LocalIdx(1))), KTy::I64)),
        ),
        KTy::I64,
    );
    let get = func(
        "get",
        2,
        vec![local("self", KTy::I64, LocalKind::RefParam), local("add", KTy::I64, LocalKind::Value)],
        Some(KTy::I64),
        vec![Stmt::Return(Some(self_plus))],
    );
    let call = ExprKind::CallVirtual {
        recv: Place::local(LocalIdx(0)),
        slot: 0,
        args: vec![Arg::Value(expr(ExprKind::Load(Place::local(LocalIdx(1))), KTy::I64))],
        sig: KSig { params: vec![(KTy::I64, false)], ret: Some(KTy::I64), pure: false },
        keeps: ContractSet::default(),
        // (No interface method of its own: it names a function only for messages.)
        method: FuncIdx(0),
    };
    let call_get = func(
        "call_get",
        2,
        vec![local("d", KTy::Iface, LocalKind::RefParam), local("add", KTy::I64, LocalKind::Value)],
        Some(KTy::I64),
        vec![Stmt::Return(Some(expr(call, KTy::I64)))],
    );
    let make = ExprKind::MakeIface { table: TableIdx(0), data: IfaceData::Borrowed(Place::local(LocalIdx(0))) };
    let lend = func(
        "lend",
        2,
        vec![local("x", KTy::I64, LocalKind::RefParam), local("out", KTy::Iface, LocalKind::RefParam)],
        None,
        vec![Stmt::Assign(Place::local(LocalIdx(1)), expr(make, KTy::Iface))],
    );
    let stub = |name: &str| func(name, 1, vec![local("data", KTy::I64, LocalKind::Value)], Some(KTy::I64), vec![Stmt::Trap]);
    let print = func("print_it", 1, vec![local("value", KTy::I64, LocalKind::RefParam)], None, Vec::new());
    Module {
        funcs: vec![get, call_get, lend, stub("copy_it"), print],
        structs: Vec::new(),
        enums: Vec::new(),
        elems: Vec::new(),
        print_fns: HashMap::new(),
        par_bodies: Vec::new(),
        globals: Vec::new(),
        init: None,
        main: None,
        files: Vec::new(),
        module_names: Vec::new(),
        tables: vec![MethodTable { type_id: 7, copy: FuncIdx(3), print: FuncIdx(4), methods: vec![FuncIdx(0)] }],
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
fn a_method_table_holds_the_type_id_and_the_functions_addresses() {
    let p = JitProgram::compile(&module(), Options::default()).expect("codegen");
    let lend: extern "C" fn(*mut i64, *mut [u64; 2]) = unsafe { std::mem::transmute(p.fn_ptr("lend").expect("lend")) };
    let mut x: i64 = 40;
    let mut value = [0u64; 2];
    lend(&mut x, &mut value);
    assert_eq!(value[0], &x as *const i64 as u64, "a borrowed value points at its place");
    let table = value[1] as *const u64;
    assert_eq!(table as usize % 8, 0);
    // SAFETY: the table has four 8-byte words: the type id, copy, print and one method.
    let words = unsafe { std::slice::from_raw_parts(table, 4) };
    let addr = |name: &str| p.fn_ptr(name).expect(name) as usize as u64;
    assert_eq!(words, [7, addr("copy_it"), addr("print_it"), addr("get")]);
    let call_get: extern "C" fn(*const [u64; 2], i64) -> i64 = unsafe { std::mem::transmute(p.fn_ptr("call_get").expect("call_get")) };
    assert_eq!(call_get(&value, 2), 42);
}
