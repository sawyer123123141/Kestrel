//! The one way lowering reads the checker's types and resolutions (P19). A copy of a generic is
//! lowered from the generic's declaration, so every type the tables recorded for its expressions,
//! locals, signatures and closures is in the generic's terms (`list[T]`, `Stack[T]`); each read
//! here applies the active substitution: the innermost field default's (`with_subst`), else the
//! copy's. Substitution only looks types up (`subst_known`, G13): the collection pass interned
//! every type a copy reads, and a miss panics naming the copy. With no active substitution
//! (plain code), each accessor is a straight read, so programs without generics lower as before.
//!
//! `kir/tests/subst_discipline.rs` keeps the direct reads (`t.ty(`, `t.res`, `t.coerce`,
//! `iface_borrows`, `t.sig(`, `t.local(`, `t.closures[`) out of every other file of `lower/`.

use kestrel_check::{ClosureInfo, FnId, GenericOwner, LocalId, LocalInfo, Res, Sig, SigId, Subst, Tables, Ty, TyListId};
use kestrel_syntax::ast::ExprId;

use super::{kty, FnLowerer};
use crate::ir::KSig;

/// A type the collection pass should have interned (or a copy it should have made) is missing.
fn missed(what: &str, copy: &str) -> ! {
    panic!("the collection pass missed `{what}` in `{copy}`; this is a compiler bug")
}

/// The signature of a call through a value of the checker's function type `sig`, which is
/// already in the terms of the code being lowered (substituted, or from plain code).
pub(super) fn known_ksig(t: &Tables, sig: SigId) -> KSig {
    let sig = t.sig(sig);
    let params = sig
        .params
        .iter()
        .map(|p| {
            let ty = kty(p.ty);
            (ty, p.is_var || ty.is_aggregate())
        })
        .collect();
    KSig { params, ret: (sig.ret != Ty::Unit).then(|| kty(sig.ret)), pure: sig.is_pure }
}

/// What a constant expression names, for code outside any function (the constants' build
/// order): constants are never generic, so there is nothing to substitute.
pub(super) fn plain_res(t: &Tables, e: ExprId) -> Option<Res> {
    t.res.get(&e).copied()
}

impl<'a> FnLowerer<'a> {
    /// The substitution the code being lowered is under: the innermost field default's, else the
    /// copy's; `None` in plain code.
    pub(crate) fn active_subst(&self) -> Option<Subst> {
        match self.default_substs.last() {
            Some(&default) => default,
            None => self.subst,
        }
    }

