//! Substitution: a type in a generic's terms (`list[T]`, `Stack[T]`, the declaration `Stack`
//! itself) made into the type for given arguments (`list[int]`, `Stack[int]`).
//!
//! `subst` interns what it makes (the checker and the collection pass); `subst_known` only looks
//! types up and panics on a miss (lowering, G13: lowering never makes a checker type).

use crate::generic::{ParamId, Subst, TyListId};
use crate::sig::{Sig, SigId, SigParam};
use crate::tables::{EnumId, StructId, Tables};
use crate::ty::Ty;

const OTHER_OWNER: &str = "a placeholder of another generic reached a substitution; this is a compiler bug";

fn missed(what: String, copy: &str) -> ! {
    panic!("the collection pass missed `{what}` in `{copy}`; this is a compiler bug")
}

impl Tables {
    /// Whether `ty` holds no placeholder anywhere: false for a `Param`, for a struct or enum
    /// whose `concrete` is false (generic declarations included: `Stack` in its own methods is
    /// `Stack[T]`), and for any type containing one.
    pub fn is_concrete(&self, ty: Ty) -> bool {
        match ty {
            Ty::Param(_) => false,
            Ty::Struct(s) => self.strukt(s).concrete,
            Ty::Enum(e) | Ty::Variant(e, _) => self.enm(e).concrete,
            Ty::Array(e) | Ty::List(e) | Ty::Optional(e) => self.is_concrete(self.elem(e)),
            Ty::Map(k, v) => self.is_concrete(self.elem(k)) && self.is_concrete(self.elem(v)),
            Ty::Fn(sig) => {
                let sig = self.sig(sig);
                sig.params.iter().all(|p| self.is_concrete(p.ty)) && self.is_concrete(sig.ret)
            }
            Ty::Unit
            | Ty::Never
            | Ty::Bool
            | Ty::Int
            | Ty::I64
            | Ty::U8
            | Ty::Float
            | Ty::F64
            | Ty::Str
            | Ty::Vec4
            | Ty::IVec4
            | Ty::Arena
            | Ty::Gpu
            | Ty::Error => true,
            Ty::Iface(i) => self.iface(i).concrete,
        }
    }

    /// The type `p` stands for under `s`.
    fn param_arg(&self, p: ParamId, s: Subst) -> Ty {
        let info = self.generic.param(p);
        if info.owner != s.owner {
            panic!("{OTHER_OWNER}");
        }
        self.ty_list(s.args)[info.index as usize]
    }

    /// A struct's declaration and arguments: an instance's own, or a declaration's own
    /// placeholders (G2).
    pub(crate) fn struct_parts(&self, id: StructId) -> (StructId, Vec<Ty>) {
        let info = self.strukt(id);
        match info.instance {
            Some(i) => (i.def, self.ty_list(i.args).to_vec()),
            None => (id, info.params.iter().map(|&p| Ty::Param(p)).collect()),
        }
    }

    /// As `struct_parts`, for an enum.
    pub(crate) fn enum_parts(&self, id: EnumId) -> (EnumId, Vec<Ty>) {
        let info = self.enm(id);
        match info.instance {
            Some(i) => (i.def, self.ty_list(i.args).to_vec()),
            None => (id, info.params.iter().map(|&p| Ty::Param(p)).collect()),
        }
    }

    /// `ty` with `s`'s owner's placeholders replaced by `s`'s arguments, interning the element
    /// types, signatures, type lists and instances it makes.
    pub fn subst(&mut self, ty: Ty, s: Subst) -> Ty {
        if self.is_concrete(ty) {
            return ty;
        }
        match ty {
            Ty::Param(p) => self.param_arg(p, s),
            Ty::Array(e) => {
                let inner = self.subst(self.elem(e), s);
                Ty::Array(self.intern_elem(inner))
            }
            Ty::List(e) => {
                let inner = self.subst(self.elem(e), s);
                Ty::List(self.intern_elem(inner))
            }
            Ty::Optional(e) => {
                let inner = self.subst(self.elem(e), s);
                Ty::Optional(self.intern_elem(inner))
            }
            Ty::Map(k, v) => {
                let (k, v) = (self.subst(self.elem(k), s), self.subst(self.elem(v), s));
                Ty::Map(self.intern_elem(k), self.intern_elem(v))
            }
            Ty::Fn(sig) => Ty::Fn(self.subst_sig(sig, s)),
            Ty::Struct(id) => {
                let (def, args) = self.struct_parts(id);
                let args = self.subst_tys(args, s);
                Ty::Struct(self.intern_struct(def, args))
            }
            Ty::Enum(id) => Ty::Enum(self.subst_enum(id, s)),
            Ty::Variant(id, v) => Ty::Variant(self.subst_enum(id, s), v),
            Ty::Iface(id) => {
                let (def, args) = self.iface_parts(id);
                let args = self.subst_tys(args, s);
                Ty::Iface(self.intern_iface(def, args))
            }
            other => unreachable!("`{other:?}` is concrete"),
        }
    }

