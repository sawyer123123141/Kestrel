//! The interface tables: an interface's id and what the checker knows about it (`Tables::ifaces`,
//! `Tables::iface_by_name`), and the instances of generic interfaces (decision 16).
//!
//! `Handler[Click]` is an `IfaceId` made the way `Stack[int]` is a `StructId`: interned once per
//! declaration and type list, with the declaration's abstract methods copied with the types
//! filled in. Each instance has its own abstract method `FnId`s, so everything keyed by a method
//! (contracts through an interface, method tables) tells `Handler[Click]` from `Handler[Key]`.

use kestrel_syntax::Span;

use crate::generic::{GenericOwner, Instance, ParamId, Pending, Subst, TyListId};
use crate::sig::{Sig, SigParam};
use crate::tables::{FnId, Tables};
use crate::ty::Ty;

/// An interface, an index into `Tables::ifaces`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IfaceId(pub u32);

#[derive(Clone, Debug)]
pub struct IfaceInfo {
    pub name: String,
    /// Index into `Program::items`.
    pub item: usize,
    pub span: Span,
    /// The methods as abstract `FnInfo`s, in declaration order: also their slot order in a
    /// method table. An instance's are its own, with its types.
    pub methods: Vec<FnId>,
    /// A generic declaration's placeholders (its methods use them); empty for everything else.
    pub params: Vec<ParamId>,
    /// For an instance (`Handler[Click]`): its declaration and type arguments.
    pub instance: Option<Instance<IfaceId>>,
    /// No placeholder anywhere: false for a generic declaration and for an instance whose
    /// arguments hold one (`Handler[T]` inside generic code).
    pub concrete: bool,
}

impl Tables {
    pub fn iface(&self, id: IfaceId) -> &IfaceInfo {
        &self.ifaces[id.0 as usize]
    }

    /// An interface's declaration and arguments: an instance's own, or a declaration's own
    /// placeholders.
    pub(crate) fn iface_parts(&self, id: IfaceId) -> (IfaceId, Vec<Ty>) {
        let info = self.iface(id);
        match info.instance {
            Some(i) => (i.def, self.ty_list(i.args).to_vec()),
            None => (id, info.params.iter().map(|&p| Ty::Param(p)).collect()),
        }
    }

    /// The instance of generic interface `def` for `args` (`def` itself for its own placeholders).
    pub fn intern_iface(&mut self, def: IfaceId, args: TyListId) -> IfaceId {
        let decl = self.iface(def);
        debug_assert!(!decl.params.is_empty() && decl.instance.is_none(), "only a generic declaration has instances");
        if self.own_args(&decl.params, args) {
            return def;
        }
        // Past the cap, the instance gets a short name and the checker reports it (P42).
        let big = self.too_big(GenericOwner::Iface(def), args);
        if let Some(&id) = self.generic.iface_instances.get(&(def, args)) {
            return id;
        }
        let decl = self.iface(def);
        let name = if big { format!("{}[...]", decl.name) } else { self.instance_name(&decl.name, self.ty_list(args)) };
        let info = IfaceInfo {
            name,
            item: decl.item,
            span: decl.span,
            methods: Vec::new(),
            params: Vec::new(),
            instance: Some(Instance { def, args }),
            concrete: self.ty_list(args).iter().all(|&a| self.is_concrete(a)),
        };
        let id = IfaceId(self.ifaces.len() as u32);
        self.ifaces.push(info);
        // In the map before its methods are made: a method may name the instance itself.
        self.generic.iface_instances.insert((def, args), id);
        if self.generic.collecting {
            self.generic.pending.push(Pending::Iface(id));
        } else {
            self.fill_iface(id);
        }
        id
    }

    /// Makes instance `id`'s abstract methods from its declaration's: the same declaration and
    /// span, each type substituted (`Error` for a declaration that would need endless copies),
    /// named after the instance (`Handler[Click].handle`).
    pub(crate) fn fill_iface(&mut self, id: IfaceId) {
        let inst = self.iface(id).instance.expect("an instance");
        let owner = GenericOwner::Iface(inst.def);
        let endless = self.generic.endless.contains(&owner);
        let s = Subst { owner, args: inst.args };
        let mut methods = Vec::new();
        for m in self.iface(inst.def).methods.clone() {
            let mut info = self.func(m).clone();
            for p in &mut info.params {
                p.ty = if endless { Ty::Error } else { self.subst(p.ty, s) };
            }
            info.ret = if endless { Ty::Error } else { self.subst(info.ret, s) };
            let params = info.params.iter().map(|p| SigParam { ty: p.ty, is_var: p.is_var }).collect();
            info.sig = self.intern_sig(Sig { params, ret: info.ret, is_pure: info.is_pure });
            let short = info.name.rsplit('.').next().unwrap_or_default().to_string();
            info.name = format!("{}.{short}", self.iface(id).name);
            info.generic = None;
            methods.push(FnId(self.fns.len() as u32));
            self.fns.push(info);
        }
        self.ifaces[id.0 as usize].methods = methods;
    }
}
