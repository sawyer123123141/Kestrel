mod assign_into;
mod assign_site;
mod assume;
mod builtins;
mod bytes;
mod bytes_fingerprint;
mod bytes_read;
mod call;
mod checksum;
mod closure;
mod collections;
mod compare;
mod consts;
mod copies;
mod difference;
mod env;
mod eq;
mod expr;
mod fnvalue;
mod heap;
mod helpers;
mod helpers_print;
mod iface;
mod iface_copy;
mod layout;
mod lines;
mod list_fns;
mod list_helpers;
mod lists;
mod map_helpers;
mod maps;
mod matching;
mod par;
mod placeholders;
mod prove;
mod reload;
mod reload_hash;
mod reload_hash_stmt;
mod remove;
mod scopes;
mod scratch;
mod simd;
mod simd_builtins;
mod sort;
mod stmt;
mod stop;
mod strings;
mod stubs;
mod subst;
mod tables;
mod temps;
mod vec_builtins;
mod vec_make;
mod vector;

use std::collections::{HashMap, HashSet};

use kestrel_check::{ConstId, FnId, LocalId, Subst, Tables, Ty};
use kestrel_syntax::ast::{self, Program};
use kestrel_syntax::{LineIndex, Span};

use crate::ir::*;
use crate::native::native_symbol;
use helpers::Helpers;
pub use reload::RELOAD_DROP;

/// Lowers a checked program. `t` must come from a `check` with no errors.
pub fn lower(program: &Program, t: &Tables, lines: &LineIndex) -> Module {
    let (structs, enums) = layout::layouts(t);
    let mut helpers = Helpers::new(t.fns.len() as u32);
    helpers.proves = prove::Proves::new(program, t);
    let mut funcs: Vec<Func> = (0..t.fns.len()).map(|i| lower_fn(program, t, lines, FnId(i as u32), &mut helpers)).collect();
    let init = consts::lower_init(program, t, lines, &mut helpers);
    // A reloadable program's generated `main` and `reload_drop` (see `reload`).
    let reload_main = t.reload.as_ref().map(|shape| reload::generate(shape, t, &mut helpers));
    funcs.append(&mut helpers.funcs);
    scratch::strip_idle_scratch(&mut funcs, &helpers.sort_idle);
    let elems = placeholders::elems(t);
    let main = t.main.map(|m| FuncIdx(m.0)).or(reload_main);
    if let (Some(init), Some(main)) = (init, main) {
        funcs[main.0 as usize].body.insert(0, Stmt::Call(init, Vec::new(), 0));
    }
    let globals = helpers.consts.into_globals();
    let tables = helpers.ifaces.tables;
    let assumptions = helpers.assumptions;
    let contracts = helpers.proves.sites;
    let (copies, uncopied) = (copies::notes(t, lines), copies::uncopied(program, t, lines));
    let mut m = Module {
        funcs,
        structs,
        enums,
        elems,
        print_fns: helpers.print,
        par_bodies: helpers.par_bodies,
        globals,
        init,
        main,
        files: Vec::new(),
        module_names: Vec::new(),
        tables,
        not_inlined: Vec::new(),
        assumptions,
        contracts,
        copies,
        uncopied,
        hoisted: HashMap::new(),
        analysed: None,
        reload: None,
    };
    m.reload = t.reload.as_ref().map(|shape| reload::info(shape, t, &m));
    // Stack environments live only as long as the call they are passed to.
    if cfg!(debug_assertions) {
        if let Err(e) = crate::check_stack_envs(&m) {
            panic!("{e}");
        }
        prove::check_homes(&m);
    }
    placeholders::check_concrete(&m, t);
    m
}

/// The function for `FnInfo` `f`: a stub for an interface's abstract method or a generic
/// original, else its body lowered (a copy's from its generic's declaration, under the copy's
/// substitution).
fn lower_fn(program: &Program, t: &Tables, lines: &LineIndex, f: FnId, helpers: &mut Helpers) -> Func {
    let info = t.func(f);
    if info.is_abstract {
        return stubs::abstract_stub(info);
    }
    let subst = match (info.generic, info.copy) {
        (Some(_), None) => return stubs::generic_stub(info),
        (Some(owner), Some(copy)) => Some(Subst { owner, args: copy.args }),
        (None, None) => None,
        (None, Some(_)) => unreachable!("a copy keeps its generic's owner"),
    };
    let mut cx = FnLowerer::new(program, t, lines, Some(f), helpers);
    cx.subst = subst;
    cx.lower()
}

