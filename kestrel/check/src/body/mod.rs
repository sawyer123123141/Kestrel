mod args;
mod assume;
mod builtins;
mod call;
mod call_stores;
mod captures;
mod closure;
mod closure_result;
mod collections;
mod consts;
mod default_cycles;
mod enums;
mod exclusive;
mod expr;
mod fnvalue;
mod from_bytes;
mod generic_call;
mod generic_variants;
mod hex;
mod iface;
mod let_else;
mod in_block;
mod infer;
mod inferred;
mod instance_calls;
mod lanes;
mod list_fns;
mod lists;
mod loop_vars;
mod maps;
mod match_guard;
mod matching;
mod moves;
mod narrow;
mod nested;
mod par;
mod param_calls;
mod param_ops;
mod ops;
mod place;
mod pop_owner;
mod regions;
mod remove;
mod stmt;
mod stop;
mod strings;
mod structs;
mod var_clash;
mod vec_builtins;
mod vec_make;
mod vec_ops;
mod wrap;

pub(crate) use builtins::{checksum_root, snapshot_verb};
pub(crate) use consts::{check_consts, is_const_expr};
pub(crate) use stop::stop_in_constant;
pub(crate) use default_cycles::check_default_cycles;
pub(crate) use structs::check_field_defaults;

use std::collections::{HashMap, HashSet};

use kestrel_syntax::ast::{Binding, Expr, ExprId, Program, Stmt, TypeExpr};
use kestrel_syntax::{Diagnostic, Span};

use crate::collect_ty::{resolve_type, TypeScope};
use crate::generic::{GenericOwner, ParamId};
use crate::sig::sig_accepts;
use crate::tables::{FnId, LocalId, LocalInfo, Tables};
use crate::ty::Ty;

/// What the surrounding code does with a block's or expression's value.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Expect {
    Discard,
    Value(Option<Ty>),
    /// A closure body's tail whose result type is not known: its type is whatever the tail shows,
    /// and an `if` or `match` whose branches that finish all give nothing gives nothing.
    Infer,
}

pub(crate) struct BodyChecker<'a> {
    t: &'a mut Tables,
    diags: &'a mut Vec<Diagnostic>,
    /// `None` while checking a constant initializer.
    fn_id: Option<FnId>,
    scopes: Vec<HashMap<String, LocalId>>,
    /// One entry per enclosing loop: whether a `break` targets it.
    loop_breaks: Vec<bool>,
    ret: Ty,
    is_pure: bool,
    /// Allocation contexts, outermost (the function's own) first.
    ctxs: Vec<regions::Ctx>,
    /// Per local: the context and loop depth it was declared in.
    local_ctx: HashMap<LocalId, (usize, usize)>,
    /// Views, which read a place without copying it: `for ... in` elements, map loop values and
    /// `let ... else` unwraps (`match` bindings are tracked by `match_guards` instead).
    views: HashSet<LocalId>,
    /// Owning locals that may have been moved by now, following the code's paths (see `moves`).
    moved: moves::Moved,
    /// The loops being checked, outermost first: where each started, and its `break`s.
    loop_moves: Vec<moves::LoopMoves>,
    /// Moves already reported as moved inside a loop, so an enclosing loop doesn't repeat them.
    loop_move_errors: HashSet<Span>,
    /// `match` arms whose bindings are reading a place right now.
    match_guards: Vec<regions::MatchGuard>,
    /// `let ... else` views in scope: nothing may move what they read.
    let_guards: Vec<let_else::LetGuard>,
    /// Enclosing `par for` bodies, outermost first.
    par: Vec<par::ParBody>,
    /// Enclosing `for ... in` loops over lists and arrays that are places, outermost first.
    for_in_lists: Vec<lists::LoopList>,
    /// Enclosing `for ... in` loops over maps that are places, outermost first.
    for_in_maps: Vec<lists::LoopList>,
    /// Enclosing `for ... in` loops, for `remove`.
    removing: remove::Removing,
    /// The call statement being checked, whose result is thrown away.
    discarded: Option<ExprId>,
    /// The closure literals being checked, outermost first (see `closure`).
    frames: Vec<closure::ClosureFrame>,
    /// The name of the parameter a closure literal is being passed for, for its messages.
    arg_name: Option<String>,
    /// Set while a closure literal passed to `map` is checked: its result type comes from its body.
    infer_result: bool,
    /// The closure literals passed straight to `filter`, `map`, `any`, `all` or `sort_by`, whose
    /// loops can't store them, so rule 7b doesn't apply to them (see `captures`).
    unstored: HashSet<ExprId>,
    /// The placeholders written types may name here: a generic function's own, or those of the
    /// struct or enum a method (or a field default) belongs to. Empty in plain code.
    type_scope: Vec<ParamId>,
    /// The generic whose placeholders `type_scope` lists. The bare name of a generic struct or
    /// enum means itself with these placeholders (G2).
    own: Option<GenericOwner>,
    /// The expression being checked against a type, and what it goes into (see `narrow`).
    into_site: Option<(ExprId, narrow::Site)>,
    /// The locals the enclosing loops declared, and as what (see `loop_vars`).
    loop_vars: HashMap<LocalId, loop_vars::LoopVar>,
}

