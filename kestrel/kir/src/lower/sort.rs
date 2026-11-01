//! `xs.sort()` and `xs.sort_by(key)`: `Stmt::List(ListOp::Sort, ..)` with a generated compare
//! function for strings, or `Stmt::List(ListOp::SortBy, ..)` with a generated fill function for
//! numbers and bools.
//!
//! **The compare function** is `(env, a, b) -> u8`, called by the runtime with the addresses of two
//! items: 0 when `a` goes before `b`, 1 when they are equal, 2 when it goes after, strings by code
//! point. Both parameters are by pointer (`RefParam`), whatever the item.
//! - `sort()` on strings has one; `env` is unused.
//! - `sort_by(key)` with a string key has one per (item type, key), and its `env` is the address
//!   of the key's function value, so it calls `Call(code, [FnEnv(key), item])` for a literal's code
//!   or a named function's thunk, or calls the value indirectly. It computes both keys inside a
//!   scratch arena that is freed before it returns, since a key may build strings
//!   (`e.name.lower()`) and runs about twice per comparison.
//!
//! **The fill function.** `sort()` on numbers or bools, and `sort_by(key)` with a number or bool
//! key, are `ListOp::SortBy` instead: a generated `(env, xs, keys)` loops over the items of `xs`
//! in order and writes each one's key, widened to an `i64` (`false` 0, `true` 1) or an `f64`, into
//! `keys`, an array the runtime makes with one slot per item (see `kestrel_rt_sort_keyed_int`).
//! The runtime calls it once, when there are at least two items, then sorts by the keys: numbers
//! by value with a float NaN after every number and `-0.0` equal to `0.0`, `false` before `true`.
//! For `sort()` the key is the item itself. For `sort_by` the loop calls the key as the compare
//! function does, so `inline_small` can inline a literal's code into the loop, each call in a
//! scratch arena of its own.
//!
//! `strip_idle_scratch` (in `scratch`) takes the arenas out again when the literal's code, or a
//! named function's thunk (and what it calls), can't build anything.
//!
//! **The call site.** A literal key's `MakeFn` (with a stack environment) must be directly an
//! argument of a call, so `sort_by` calls a generated function `(xs, key)` that holds the
//! `ListOp::Sort` or `ListOp::SortBy`; `key` is passed on to the runtime by address. The key is `pure` (the checker
//! sees to that), so none of this calls into a built-in function that stores a line.

use kestrel_check::{ListMethod, Tables, Ty};
use kestrel_syntax::ast;

use super::subst::known_ksig;
use super::helpers::{elem_of, expr, with, Helpers};
use super::list_fns::Callee;
use super::{kty, FnLowerer};
use crate::ir::*;

fn local(name: &str, ty: KTy, kind: LocalKind) -> Local {
    Local { name: name.into(), ty, kind }
}

fn load(p: &Place, ty: KTy) -> Expr {
    expr(ExprKind::Load(p.clone()), ty)
}

/// The statement that puts in `r` (a `u8` local) how string `a` stands to string `b`.
fn order(a: &Place, b: &Place, r: LocalIdx) -> Vec<Stmt> {
    vec![Stmt::Let(r, expr(ExprKind::Str(StrOp::Cmp, vec![load(a, KTy::Str), load(b, KTy::Str)]), KTy::U8))]
}

/// Whether items or keys of type `ty` sort by a fill function (`ListOp::SortBy`): numbers and
/// bools. Strings sort by a compare function.
fn sorts_by_keys(ty: KTy) -> bool {
    matches!(ty, KTy::Bool | KTy::U8 | KTy::I32 | KTy::I64 | KTy::F32 | KTy::F64)
}

