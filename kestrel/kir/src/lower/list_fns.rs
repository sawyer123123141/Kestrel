//! The loops behind `xs.filter(f)`, `xs.map(f)`, `xs.any(f)` and `xs.all(f)`.
//!
//! Each is a generated function `(xs, f) -> result`, named for the method (`__kestrel_list_fn_filter_7`: the
//! report names the built-in by it), a `ForRange` over `xs` with unchecked
//! indexing (the loop's bound is the list's length, and `f` cannot change `xs`) that calls `f`
//! for each item, read-only, in order:
//! - `filter` copies each kept item into a new list (as an indexed read would); a scalar item is
//!   read once, into a local, for both the test and the copy;
//! - `map` adds each result, a fresh value, to a new list, which it gives room for every result
//!   before the loop (`ListOp::Reserve`), so the loop's `add` never grows it;
//! - `any` and `all` return at the first item that decides.
//!
//! **A literal gets a loop of its own** (one per literal's code): the loop calls the literal's
//! code directly, `Call(code, [FnEnv(f), item])`, so `inline_small` can inline the closure body
//! into the loop. The call site passes the literal itself as the loop's `f` argument, a
//! `MakeFn` with a stack environment that is directly the `Call`'s argument (see
//! `check_stack_envs`). **Any other function value** goes through a loop per function type, which
//! calls it indirectly. The result is built in the current arena, like any list.

use kestrel_check::{ListMethod, SigId, Tables, Ty};
use kestrel_syntax::ast;

use super::subst::known_ksig;
use super::helpers::{elem_of, expr, has_heap, with, Helpers};
use super::{kty, FnLowerer};
use crate::ir::*;

/// What a generated loop calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Callee {
    /// A closure literal's code, called with the environment word of the loop's function value.
    Code { code: FuncIdx, pure: bool },
    /// Any function value of this type (in the terms of the code calling the loop), called
    /// through the loop's function value; `pure` is the type's.
    Value { sig: SigId, pure: bool },
}

fn local(name: &str, ty: KTy, kind: LocalKind) -> Local {
    Local { name: name.into(), ty, kind }
}

