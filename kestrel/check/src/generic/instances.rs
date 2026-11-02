//! Instances of generic structs and enums (G12): one `StructInfo` / `EnumInfo` per declaration
//! and type list, with its fields filled in by substitution, so every type question answers for
//! it as for any struct.
//!
//! An instance copies its declaration's facts: `item`, `span`, `methods`, `ifaces`, each field's
//! `has_default` and `default_allocates`, and its owning-ness. During `collect` those facts arrive
//! in stages, so instances interned there are synced as each one arrives: fields and `methods` in
//! `fill_pending`, `ifaces` after conformance (P23), the field flags after default allocation
//! (P22). An instance interned later copies them all at once.

use crate::generic::{GenericOwner, Instance, Pending, Subst, TyListId};
use crate::tables::{EnumId, EnumInfo, FieldInfo, StructId, StructInfo, Tables, VariantInfo};
use crate::ty::Ty;

impl Tables {
    /// Whether `args` are `params` themselves, in order: the declaration with its own placeholders.
    pub(crate) fn own_args(&self, params: &[crate::generic::ParamId], args: TyListId) -> bool {
        self.ty_list(args).iter().copied().eq(params.iter().map(|&p| Ty::Param(p)))
    }

    /// The instance of generic struct `def` for `args` (`def` itself for its own placeholders).
    pub fn intern_struct(&mut self, def: StructId, args: TyListId) -> StructId {
        let decl = self.strukt(def);
        debug_assert!(!decl.params.is_empty() && decl.instance.is_none(), "only a generic declaration has instances");
        if self.own_args(&decl.params, args) {
            return def;
        }
        // Past the cap, the instance gets a short name and the checker reports it (P42).
        let big = self.too_big(GenericOwner::Struct(def), args);
        if let Some(&id) = self.generic.struct_instances.get(&(def, args)) {
            return id;
        }
        let decl = self.strukt(def);
        let name = if big { format!("{}[...]", decl.name) } else { self.instance_name(&decl.name, self.ty_list(args)) };
        let info = StructInfo {
            name,
            item: decl.item,
            span: decl.span,
            fields: Vec::new(),
            methods: decl.methods.clone(),
            ifaces: decl.ifaces.clone(),
            params: Vec::new(),
            instance: Some(Instance { def, args }),
            concrete: self.ty_list(args).iter().all(|&a| self.is_concrete(a)),
        };
        let id = StructId(self.structs.len() as u32);
        self.structs.push(info);
        // In the map before its fields are filled: a field may name the instance itself.
        self.generic.struct_instances.insert((def, args), id);
        // Once `compute_owning` has run, every struct needs an entry (a missing one reads as
        // not owning). An instance owns exactly when its declaration does (R3).
        if self.owning_structs.len() == id.0 as usize {
            let owning = self.owning_structs[def.0 as usize];
            self.owning_structs.push(owning);
        }
        if self.generic.collecting {
            self.generic.pending.push(Pending::Struct(id));
        } else {
            self.fill_struct(id);
        }
        id
    }

    /// The instance of generic enum `def` for `args` (`def` itself for its own placeholders).
    pub fn intern_enum(&mut self, def: EnumId, args: TyListId) -> EnumId {
        let decl = self.enm(def);
        debug_assert!(!decl.params.is_empty() && decl.instance.is_none(), "only a generic declaration has instances");
        if self.own_args(&decl.params, args) {
            return def;
        }
        // Past the cap, the instance gets a short name and the checker reports it (P42).
        let big = self.too_big(GenericOwner::Enum(def), args);
        if let Some(&id) = self.generic.enum_instances.get(&(def, args)) {
            return id;
        }
        let decl = self.enm(def);
        let name = if big { format!("{}[...]", decl.name) } else { self.instance_name(&decl.name, self.ty_list(args)) };
        let info = EnumInfo {
            name,
            item: decl.item,
            span: decl.span,
            variants: Vec::new(),
            methods: decl.methods.clone(),
            ifaces: decl.ifaces.clone(),
            params: Vec::new(),
            instance: Some(Instance { def, args }),
            concrete: self.ty_list(args).iter().all(|&a| self.is_concrete(a)),
        };
        let id = EnumId(self.enums.len() as u32);
        self.enums.push(info);
        self.generic.enum_instances.insert((def, args), id);
        if self.owning_enums.len() == id.0 as usize {
            let owning = self.owning_enums[def.0 as usize];
            self.owning_enums.push(owning);
        }
        if self.generic.collecting {
            self.generic.pending.push(Pending::Enum(id));
        } else {
            self.fill_enum(id);
        }
        id
    }