/// `k`, a number or bool key, widened for the runtime: `false` is 0 and `true` 1; an integer or
/// float widens exactly.
fn widened(k: Expr) -> Expr {
    let (key, wide) = (k.ty, wide_key(k.ty));
    match key {
        KTy::Bool => expr(
            ExprKind::If {
                cond: Box::new(k),
                then_body: Vec::new(),
                then_val: Some(Box::new(expr(ExprKind::Int(1), KTy::I64))),
                else_body: Vec::new(),
                else_val: Some(Box::new(expr(ExprKind::Int(0), KTy::I64))),
            },
            KTy::I64,
        ),
        _ if key == wide => k,
        _ => expr(ExprKind::Cast(Box::new(k)), wide),
    }
}

impl Helpers {
    /// The compare function for `sort()` on strings (`item`).
    pub(crate) fn sort_items_fn(&mut self, item: KTy) -> FuncIdx {
        if let Some(&f) = self.sort_less.get(&(item, None)) {
            return f;
        }
        let idx = self.reserve();
        self.sort_less.insert((item, None), idx);
        let (a, b, r) = (Place::local(LocalIdx(1)), Place::local(LocalIdx(2)), LocalIdx(3));
        let locals = vec![
            local("env", KTy::I64, LocalKind::Value),
            local("a", item, LocalKind::RefParam),
            local("b", item, LocalKind::RefParam),
            local("r", KTy::U8, LocalKind::Value),
        ];
        let mut body = order(&a, &b, r);
        body.push(Stmt::Return(Some(load(&Place::local(r), KTy::U8))));
        self.compare_fn(idx, locals, body);
        idx
    }

    /// The compare function for `sort_by` on items of type `item`, with string keys given by
    /// `callee`.
    fn sort_by_fn(&mut self, item: KTy, key: KTy, callee: Callee, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.sort_less.get(&(item, Some(callee))) {
            return f;
        }
        let idx = self.reserve();
        self.sort_less.insert((item, Some(callee)), idx);
        // Locals: 0 the key function value, 1 and 2 the items, 3 and 4 their keys, 5 the answer.
        let (f, a, b) = (Place::local(LocalIdx(0)), Place::local(LocalIdx(1)), Place::local(LocalIdx(2)));
        let (ka, kb, r) = (LocalIdx(3), LocalIdx(4), LocalIdx(5));
        let key_kind = if key.is_aggregate() { LocalKind::Addressed } else { LocalKind::Value };
        let locals = vec![
            local("key", KTy::Fn, LocalKind::RefParam),
            local("a", item, LocalKind::RefParam),
            local("b", item, LocalKind::RefParam),
            local("ka", key, key_kind),
            local("kb", key, key_kind),
            local("r", KTy::U8, LocalKind::Value),
        ];
        let call = |item: &Place| key_call(callee, &f, load(item, item_ty(&locals, item)), key, t);
        let mut body = vec![Stmt::PushArena(None, 0), Stmt::Let(ka, call(&a)), Stmt::Let(kb, call(&b))];
        body.extend(order(&Place::local(ka), &Place::local(kb), r));
        body.push(Stmt::PopArena { free: true });
        body.push(Stmt::Return(Some(load(&Place::local(r), KTy::U8))));
        self.compare_fn(idx, locals, body);
        if let Callee::Code { code, .. } = callee {
            self.sort_idle.push((idx, code));
        }
        idx
    }

