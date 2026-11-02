//! Generics in the checker: placeholders (`Ty::Param`), interned type lists, instances of generic
//! structs and enums (`Stack[int]`), and substitution.
//!
//! A generic declaration is a `StructId` / `EnumId` whose `params` aren't empty; its fields use
//! `Ty::Param`, and as a type it means itself with its own placeholders (`Stack[T]`). Every other
//! use is an instance, interned once per (declaration, type list) with its fields filled in, so
//! every type question works on it unchanged (G12).

use std::collections::{HashMap, HashSet};

use kestrel_syntax::ast::ExprId;
use kestrel_syntax::Span;

use crate::tables::{EnumId, FnId, IfaceId, StructId, Tables};
use crate::ty::Ty;

pub(crate) mod copies;
mod copies_cap;
mod copies_checksum;
pub(crate) mod declare;
pub(crate) mod expand;
pub(crate) mod graph;
pub(crate) mod infer;
mod instances;
mod parts;
pub(crate) mod rules;
mod subst;

/// The most parts an interned type list may have (P41, P42): a type that doubles in width at each
/// step would otherwise build names of billions of characters.
pub(crate) const MAX_PARTS: u32 = 1_000;

/// A placeholder, an index into `Generics::params`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParamId(pub u32);

/// An interned list of types, an index into `Generics::ty_lists`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TyListId(pub u32);

/// The generic whose placeholders a type or function uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GenericOwner {
    Struct(StructId),
    Enum(EnumId),
    Fn(FnId),
    Iface(IfaceId),
}

/// One placeholder: its name, its owner and position there, and its rules.
#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub name: String,
    pub span: Span,
    pub owner: GenericOwner,
    pub index: u32,
    /// Its interface rules, in the order written, each once.
    pub ifaces: Vec<IfaceId>,
    /// Set by `equatable`, and by `ordered` and `key`, which include it (G4).
    pub equatable: bool,
    pub ordered: bool,
    pub key: bool,
}

/// The types for an owner's placeholders, by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Subst {
    pub owner: GenericOwner,
    pub args: TyListId,
}

/// What an instance is made of: its declaration and the types for its placeholders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Instance<I> {
    pub def: I,
    pub args: TyListId,
}

/// What a copy of a generic function or method was made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CopyInfo {
    pub generic: FnId,
    pub args: TyListId,
    pub made_at: Span,
    pub made_by: Option<FnId>,
}

/// An instance interned while `collect` resolves fields, waiting for `fill_pending`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Struct(StructId),
    Enum(EnumId),
    Iface(IfaceId),
}

/// The generic tables (`Tables::generic`).
#[derive(Debug, Default)]
pub struct Generics {
    pub params: Vec<ParamInfo>,
    pub ty_lists: Vec<Vec<Ty>>,
    pub ty_list_ids: HashMap<Vec<Ty>, TyListId>,
    pub struct_instances: HashMap<(StructId, TyListId), StructId>,
    pub enum_instances: HashMap<(EnumId, TyListId), EnumId>,
    pub iface_instances: HashMap<(IfaceId, TyListId), IfaceId>,
    pub fn_copies: HashMap<(FnId, TyListId), FnId>,
    /// For every call, method call, type call and function value whose callee is generic: the
    /// types for the callee owner's placeholders, in the caller's terms.
    pub type_args: HashMap<ExprId, TyListId>,
    /// The declarations that would need endless copies: their instances keep `Error` fields.
    pub endless: HashSet<GenericOwner>,
    /// While `collect` resolves fields and signatures, instances are interned with no fields
    /// and queued here (their declarations' fields may not be known yet).
    pub(crate) collecting: bool,
    pub(crate) pending: Vec<Pending>,
    /// While `collect` runs, the written type arguments waiting for their R3/R7 check (phase 7,
    /// after `compute_owning`); `None` before and after, when they are checked at once.
    pub(crate) queued_args: Option<Vec<rules::QueuedArg>>,
    /// How many parts each type has, as worked out so far (`type_parts`).
    pub(crate) parts: HashMap<Ty, u32>,
    /// The generic of the first instance interned past `MAX_PARTS` since the last report (P42).
    pub(crate) grew: Option<GenericOwner>,
    /// The placeholders with a rule that isn't an interface or a built-in rule (reported).
    pub(crate) refused_rules: HashSet<ParamId>,
}

impl Generics {
    pub fn param(&self, p: ParamId) -> &ParamInfo {
        &self.params[p.0 as usize]
    }
}

impl Tables {
    pub fn intern_ty_list(&mut self, tys: Vec<Ty>) -> TyListId {
        if let Some(&id) = self.generic.ty_list_ids.get(&tys) {
            return id;
        }
        let id = TyListId(self.generic.ty_lists.len() as u32);
        self.generic.ty_lists.push(tys.clone());
        self.generic.ty_list_ids.insert(tys, id);
        id
    }

    pub fn ty_list(&self, l: TyListId) -> &[Ty] {
        &self.generic.ty_lists[l.0 as usize]
    }

    /// The placeholders of a generic owner, in order.
    pub fn owner_params(&self, owner: GenericOwner) -> &[ParamId] {
        match owner {
            GenericOwner::Struct(s) => &self.strukt(s).params,
            GenericOwner::Enum(e) => &self.enm(e).params,
            GenericOwner::Fn(f) => &self.func(f).type_params,
            GenericOwner::Iface(i) => &self.iface(i).params,
        }
    }

    /// A generic owner's name as messages write it (`Stack`, `best`, `Stack.push`).
    pub(crate) fn owner_name(&self, owner: GenericOwner) -> &str {
        match owner {
            GenericOwner::Struct(s) => &self.strukt(s).name,
            GenericOwner::Enum(e) => &self.enm(e).name,
            GenericOwner::Fn(f) => &self.func(f).name,
            GenericOwner::Iface(i) => &self.iface(i).name,
        }
    }

    /// Where a generic owner is declared (its name).
    pub(crate) fn owner_span(&self, owner: GenericOwner) -> Span {
        match owner {
            GenericOwner::Struct(s) => self.strukt(s).span,
            GenericOwner::Enum(e) => self.enm(e).span,
            GenericOwner::Fn(f) => self.func(f).span,
            GenericOwner::Iface(i) => self.iface(i).span,
        }
    }

    /// A generic owner's index in `Program::items` (a method's is its type's).
    pub(crate) fn owner_item(&self, owner: GenericOwner) -> usize {
        match owner {
            GenericOwner::Struct(s) => self.strukt(s).item,
            GenericOwner::Enum(e) => self.enm(e).item,
            GenericOwner::Fn(f) => self.func(f).item,
            GenericOwner::Iface(i) => self.iface(i).item,
        }
    }

    /// A placeholder as a message writes it.
    pub(crate) fn param_name(&self, p: ParamId) -> &str {
        &self.generic.param(p).name
    }
}