pub(crate) fn check_fn(program: &Program, fn_id: FnId, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let info = &t.fns[fn_id.0 as usize];
    let decl = info.decl(program);
    let (ret, is_pure, params) = (info.ret, info.is_pure, info.params.clone());
    let is_native = info.is_native;
    let own = info.generic;
    let mut bc = BodyChecker::new(t, diags, Some(fn_id), ret, is_pure);
    bc.set_scope(own);
    bc.scopes.push(HashMap::new());
    // A native has no body; its parameters are declared so calls and codegen see them, but
    // they are only labels, so their names aren't checked (see `link`).
    let declare = if is_native { BodyChecker::add_local } else { BodyChecker::declare };
    for (p, sig) in decl.params.iter().zip(&params) {
        declare(&mut bc, &p.bind, sig.ty, p.is_var, true, p.is_var);
    }
    if is_native {
        return;
    }
    let expect = if ret == Ty::Unit { Expect::Discard } else { Expect::Value(Some(ret)) };
    // The body's last expression is the function's result.
    if let Some(Stmt::Expr(tail)) = decl.body.stmts.last() {
        bc.set_into(Some((tail.id, narrow::Site::Return)));
    }
    let body = bc.check_block(&decl.body, expect);
    bc.set_into(None);
    if let (Some(Stmt::Expr(tail)), false) = (decl.body.stmts.last(), ret == Ty::Unit) {
        bc.consume(tail);
        bc.check_return_region(tail);
    }
    // An unresolved return type was already reported; don't pile on.
    if !matches!(ret, Ty::Unit | Ty::Error) && body == Ty::Unit {
        let msg = format!("`{}` must return {}, but its body can finish without a value", decl.name, bc.tn(ret));
        bc.diags.push(Diagnostic::error(decl.name_span, msg).with_help("end the body with a value or a `return`"));
    }
}

impl<'a> BodyChecker<'a> {
    fn new(t: &'a mut Tables, diags: &'a mut Vec<Diagnostic>, fn_id: Option<FnId>, ret: Ty, is_pure: bool) -> Self {
        let ctxs = vec![regions::Ctx::Function];
        let (local_ctx, views, moved) = (HashMap::new(), HashSet::new(), HashMap::new());
        let (scopes, loop_breaks, match_guards) = (Vec::new(), Vec::new(), Vec::new());
        let (loop_moves, loop_move_errors) = (Vec::new(), HashSet::new());
        let (par, let_guards, for_in_lists, for_in_maps) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        BodyChecker {
            t,
            diags,
            fn_id,
            scopes,
            loop_breaks,
            ret,
            is_pure,
            ctxs,
            local_ctx,
            views,
            moved,
            loop_moves,
            loop_move_errors,
            match_guards,
            let_guards,
            par,
            for_in_lists,
            for_in_maps,
            removing: remove::Removing::default(),
            discarded: None,
            frames: Vec::new(),
            arg_name: None,
            infer_result: false,
            unstored: HashSet::new(),
            type_scope: Vec::new(),
            own: None,
            into_site: None,
            loop_vars: HashMap::new(),
        }
    }

    /// Puts a generic owner's placeholders in scope: types written in this code may name them.
    pub(crate) fn set_scope(&mut self, own: Option<GenericOwner>) {
        self.type_scope = own.map(|o| self.t.owner_params(o).to_vec()).unwrap_or_default();
        self.own = own;
    }

    /// A type written in this code, with this code's placeholders in scope.
    pub(crate) fn resolve(&mut self, ty: &TypeExpr) -> Ty {
        // A generic function's bare name for itself doesn't exist; a type's own does (G2).
        let own = self.own.filter(|o| !matches!(o, GenericOwner::Fn(_)));
        let scope = TypeScope { params: &self.type_scope, own };
        resolve_type(ty, &scope, self.t, self.diags)
    }

    /// The placeholder called `name` in this code, if there is one.
    pub(crate) fn param_named(&self, name: &str) -> Option<ParamId> {
        self.type_scope.iter().copied().find(|&p| self.t.generic.param(p).name == name)
    }