    /// The fill function for sorting a list or array of type `list_ty` by number or bool keys:
    /// `(env, xs, keys)` (see the module comment). With `callee`, each item's key is the one it
    /// gives (`sort_by`, keys of type `key`), each call in a scratch arena; without, the item
    /// itself (`sort()`). Fill functions are cached by list type, compare functions by item type.
    fn sort_fill_fn(&mut self, list_ty: KTy, callee: Option<(KTy, Callee)>, t: &Tables) -> FuncIdx {
        let cache = (list_ty, callee.map(|(_, c)| c));
        if let Some(&f) = self.sort_less.get(&cache) {
            return f;
        }
        let idx = self.reserve();
        self.sort_less.insert(cache, idx);
        let item = elem_of(list_ty, t);
        let key = callee.map_or(item, |(key, _)| key);
        // Locals: 0 the key function value (or the unused environment word), 1 the items, 2 the
        // keys, 3 the index, 4 the key and 5 the item.
        let (f, xs, keys, i, k) = (Place::local(LocalIdx(0)), Place::local(LocalIdx(1)), Place::local(LocalIdx(2)), LocalIdx(3), LocalIdx(4));
        let env = match callee {
            Some(_) => local("key", KTy::Fn, LocalKind::RefParam),
            None => local("env", KTy::I64, LocalKind::Value),
        };
        let mut locals = vec![
            env,
            local("xs", list_ty, LocalKind::RefParam),
            local("keys", wide_keys(key, t), LocalKind::RefParam),
            local("i", KTy::I64, LocalKind::Value),
        ];
        // The loop's bound is the length of `xs`, which nothing in it changes, and `keys` has a
        // slot for each item.
        let at = || Proj::Index { at: load(&Place::local(i), KTy::I64), check: IndexCheck::Known };
        let item_at = with(&xs, at());
        let mut per_item = Vec::new();
        let value = match callee {
            Some((key, callee)) => {
                // The item is bound to `a`, as the compare function's parameter is named, so the
                // report names what the key does to it the same way.
                let a = LocalIdx(5);
                locals.push(local("k", key, LocalKind::Value));
                locals.push(local("a", item, LocalKind::RefParam));
                per_item.push(Stmt::Bind(a, item_at));
                per_item.push(Stmt::PushArena(None, 0));
                per_item.push(Stmt::Let(k, key_call(callee, &f, load(&Place::local(a), item), key, t)));
                per_item.push(Stmt::PopArena { free: true });
                if let Callee::Code { code, .. } = callee {
                    self.sort_idle.push((idx, code));
                }
                widened(load(&Place::local(k), key))
            }
            None => widened(load(&item_at, item)),
        };
        per_item.push(Stmt::Assign(with(&keys, at()), value));
        let body = vec![Stmt::ForRange { var: i, start: expr(ExprKind::Int(0), KTy::I64), end: expr(ExprKind::Len(xs), KTy::I64), body: per_item }];
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_sort_key_{}", idx.0),
            params: vec![LocalIdx(0), LocalIdx(1), LocalIdx(2)],
            ret: None,
            locals,
            body,
            is_pure: true,
            io: false,
            mutates_params: true,
            exported: false,
            lines: (0, 0),
            native: None,
        };
        idx
    }

    /// Fills in the reserved compare function `idx`: `(env, a, b) -> u8`, the first three locals.
    fn compare_fn(&mut self, idx: FuncIdx, locals: Vec<Local>, body: Vec<Stmt>) {
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_compare_{}", idx.0),
            params: vec![LocalIdx(0), LocalIdx(1), LocalIdx(2)],
            ret: Some(KTy::U8),
            locals,
            body,
            is_pure: true,
            io: false,
            mutates_params: false,
            exported: false,
            lines: (0, 0),
            native: None,
        };
    }

    /// The function `sort_by(var xs, key)` calls for a list or array of type `list_ty`: it sorts
    /// `xs` with the compare function (a string key) or the fill function (any other) for `callee`.
    fn sort_by_call_fn(&mut self, list_ty: KTy, key: KTy, callee: Callee, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.sort_calls.get(&(list_ty, callee)) {
            return f;
        }
        let idx = self.reserve();
        self.sort_calls.insert((list_ty, callee), idx);
        let item = elem_of(list_ty, t);
        let (op, f) = if key == KTy::Str {
            (ListOp::Sort, self.sort_by_fn(item, key, callee, t))
        } else {
            let kind = if key.is_float() { SortKey::Float } else { SortKey::Int };
            (ListOp::SortBy(kind), self.sort_fill_fn(list_ty, Some((key, callee)), t))
        };
        let xs = Place::local(LocalIdx(0));
        let sort = Stmt::List(op, xs, vec![expr(ExprKind::FnRef(f), KTy::Fn), load(&Place::local(LocalIdx(1)), KTy::Fn)]);
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_sort_by_{}", idx.0),
            params: vec![LocalIdx(0), LocalIdx(1)],
            ret: None,
            locals: vec![local("xs", list_ty, LocalKind::RefParam), local("key", KTy::Fn, LocalKind::RefParam)],
            body: vec![sort],
            is_pure: true,
            io: false,
            mutates_params: true,
            exported: false,
            lines: (0, 0),
            native: None,
        };
        idx
    }
}