impl Helpers {
    /// The loop for `method` over a list or array of type `list_ty`, calling `callee`. `ret` is
    /// the type of its result: the new list for `filter` and `map`, a bool otherwise.
    pub(crate) fn list_fn(&mut self, method: ListMethod, list_ty: KTy, callee: Callee, ret: KTy, t: &Tables) -> FuncIdx {
        let key = (method, list_ty, callee);
        if let Some(&f) = self.list_fns.get(&key) {
            return f;
        }
        let idx = self.reserve();
        self.list_fns.insert(key, idx);
        let elem = elem_of(list_ty, t);
        // (The expressions here carry no source line: nothing in the loop can fail by line. The
        // index is unchecked, and the function it calls reports its own lines. A checked
        // operation added here needs a line.)
        // Local 0 is the list and local 1 the function value (both passed by pointer), local 2
        // the index, and local 3 the new list when there is one.
        let (xs, f, i, out) = (Place::local(LocalIdx(0)), Place::local(LocalIdx(1)), LocalIdx(2), LocalIdx(3));
        let mut locals = vec![
            local("xs", list_ty, LocalKind::RefParam),
            local("f", KTy::Fn, LocalKind::RefParam),
            local("i", KTy::I64, LocalKind::Value),
        ];
        // The loop never changes the list, so `i < xs.len` holds throughout.
        let at = expr(ExprKind::Load(Place::local(i)), KTy::I64);
        let item_at = with(&xs, Proj::Index { at, check: IndexCheck::Known });
        let call_on = |item: Expr| match callee {
            Callee::Code { code, .. } => {
                let env = expr(ExprKind::FnEnv(f.clone()), KTy::I64);
                ExprKind::Call(code, vec![Arg::Value(env), Arg::Value(item)])
            }
            Callee::Value { sig, .. } => ExprKind::CallIndirect { callee: f.clone(), args: vec![Arg::Value(item)], sig: known_ksig(t, sig) },
        };
        let call = call_on(expr(ExprKind::Load(item_at.clone()), elem));
        let result_of = |ty: KTy| expr(call.clone(), ty);
        let truth = |b: bool| Stmt::Return(Some(expr(ExprKind::Bool(b), KTy::Bool)));
        let mut body = Vec::new();
        let per_item = match method {
            ListMethod::Filter => {
                locals.push(local("kept", ret, LocalKind::Addressed));
                body.push(Stmt::Let(out, expr(ExprKind::ListNew, ret)));
                if !elem.is_aggregate() && !has_heap(elem, t) {
                    // A scalar: read once, for the test and for `add`.
                    let item = LocalIdx(locals.len() as u32);
                    locals.push(local("item", elem, LocalKind::Value));
                    let now = || expr(ExprKind::Load(Place::local(item)), elem);
                    let keep = vec![Stmt::ListAdd(Place::local(out), now())];
                    vec![
                        Stmt::Let(item, expr(ExprKind::Load(item_at), elem)),
                        Stmt::If { cond: expr(call_on(now()), KTy::Bool), then_body: keep, else_body: Vec::new() },
                    ]
                } else {
                    let mut keep = Vec::new();
                    let value = if has_heap(elem, t) {
                        // The item is copied byte for byte, then its strings and lists get their own.
                        let copy = LocalIdx(locals.len() as u32);
                        locals.push(local("copy", elem, LocalKind::Addressed));
                        keep.push(Stmt::Let(copy, expr(ExprKind::Load(item_at.clone()), elem)));
                        keep.extend(self.fixup(Place::local(copy), elem, t));
                        expr(ExprKind::Load(Place::local(copy)), elem)
                    } else {
                        expr(ExprKind::Load(item_at), elem)
                    };
                    keep.push(Stmt::ListAdd(Place::local(out), value));
                    vec![Stmt::If { cond: result_of(KTy::Bool), then_body: keep, else_body: Vec::new() }]
                }
            }
            ListMethod::Map => {
                locals.push(local("mapped", ret, LocalKind::Addressed));
                body.push(Stmt::Let(out, expr(ExprKind::ListNew, ret)));
                // Room for every result first, so the loop's `add` never grows the list.
                body.push(Stmt::List(ListOp::Reserve, Place::local(out), vec![expr(ExprKind::Len(xs.clone()), KTy::I64)]));
                vec![Stmt::ListAdd(Place::local(out), result_of(elem_of(ret, t)))]
            }
            ListMethod::Any => vec![Stmt::If { cond: result_of(KTy::Bool), then_body: vec![truth(true)], else_body: Vec::new() }],
            ListMethod::All => {
                let no = expr(ExprKind::Unary(UnOp::Not, Box::new(result_of(KTy::Bool))), KTy::Bool);
                vec![Stmt::If { cond: no, then_body: vec![truth(false)], else_body: Vec::new() }]
            }
            other => unreachable!("`{other:?}` takes no function"),
        };
        body.push(Stmt::ForRange {
            var: i,
            start: expr(ExprKind::Int(0), KTy::I64),
            end: expr(ExprKind::Len(xs), KTy::I64),
            body: per_item,
        });
        body.push(match method {
            ListMethod::Filter | ListMethod::Map => Stmt::Return(Some(expr(ExprKind::Load(Place::local(out)), ret))),
            ListMethod::Any => truth(false),
            _ => truth(true),
        });
        let pure = match callee {
            Callee::Code { pure, .. } => pure,
            Callee::Value { pure, .. } => pure,
        };
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_list_fn_{}_{}", format!("{method:?}").to_lowercase(), idx.0),
            params: vec![LocalIdx(0), LocalIdx(1)],
            ret: Some(ret),
            locals,
            body,
            is_pure: pure,
            io: !pure,
            mutates_params: false,
            exported: false,
            lines: (0, 0),
            native: None,
        };
        idx
    }
}

impl FnLowerer<'_> {
    /// `xs.filter(f)`, `xs.map(f)`, `xs.any(f)` or `xs.all(f)`: a call of the generated loop with
    /// the list and the function value. A literal is passed as it is, with its stack environment
    /// (see the module comment).
    pub(crate) fn list_fn_call(&mut self, method: ListMethod, receiver: &ast::Expr, f: &ast::Expr, ty: KTy, line: u32) -> ExprKind {
        let mut pre = Vec::new();
        let list_ty = kty(self.ty(receiver.id));
        let list = self.place_or_temp(receiver, &mut pre, true);
        let value = self.expr(f);
        let callee = match (&f.kind, &value.kind) {
            (ast::ExprKind::Closure(_), ExprKind::MakeFn { code, .. }) => Callee::Code { code: *code, pure: self.closure(f.id).is_pure },
            _ => match self.ty(f.id) {
                Ty::Fn(sig) => Callee::Value { sig, pure: self.sig(sig).is_pure },
                other => unreachable!("the checker gives `{other:?}` to a function argument"),
            },
        };
        let helper = self.helpers.list_fn(method, list_ty, callee, ty, self.t);
        let args = vec![Arg::Value(Expr { kind: ExprKind::Load(list), ty: list_ty, line }), Arg::Value(value)];
        Self::with_pre(pre, ExprKind::Call(helper, args), ty, line)
    }
}

impl Helpers {
    /// Whether `f`, a generated function, is a list method's loop that calls a function that
    /// isn't `pure`: running it may call into a built-in module, and so store a line of its own
    /// (see `lines`).
    pub(crate) fn runs_user_code(&self, f: FuncIdx) -> bool {
        f.0 >= self.base && self.list_fns.values().any(|&g| g == f) && !self.funcs[(f.0 - self.base) as usize].is_pure
    }
}