    /// `fields` substituted (each `Error` for a declaration that would need endless copies), each
    /// keeping its declaration field's name, span and flags.
    fn filled(&mut self, decl: Vec<FieldInfo>, owner: GenericOwner, args: TyListId) -> Vec<FieldInfo> {
        let endless = self.generic.endless.contains(&owner);
        let s = Subst { owner, args };
        let mut fields = Vec::with_capacity(decl.len());
        for f in decl {
            let ty = if endless { Ty::Error } else { self.subst(f.ty, s) };
            fields.push(FieldInfo { ty, ..f });
        }
        fields
    }

    /// Fills instance `id`'s fields from its declaration, and copies its `methods` and `ifaces` again.
    fn fill_struct(&mut self, id: StructId) {
        let inst = self.strukt(id).instance.expect("an instance");
        let decl = self.strukt(inst.def);
        let (fields, methods, ifaces) = (decl.fields.clone(), decl.methods.clone(), decl.ifaces.clone());
        let fields = self.filled(fields, GenericOwner::Struct(inst.def), inst.args);
        let info = &mut self.structs[id.0 as usize];
        info.fields = fields;
        info.methods = methods;
        info.ifaces = ifaces;
    }

    /// As `fill_struct`, for an enum's variants.
    fn fill_enum(&mut self, id: EnumId) {
        let inst = self.enm(id).instance.expect("an instance");
        let decl = self.enm(inst.def);
        let (variants, methods, ifaces) = (decl.variants.clone(), decl.methods.clone(), decl.ifaces.clone());
        let mut filled = Vec::with_capacity(variants.len());
        for v in variants {
            let fields = self.filled(v.fields, GenericOwner::Enum(inst.def), inst.args);
            filled.push(VariantInfo { name: v.name, span: v.span, fields });
        }
        let info = &mut self.enums[id.0 as usize];
        info.variants = filled;
        info.methods = methods;
        info.ifaces = ifaces;
    }

    /// Phase 5 of `collect`: fills every instance interned while fields and signatures were
    /// resolved (a worklist: filling one may intern more), then interns at once from here on.
    pub(crate) fn fill_pending(&mut self) {
        while let Some(p) = self.generic.pending.pop() {
            match p {
                Pending::Struct(id) => self.fill_struct(id),
                Pending::Enum(id) => self.fill_enum(id),
                Pending::Iface(id) => self.fill_iface(id),
            }
        }
        self.generic.collecting = false;
    }

    /// After `check_conformance` (which fills `ifaces`): every instance's `ifaces` from its declaration (P23).
    pub(crate) fn sync_instance_ifaces(&mut self) {
        for i in 0..self.structs.len() {
            if let Some(inst) = self.structs[i].instance {
                self.structs[i].ifaces = self.strukt(inst.def).ifaces.clone();
            }
        }
        for i in 0..self.enums.len() {
            if let Some(inst) = self.enums[i].instance {
                self.enums[i].ifaces = self.enm(inst.def).ifaces.clone();
            }
        }
    }

    /// After `mark_default_allocation`: every instance field's `has_default` and
    /// `default_allocates` from its declaration's field (P22). The declaration's answer is the
    /// strict one (`has_heap(T)` is true), so it is right for every instance.
    pub(crate) fn sync_instance_field_flags(&mut self) {
        for i in 0..self.structs.len() {
            let Some(inst) = self.structs[i].instance else { continue };
            for f in 0..self.structs[i].fields.len() {
                let d = &self.structs[inst.def.0 as usize].fields[f];
                let (has_default, allocates) = (d.has_default, d.default_allocates);
                let field = &mut self.structs[i].fields[f];
                field.has_default = has_default;
                field.default_allocates = allocates;
            }
        }
        for i in 0..self.enums.len() {
            let Some(inst) = self.enums[i].instance else { continue };
            for v in 0..self.enums[i].variants.len() {
                for f in 0..self.enums[i].variants[v].fields.len() {
                    let d = &self.enums[inst.def.0 as usize].variants[v].fields[f];
                    let (has_default, allocates) = (d.has_default, d.default_allocates);
                    let field = &mut self.enums[i].variants[v].fields[f];
                    field.has_default = has_default;
                    field.default_allocates = allocates;
                }
            }
        }
    }
}