/// The type of the local `p` names (a plain local).
fn item_ty(locals: &[Local], p: &Place) -> KTy {
    locals[p.root.0 as usize].ty
}

/// The key of `item` (a value of the item type), a `key`, given by `callee` through the function
/// value at `f`: the literal's code or the thunk directly, any other value indirectly.
fn key_call(callee: Callee, f: &Place, item: Expr, key: KTy, t: &Tables) -> Expr {
    let arg = Arg::Value(item);
    match callee {
        Callee::Code { code, .. } => expr(ExprKind::Call(code, vec![Arg::Value(expr(ExprKind::FnEnv(f.clone()), KTy::I64)), arg]), key),
        Callee::Value { sig, .. } => expr(ExprKind::CallIndirect { callee: f.clone(), args: vec![arg], sig: known_ksig(t, sig) }, key),
    }
}

/// The type a number or bool key of type `key` is widened to for the runtime.
fn wide_key(key: KTy) -> KTy {
    if key.is_float() {
        KTy::F64
    } else {
        KTy::I64
    }
}

/// The array of widened keys of type `key` that a fill function writes: `[i64]` or `[f64]`, whose
/// element types the checker interns for every sort (see its `list_sort`).
fn wide_keys(key: KTy, t: &Tables) -> KTy {
    let elem = if key.is_float() { Ty::F64 } else { Ty::I64 };
    let id = t.elem_ids.get(&elem).expect("the checker interns the keys' element type for every sort");
    KTy::Array(ElemIdx(id.0))
}

impl FnLowerer<'_> {
    /// `xs.sort()` or `xs.sort_by(key)`, as a statement.
    pub(crate) fn sort_stmt(&mut self, method: ListMethod, receiver: &ast::Expr, args: &[ast::Arg], out: &mut Vec<Stmt>) {
        let list_ty = kty(self.ty(receiver.id));
        let list = self.place_or_temp(receiver, out, true);
        if method == ListMethod::Sort {
            let item = elem_of(list_ty, self.t);
            let (op, f) = if sorts_by_keys(item) {
                let kind = if item.is_float() { SortKey::Float } else { SortKey::Int };
                (ListOp::SortBy(kind), self.helpers.sort_fill_fn(list_ty, None, self.t))
            } else {
                (ListOp::Sort, self.helpers.sort_items_fn(item))
            };
            // (The reference carries the line of the call, for the report.)
            let f = Expr { kind: ExprKind::FnRef(f), ty: KTy::Fn, line: self.line(receiver.span) };
            out.push(Stmt::List(op, list, vec![f]));
            return;
        }
        let f = &args[0].value;
        let value = self.expr(f);
        let Ty::Fn(sig) = self.ty(f.id) else { unreachable!("the checker gives a function to `sort_by`") };
        let callee = match (&f.kind, &value.kind) {
            (ast::ExprKind::Closure(_), ExprKind::MakeFn { code, .. }) => Callee::Code { code: *code, pure: true },
            // A named function's thunk has the shape of a closure's code (it ignores the environment).
            (_, ExprKind::FnRef(thunk)) => Callee::Code { code: *thunk, pure: true },
            _ => Callee::Value { sig, pure: self.sig(sig).is_pure },
        };
        let key = kty(self.sig(sig).ret);
        let helper = self.helpers.sort_by_call_fn(list_ty, key, callee, self.t);
        // The literal is directly the call's argument (see `check_stack_envs`).
        out.push(Stmt::Call(helper, vec![Arg::Ref(list), Arg::Value(value)], self.line(receiver.span)));
    }
}