    /// The instance whose field default is being lowered (`Stack[int]`), when the active
    /// substitution is one's: in the default itself, or in a closure lowered for it.
    pub(crate) fn subst_instance(&self) -> Option<&'a str> {
        let t = self.t;
        let Some(Subst { owner: GenericOwner::Struct(def), args }) = self.active_subst() else { return None };
        t.generic.struct_instances.get(&(def, args)).map(|&inst| t.strukt(inst).name.as_str())
    }

    /// The name a missed type is reported in: the instance whose field default is being lowered
    /// (`Stack[int]`), else the function being lowered.
    fn copy_name(&self) -> &'a str {
        let in_default = self.default_substs.last().is_some_and(|s| s.is_some()) || self.fn_id.is_none();
        match (in_default.then(|| self.subst_instance()).flatten(), self.fn_id) {
            (Some(inst), _) => inst,
            (None, Some(f)) => &self.t.func(f).name,
            (None, None) => "a field default",
        }
    }

    /// Type list `l` (a generic callee's `type_args`), in the terms of the code being lowered.
    fn sub_list(&self, l: TyListId) -> TyListId {
        if self.active_subst().is_none() {
            return l;
        }
        let tys: Vec<Ty> = self.t.ty_list(l).iter().map(|&ty| self.sub(ty)).collect();
        match self.t.generic.ty_list_ids.get(&tys) {
            Some(&l) => l,
            None => {
                let shown: Vec<String> = tys.iter().map(|&ty| self.t.ty_name(ty)).collect();
                missed(&format!("[{}]", shown.join(", ")), self.copy_name())
            }
        }
    }

    /// `r`, the checker's resolution of `e`, in the terms of the code being lowered: a generic
    /// callee becomes its copy for `e`'s types, a construction or variant names its instance, and
    /// a call on a `T` (`ParamCall`) becomes a direct call of the concrete type's method (or its
    /// copy) or, when `T` is an interface type, a call through the interface (R7: then `T` has
    /// exactly that one interface rule).
    fn res_in_terms(&self, e: ExprId, r: Res) -> Res {
        let t = self.t;
        match r {
            Res::Fn(f) => Res::Fn(self.callee(e, f)),
            Res::FnValue(f) => Res::FnValue(self.callee(e, f)),
            Res::Construct(sid) => match self.sub(Ty::Struct(sid)) {
                Ty::Struct(sid) => Res::Construct(sid),
                other => unreachable!("a struct substitutes to a struct, not {other:?}"),
            },
            Res::Variant(eid, v) => match self.sub(Ty::Enum(eid)) {
                Ty::Enum(eid) => Res::Variant(eid, v),
                other => unreachable!("an enum substitutes to an enum, not {other:?}"),
            },
            Res::CallValue(sig) => match self.sub(Ty::Fn(sig)) {
                Ty::Fn(sig) => Res::CallValue(sig),
                other => unreachable!("a function type substitutes to a function type, not {other:?}"),
            },
            Res::Convert(ty) => Res::Convert(self.sub(ty)),
            // A call through `Handler[T]` in a copy goes through its instance (`Handler[int]`).
            Res::IfaceCall { iface, slot } => match self.sub(Ty::Iface(iface)) {
                Ty::Iface(iface) => Res::IfaceCall { iface, slot },
                other => unreachable!("an interface substitutes to an interface, not {other:?}"),
            },
            Res::SimdMake(ty) => Res::SimdMake(self.sub(ty)),
            Res::ParamCall { param, iface, slot } => {
                let method = t.iface(iface).methods[slot as usize];
                let name = t.func(method).name.rsplit('.').next().unwrap_or_default();
                let on = self.sub(Ty::Param(param));
                let (methods, args) = match on {
                    Ty::Struct(s) => (&t.strukt(s).methods, t.strukt(s).instance.map(|i| i.args)),
                    Ty::Enum(en) => (&t.enm(en).methods, t.enm(en).instance.map(|i| i.args)),
                    Ty::Iface(i) => {
                        let slot = t.iface(i).methods.iter().position(|&m| t.func(m).name.rsplit('.').next() == Some(name));
                        let slot = slot.unwrap_or_else(|| panic!("`{}` has no method `{name}`; this is a compiler bug", t.iface(i).name));
                        return Res::IfaceCall { iface: i, slot: slot as u32 };
                    }
                    other => panic!("a rule call on `{}` reached KIR; this is a compiler bug", t.ty_name(other)),
                };
                let m = *methods.get(name).unwrap_or_else(|| panic!("conformance guarantees a method `{name}`"));
                Res::Fn(match args {
                    Some(args) => self.copy_of(m, args),
                    None => m,
                })
            }
            other => other,
        }
    }

    /// The function call (or function value) `e` reaches when it names `f`: `f` itself, or, for
    /// a generic original, its copy for `e`'s types.
    fn callee(&self, e: ExprId, f: FnId) -> FnId {
        let info = self.t.func(f);
        if info.generic.is_none() || info.copy.is_some() {
            return f;
        }
        let Some(&l) = self.t.generic.type_args.get(&e) else {
            panic!("a call of the generic `{}` has no types; this is a compiler bug", info.name)
        };
        self.copy_of(f, self.sub_list(l))
    }

    /// The copy of generic `f` for (already substituted) `args`, which the collection pass made.
    fn copy_of(&self, f: FnId, args: TyListId) -> FnId {
        match self.t.generic.fn_copies.get(&(f, args)) {
            Some(&c) => c,
            None => {
                let shown: Vec<String> = self.t.ty_list(args).iter().map(|&ty| self.t.ty_name(ty)).collect();
                missed(&format!("{}[{}]", self.t.func(f).name, shown.join(", ")), self.copy_name())
            }
        }
    }

    /// `ty`, read from the tables, in the terms of the code being lowered.
    pub(crate) fn sub(&self, ty: Ty) -> Ty {
        match self.active_subst() {
            None => ty,
            Some(s) => self.t.subst_known(ty, s, self.copy_name()),
        }
    }

    /// The type of expression `e`.
    pub(crate) fn ty(&self, e: ExprId) -> Ty {
        self.sub(self.t.ty(e))
    }

    /// What the checker knows about local `l`: its name and flags. Its type is `local_ty`.
    pub(crate) fn local(&self, l: LocalId) -> &'a LocalInfo {
        self.t.local(l)
    }

    /// The type of local `l`.
    pub(crate) fn local_ty(&self, l: LocalId) -> Ty {
        self.sub(self.t.local(l).ty)
    }

    /// Signature `s` in the terms of the code being lowered.
    pub(crate) fn sig(&self, s: SigId) -> &'a Sig {
        match self.sub(Ty::Fn(s)) {
            Ty::Fn(s) => self.t.sig(s),
            other => unreachable!("a function type substitutes to a function type, not {other:?}"),
        }
    }

    /// The signature of a call through a value of function type `s`.
    pub(crate) fn ksig(&self, s: SigId) -> KSig {
        match self.sub(Ty::Fn(s)) {
            Ty::Fn(s) => known_ksig(self.t, s),
            other => unreachable!("a function type substitutes to a function type, not {other:?}"),
        }
    }

    /// The conversion the checker recorded for `e`, as a target type. In a copy, a conversion
    /// whose substituted source and target are the same type is no conversion (G14: a `T` passed
    /// as `Damageable` when `T` is `Damageable`).
    pub(crate) fn coerce(&self, e: ExprId) -> Option<Ty> {
        let to = *self.t.coerce.get(&e)?;
        if self.active_subst().is_none() {
            return Some(to);
        }
        let to = self.sub(to);
        (self.ty(e) != to).then_some(to)
    }

    /// Whether `e` is lent to an interface parameter (`Tables::iface_borrows`); dropped with its
    /// conversion when that is dropped (G14).
    pub(crate) fn lent(&self, e: ExprId) -> bool {
        self.t.iface_borrows.contains(&e) && (self.active_subst().is_none() || self.coerce(e).is_some())
    }

    /// What `e` resolves to, in the terms of the code being lowered (see `res_in_terms`).
    pub(crate) fn res(&self, e: ExprId) -> Option<Res> {
        let r = self.t.res.get(&e).copied()?;
        Some(self.res_in_terms(e, r))
    }

    /// What `e` resolves to, when the checker resolves every such expression.
    pub(crate) fn resolved(&self, e: ExprId) -> Res {
        self.res(e).expect("the checker resolves this expression")
    }

    /// Closure literal `e`'s names and flags. Its signature and capture types are in the
    /// literal's terms: read them through `sig` and `sub`.
    pub(crate) fn closure(&self, e: ExprId) -> &'a ClosureInfo {
        &self.t.closures[&e]
    }

    /// Runs `f` under substitution `s` (a field default lowered at a construction: its instance's,
    /// or `None` for a plain struct's, which means the same in every copy it is built in).
    pub(crate) fn with_subst<R>(&mut self, s: Option<Subst>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.default_substs.push(s);
        let out = f(self);
        self.default_substs.pop();
        out
    }
}