pub(crate) fn kty(ty: Ty) -> KTy {
    match ty {
        Ty::Bool => KTy::Bool,
        Ty::U8 => KTy::U8,
        Ty::Int => KTy::I32,
        Ty::I64 => KTy::I64,
        Ty::Float => KTy::F32,
        Ty::F64 => KTy::F64,
        Ty::Str => KTy::Str,
        Ty::Vec4 => KTy::F32X4,
        Ty::IVec4 => KTy::I32X4,
        Ty::Struct(s) => KTy::Struct(StructIdx(s.0)),
        Ty::Enum(e) => KTy::Enum(EnumIdx(e.0)),
        Ty::Array(e) => KTy::Array(ElemIdx(e.0)),
        Ty::List(e) => KTy::List(ElemIdx(e.0)),
        Ty::Arena => KTy::Arena,
        Ty::Gpu => KTy::Gpu,
        Ty::Optional(e) => KTy::Optional(ElemIdx(e.0)),
        Ty::Map(k, v) => KTy::Map(ElemIdx(k.0), ElemIdx(v.0)),
        Ty::Fn(_) => KTy::Fn,
        Ty::Iface(_) => KTy::Iface,
        // Written by hand: the catch-all below would take it with another message.
        Ty::Param(_) => panic!("a generic type reached KIR unsubstituted; this is a compiler bug"),
        other => panic!("type {other:?} has no runtime representation"),
    }
}

pub(crate) struct FnLowerer<'a> {
    program: &'a Program,
    t: &'a Tables,
    lines: &'a LineIndex,
    /// The function being lowered; `None` for the initializer of the constants and for a closure
    /// written in a field default or a constant.
    fn_id: Option<FnId>,
    /// The code being lowered is a built-in module's (see `lines`).
    in_builtin: bool,
    local_map: HashMap<LocalId, LocalIdx>,
    /// Grows during lowering as temporaries are added.
    locals: Vec<Local>,
    /// `match` bindings: the matched place and the variant whose fields they name.
    bindings: HashMap<LocalId, (Place, u32)>,
    /// `for ... in` bindings: the element place they stand for.
    elems: HashMap<LocalId, Place>,
    /// Element pointers made by `for ... in`: the place each points at.
    origins: HashMap<LocalIdx, Place>,
    /// Enclosing blocks, innermost last, with what leaving each must do.
    scopes: Vec<scopes::Scope>,
    /// Temporaries holding an owning result that is handed on as a new value: a native called in
    /// a scratch arena (see `scratch_call_value`), or a `pop` (see `temps`).
    scratch_results: HashSet<LocalIdx>,
    /// The reference local of each heap constant this function uses (see `consts`).
    const_locals: HashMap<ConstId, LocalIdx>,
    /// Enclosing loops that remove their item, innermost last (see `remove`).
    removing: Vec<remove::RemoveLoop>,
    /// When the code is a closure's: the closure, and the local bound to each of its captures.
    captured: Option<env::Captured>,
    /// A copy's substitution: its generic's placeholders and the copy's types for them.
    subst: Option<Subst>,
    /// The substitutions of the field defaults being lowered, innermost last (`None` for a plain
    /// struct's, whose types hold no placeholders); the active one is the top, else `subst` (see
    /// `subst.rs`).
    default_substs: Vec<Option<Subst>>,
    helpers: &'a mut Helpers,
}

impl<'a> FnLowerer<'a> {
    fn new(program: &'a Program, t: &'a Tables, lines: &'a LineIndex, fn_id: Option<FnId>, helpers: &'a mut Helpers) -> Self {
        let in_builtin = fn_id.is_some_and(|f| t.in_builtin(t.func(f).decl(program).name_span));
        let (local_map, locals, bindings, elems) = (HashMap::new(), Vec::new(), HashMap::new(), HashMap::new());
        let (origins, scopes) = (HashMap::new(), Vec::new());
        let (scratch_results, const_locals) = (HashSet::new(), HashMap::new());
        let removing = Vec::new();
        FnLowerer {
            program,
            t,
            lines,
            fn_id,
            in_builtin,
            local_map,
            locals,
            bindings,
            elems,
            origins,
            scopes,
            scratch_results,
            const_locals,
            removing,
            captured: None,
            subst: None,
            default_substs: Vec::new(),
            helpers,
        }
    }

    /// A function's (or a copy's) body. A copy's declaration is its generic's.
    fn lower(mut self) -> Func {
        // Copy the shared references out so borrows of them don't hold `self`.
        let (t, program) = (self.t, self.program);
        let info = t.func(self.fn_id.expect("a function is lowered with its id"));
        let decl = info.decl(program);
        self.declare_locals(&info.locals);
        let params = (0..info.params.len()).map(|i| LocalIdx(i as u32)).collect();
        let ret = self.sub(info.ret);
        let ret = if ret == Ty::Unit { None } else { Some(kty(ret)) };
        let body = if info.is_native { Vec::new() } else { self.lower_body(&decl.body, ret) };
        let lines = self.lines_of(decl.span);
        Func {
            name: info.name.clone(),
            params,
            ret,
            locals: self.locals,
            body,
            is_pure: info.is_pure,
            io: info.io,
            mutates_params: info.mutates_params,
            // Items of other modules are named `module.name`. A copy is never exported.
            exported: decl.is_pub && info.method.is_none() && !info.name.contains('.') && !info.is_native && info.copy.is_none(),
            lines,
            native: info.is_native.then(|| native_symbol(&info.name)),
        }
    }