    fn subst_enum(&mut self, id: EnumId, s: Subst) -> EnumId {
        let (def, args) = self.enum_parts(id);
        let args = self.subst_tys(args, s);
        self.intern_enum(def, args)
    }

    fn subst_tys(&mut self, tys: Vec<Ty>, s: Subst) -> TyListId {
        let tys = tys.into_iter().map(|ty| self.subst(ty, s)).collect();
        self.intern_ty_list(tys)
    }

    /// A type list substituted, interned.
    pub fn subst_list(&mut self, l: TyListId, s: Subst) -> TyListId {
        let tys = self.ty_list(l).to_vec();
        self.subst_tys(tys, s)
    }

    /// A signature substituted, interned.
    pub fn subst_sig(&mut self, sig: SigId, s: Subst) -> SigId {
        let old = self.sig(sig).clone();
        let params = old.params.iter().map(|p| SigParam { ty: self.subst(p.ty, s), is_var: p.is_var }).collect();
        let ret = self.subst(old.ret, s);
        self.intern_sig(Sig { params, ret, is_pure: old.is_pure })
    }

    /// `subst` with lookups only: every type it would make must already be interned, else it
    /// panics naming the type and `copy`, the copy being lowered (G13).
    pub fn subst_known(&self, ty: Ty, s: Subst, copy: &str) -> Ty {
        if self.is_concrete(ty) {
            return ty;
        }
        let elem = |inner: Ty, shown: &dyn Fn(String) -> String| match self.elem_ids.get(&inner) {
            Some(&id) => id,
            None => missed(shown(self.ty_name(inner)), copy),
        };
        match ty {
            Ty::Param(p) => self.param_arg(p, s),
            Ty::Array(e) => Ty::Array(elem(self.subst_known(self.elem(e), s, copy), &|n| format!("[{n}]"))),
            Ty::List(e) => Ty::List(elem(self.subst_known(self.elem(e), s, copy), &|n| format!("list[{n}]"))),
            Ty::Optional(e) => Ty::Optional(elem(self.subst_known(self.elem(e), s, copy), &|n| format!("{n}?"))),
            Ty::Map(k, v) => {
                let (k, v) = (self.subst_known(self.elem(k), s, copy), self.subst_known(self.elem(v), s, copy));
                let shown = format!("map[{}, {}]", self.ty_name(k), self.ty_name(v));
                match (self.elem_ids.get(&k), self.elem_ids.get(&v)) {
                    (Some(&k), Some(&v)) => Ty::Map(k, v),
                    _ => missed(shown, copy),
                }
            }
            Ty::Fn(sig) => {
                let old = self.sig(sig);
                let params = old.params.iter().map(|p| SigParam { ty: self.subst_known(p.ty, s, copy), is_var: p.is_var }).collect();
                let new = Sig { params, ret: self.subst_known(old.ret, s, copy), is_pure: old.is_pure };
                match self.sig_ids.get(&new) {
                    Some(&id) => Ty::Fn(id),
                    None => missed(self.sig_text(&new), copy),
                }
            }
            Ty::Struct(id) => {
                let (def, args) = self.struct_parts(id);
                let args: Vec<Ty> = args.into_iter().map(|a| self.subst_known(a, s, copy)).collect();
                let found = self.generic.ty_list_ids.get(&args).and_then(|l| self.generic.struct_instances.get(&(def, *l)));
                match found {
                    Some(&inst) => Ty::Struct(inst),
                    None => missed(self.instance_name(&self.strukt(def).name, &args), copy),
                }
            }
            Ty::Enum(id) => Ty::Enum(self.enum_known(id, s, copy)),
            Ty::Variant(id, v) => Ty::Variant(self.enum_known(id, s, copy), v),
            Ty::Iface(id) => {
                let (def, args) = self.iface_parts(id);
                let args: Vec<Ty> = args.into_iter().map(|a| self.subst_known(a, s, copy)).collect();
                let found = self.generic.ty_list_ids.get(&args).and_then(|l| self.generic.iface_instances.get(&(def, *l)));
                match found {
                    Some(&inst) => Ty::Iface(inst),
                    None => missed(self.instance_name(&self.iface(def).name, &args), copy),
                }
            }
            other => unreachable!("`{other:?}` is concrete"),
        }
    }

    fn enum_known(&self, id: EnumId, s: Subst, copy: &str) -> EnumId {
        let (def, args) = self.enum_parts(id);
        let args: Vec<Ty> = args.into_iter().map(|a| self.subst_known(a, s, copy)).collect();
        let found = self.generic.ty_list_ids.get(&args).and_then(|l| self.generic.enum_instances.get(&(def, *l)));
        match found {
            Some(&inst) => inst,
            None => missed(self.instance_name(&self.enm(def).name, &args), copy),
        }
    }

    /// An instance's name: `Stack[int]`, `Pair[string, vec2]`.
    pub(crate) fn instance_name(&self, def: &str, args: &[Ty]) -> String {
        let args: Vec<String> = args.iter().map(|&a| self.ty_name(a)).collect();
        format!("{def}[{}]", args.join(", "))
    }
}