    /// A type as it reads in messages.
    fn tn(&self, ty: Ty) -> String {
        self.t.ty_name(ty)
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(span, msg));
    }

    fn error_help(&mut self, span: Span, msg: impl Into<String>, help: impl Into<String>) {
        self.diags.push(Diagnostic::error(span, msg).with_help(help));
    }

    fn lookup_local(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// A new local, which can't take a built-in or reserved name or hide another local. (A
    /// clash with one of its module's items or imports is `link`'s to report: here every
    /// module's items are global, and another module's names don't clash.)
    fn declare(&mut self, bind: &Binding, ty: Ty, mutable: bool, is_param: bool, is_var_param: bool) -> LocalId {
        let name = &bind.name;
        if let Some(msg) = crate::collect::reserved_name_error(name, name) {
            self.error(bind.span, msg);
        }
        if self.lookup_local(name).is_some() {
            self.error_help(
                bind.span,
                format!("`{name}` is already defined; Kestrel doesn't allow shadowing"),
                "pick a different name",
            );
        }
        self.add_local(bind, ty, mutable, is_param, is_var_param)
    }

    /// `declare` without the checks on the name.
    fn add_local(&mut self, bind: &Binding, ty: Ty, mutable: bool, is_param: bool, is_var_param: bool) -> LocalId {
        let name = &bind.name;
        let id = LocalId(self.t.locals.len() as u32);
        // A local belongs to the innermost closure around it, or else to its function. (A field
        // default or a constant has no function: only closures' locals exist there.)
        let closure = self.frames.last().map(|f| f.id);
        self.t.locals.push(LocalInfo {
            name: name.clone(),
            ty,
            mutable,
            is_param,
            is_var_param,
            addressed: false,
            owner: self.fn_id,
            closure,
            span: bind.span,
            alias: None,
        });
        match self.frames.last_mut() {
            Some(frame) => frame.locals.push(id),
            None => {
                let owner = self.fn_id.expect("locals only exist inside functions and closures");
                self.t.fns[owner.0 as usize].locals.push(id);
            }
        }
        self.t.bind_local.insert(bind.id, id);
        self.local_ctx.insert(id, (self.ctxs.len() - 1, self.loop_breaks.len()));
        self.scopes.last_mut().expect("a scope is open").insert(name.clone(), id);
        id
    }

    /// Checks `e` against an expected type, recording a conversion or an error.
    pub(crate) fn check_value(&mut self, e: &Expr, want: Option<Ty>) -> Ty {
        let found = self.check_expr(e, want);
        self.finish_value(e, found, want)
    }

    /// The rest of `check_value`, once `e` has been checked and found to be `found`.
    pub(crate) fn finish_value(&mut self, e: &Expr, found: Ty, want: Option<Ty>) -> Ty {
        if found == Ty::Never {
            return self.reject_never(e, found);
        }
        match want {
            Some(w) => {
                self.coerce(e, found, w);
                w
            }
            None => found,
        }
    }

    /// Like `check_value`, for a block's last expression: an `if` whose
    /// branches all leave early is fine there (the block just diverges).
    pub(crate) fn check_tail(&mut self, e: &Expr, want: Option<Ty>) -> Ty {
        let found = self.check_expr(e, want);
        match want {
            Some(w) => {
                self.coerce(e, found, w);
                if found == Ty::Never {
                    Ty::Never
                } else {
                    w
                }
            }
            None => found,
        }
    }

    /// An `if` whose branches all leave early produces nothing to use here.
    pub(crate) fn reject_never(&mut self, e: &Expr, ty: Ty) -> Ty {
        if ty != Ty::Never {
            return ty;
        }
        let msg = self.never_message(e);
        self.error(e.span, msg);
        Ty::Error
    }

    pub(crate) fn coerce(&mut self, e: &Expr, from: Ty, to: Ty) {
        if from == to || matches!(from, Ty::Never | Ty::Error) || to == Ty::Error {
            return;
        }
        if from.coerces_to(to) {
            if !self.widen_hex(e, from, to) {
                self.t.coerce.insert(e.id, to);
            }
            return;
        }
        // A struct or enum goes where an interface it lists is expected (or that interface's optional).
        if self.coerce_to_iface(e, from, to) {
            return;
        }
        // A pure function value goes where a non-pure one of the same shape is expected; they
        // are the same value, so nothing converts.
        if let (Ty::Fn(got), Ty::Fn(want)) = (from, to) {
            if sig_accepts(self.t, want, got) {
                return;
            }
        }
        // A `T` is a `T?` that has a value.
        if let Ty::Optional(inner) = to {
            if from.coerces_to(self.t.elem(inner)) {
                self.t.coerce.insert(e.id, to);
                return;
            }
        }
        let msg = format!("expected {}, found {}", self.tn(to), self.tn(from));
        if from.is_numeric() && to.is_numeric() && !from.is_float() && !to.is_float() {
            let help = self.narrowing_help(e, from, to);
            self.error_help(e.span, msg, help);
        } else if from.is_numeric() && to.is_numeric() {
            self.error_help(e.span, msg, format!("convert it explicitly: `{}(...)`", to.name()));
        } else {
            self.error(e.span, msg);
        }
    }
}