    /// The packed lines of the first and last line of `span` (a declaration, or a closure's body).
    pub(crate) fn lines_of(&self, span: Span) -> (u32, u32) {
        (self.line(span), self.line(Span { start: span.end.saturating_sub(1).max(span.start), end: span.end }))
    }

    /// Gives each of the checker's locals `ids` (parameters first) a KIR local.
    pub(crate) fn declare_locals(&mut self, ids: &[LocalId]) {
        for &lid in ids {
            let l = self.local(lid);
            let ty = self.local_ty(lid);
            // `match` bindings aren't storage; they name fields of the matched place.
            if let Ty::Variant(..) = ty {
                continue;
            }
            let ty = kty(ty);
            let kind = if l.is_param && (l.is_var_param || ty.is_aggregate()) {
                LocalKind::RefParam
            } else if l.addressed || ty.is_aggregate() {
                LocalKind::Addressed
            } else {
                LocalKind::Value
            };
            self.local_map.insert(lid, LocalIdx(self.locals.len() as u32));
            self.locals.push(Local { name: l.name.clone(), ty, kind });
        }
    }

    /// The statements of a function's or closure's body, which gives a value of type `ret`
    /// (if any) and starts by binding the heap constants it uses.
    pub(crate) fn lower_body(&mut self, block: &ast::Block, ret: Option<KTy>) -> Vec<Stmt> {
        let mut body = if ret.is_some() {
            let (mut stmts, tail) = self.value_block(block, assume::Ending::Body);
            if let Some(v) = tail {
                let v = self.owned(v, true);
                stmts.push(Stmt::Return(Some(v)));
            }
            stmts
        } else {
            self.block_stmts(block)
        };
        body.splice(0..0, self.const_binds());
        body
    }

    /// A fresh local holding an intermediate value.
    pub(crate) fn temp(&mut self, ty: KTy) -> LocalIdx {
        let idx = LocalIdx(self.locals.len() as u32);
        let kind = if ty.is_aggregate() { LocalKind::Addressed } else { LocalKind::Value };
        self.locals.push(Local { name: format!("tmp{}", idx.0), ty, kind });
        idx
    }

    pub(crate) fn line(&self, span: Span) -> u32 {
        crate::ir::pack_line(self.lines.file(span.start), self.lines.line(span.start))
    }

    fn bind_local(&self, bind: &ast::Binding) -> LocalIdx {
        self.local_map[&self.t.bind_local[&bind.id]]
    }

    /// Whether `e` as a block's last expression can never yield a value: it
    /// is typed `Never`, or an `if` whose branches all end that way.
    pub(crate) fn leaves_early(&self, e: &ast::Expr) -> bool {
        if self.ty(e.id) == Ty::Never {
            return true;
        }
        match &e.kind {
            ast::ExprKind::If { then_block, else_block: Some(else_block), .. } => {
                [then_block, else_block].into_iter().all(|b| match b.stmts.last() {
                    Some(ast::Stmt::Expr(tail)) => self.leaves_early(tail),
                    _ => true,
                })
            }
            _ => false,
        }
    }
}

/// A shift's amount, converted to the shifted value's type (a vector's lane type) if it differs.
pub(crate) fn shift_amount(op: BinOp, amount: Expr, ty: KTy) -> Expr {
    let ty = ty.lane();
    if !matches!(op, BinOp::Shl | BinOp::Shr) || amount.ty == ty {
        return amount;
    }
    let line = amount.line;
    Expr { kind: ExprKind::Cast(Box::new(amount)), ty, line }
}

pub(crate) fn binop(op: ast::BinaryOp) -> BinOp {
    match op {
        ast::BinaryOp::Add => BinOp::Add,
        ast::BinaryOp::Sub => BinOp::Sub,
        ast::BinaryOp::Mul => BinOp::Mul,
        ast::BinaryOp::Div => BinOp::Div,
        ast::BinaryOp::Rem => BinOp::Rem,
        ast::BinaryOp::Eq => BinOp::Eq,
        ast::BinaryOp::Ne => BinOp::Ne,
        ast::BinaryOp::Lt => BinOp::Lt,
        ast::BinaryOp::Gt => BinOp::Gt,
        ast::BinaryOp::Le => BinOp::Le,
        ast::BinaryOp::Ge => BinOp::Ge,
        ast::BinaryOp::BitAnd => BinOp::BitAnd,
        ast::BinaryOp::BitOr => BinOp::BitOr,
        ast::BinaryOp::BitXor => BinOp::BitXor,
        ast::BinaryOp::Shl => BinOp::Shl,
        ast::BinaryOp::Shr => BinOp::Shr,
        ast::BinaryOp::AddWrap => BinOp::AddWrap,
        ast::BinaryOp::SubWrap => BinOp::SubWrap,
        ast::BinaryOp::MulWrap => BinOp::MulWrap,
        ast::BinaryOp::And | ast::BinaryOp::Or => unreachable!("lowered to And/Or nodes"),
    }
}
