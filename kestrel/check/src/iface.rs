//! Interfaces: their methods as abstract functions, the interfaces a type lists (an instance of a
//! generic one with its types, decision 16), and the check that a struct or enum that lists an
//! interface has everything it promises.

use kestrel_syntax::ast::{IfaceRef, InterfaceDecl, Item, Program, TypeExpr, TypeKind};
use kestrel_syntax::{Diagnostic, Span};

use crate::collect::add_fn;
use crate::collect_ty::{resolve_type, TypeScope};
use crate::generic::{GenericOwner, ParamId};
use crate::tables::{EnumId, FnId, IfaceId, StructId, Tables, BUILTIN_STRUCTS};
use crate::ty::Ty;

/// Whether a value of type `ty` can stand for interface `i`: `ty` is a struct or enum that lists it
/// (an instance lists its declaration's), or a placeholder with `i` as a rule.
pub fn implements(t: &Tables, ty: Ty, i: IfaceId) -> bool {
    match ty {
        Ty::Struct(s) => t.strukt(s).ifaces.contains(&i),
        Ty::Enum(e) => t.enm(e).ifaces.contains(&i),
        Ty::Param(p) => t.generic.param(p).ifaces.contains(&i),
        _ => false,
    }
}

/// Registers the methods of interface `id` as abstract `FnInfo`s (no body, never checked).
/// A method with no `self` first, or named like an earlier one, is reported and left out.
pub(crate) fn add_iface_members(decl: &InterfaceDecl, item_idx: usize, id: IfaceId, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    if decl.methods.is_empty() {
        diags.push(Diagnostic::error(decl.name_span, "an interface needs at least one method"));
    }
    let mut methods: Vec<FnId> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    // A generic interface's methods use its placeholders; `Handler` alone inside it is `Handler[E]`.
    let params = t.iface(id).params.clone();
    let scope = TypeScope { params: &params, own: (!params.is_empty()).then_some(GenericOwner::Iface(id)) };
    for (mi, m) in decl.methods.iter().enumerate() {
        if !m.params.first().is_some_and(|p| p.is_self) {
            let msg = format!("`{}` in `{}` needs `self` or `var self` first", m.name, decl.name);
            diags.push(Diagnostic::error(m.name_span, msg));
            continue;
        }
        if seen.contains(&m.name.as_str()) {
            diags.push(Diagnostic::error(m.name_span, format!("`{}` declares `{}` twice", decl.name, m.name)));
            continue;
        }
        seen.push(&m.name);
        // The `self` parameter is typed as the interface itself.
        let fid = add_fn(m, item_idx, Some((mi, &decl.name)), &scope, t, diags);
        t.fns[fid.0 as usize].is_abstract = true;
        methods.push(fid);
    }
    t.ifaces[id.0 as usize].methods = methods;
}

/// Resolves every struct's and enum's interface list, and checks that each type has a method
/// that fits each method of each interface it lists.
pub(crate) fn check_conformance(program: &Program, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let (mut struct_id, mut enum_id) = (BUILTIN_STRUCTS, 0);
    for item in &program.items {
        match item {
            Item::Struct(s) => {
                let id = StructId(struct_id);
                struct_id += 1;
                let params = t.strukt(id).params.clone();
                let ifaces = resolve_list(&s.name, &s.ifaces, &params, t, diags);
                for &i in &ifaces {
                    check_type(&s.name, s.name_span, &t.strukt(id).methods.clone(), i, t, diags);
                }
                t.structs[id.0 as usize].ifaces = ifaces;
            }
            Item::Enum(e) => {
                let id = EnumId(enum_id);
                enum_id += 1;
                let params = t.enm(id).params.clone();
                let ifaces = resolve_list(&e.name, &e.ifaces, &params, t, diags);
                for &i in &ifaces {
                    check_type(&e.name, e.name_span, &t.enm(id).methods.clone(), i, t, diags);
                }
                t.enums[id.0 as usize].ifaces = ifaces;
            }
            _ => {}
        }
    }
}

/// The interface `r` names: an instance of a generic one with its types (`Handler[Click]`), made
/// as a type is in `scope`. What isn't an interface, or has the wrong types, is reported (`None`).
pub(crate) fn resolve_ref(r: &IfaceRef, scope: &TypeScope, t: &mut Tables, diags: &mut Vec<Diagnostic>) -> Option<IfaceId> {
    if !t.iface_by_name.contains_key(&r.name) {
        diags.push(Diagnostic::error(r.span, format!("`{}` isn't an interface", r.name)));
        return None;
    }
    let ty = TypeExpr { kind: TypeKind::Named { name: r.name.clone(), args: r.args.clone() }, span: r.span };
    match resolve_type(&ty, scope, t, diags) {
        Ty::Iface(i) => Some(i),
        _ => None,
    }
}

/// The interfaces named in the `: A, B` list of type `name` (whose placeholders are `params`),
/// each once. What isn't one, repeats, is a second instance of one interface, or (in a generic
/// type) holds a placeholder, is reported (decision 16).
fn resolve_list(name: &str, list: &[IfaceRef], params: &[ParamId], t: &mut Tables, diags: &mut Vec<Diagnostic>) -> Vec<IfaceId> {
    let scope = TypeScope { params, own: None };
    let mut ids: Vec<IfaceId> = Vec::new();
    for r in list {
        let Some(id) = resolve_ref(r, &scope, t, diags) else { continue };
        let def = t.iface_parts(id).0;
        let shown = t.iface(id).name.clone();
        let msg = if ids.contains(&id) {
            format!("`{shown}` is listed twice")
        } else if let Some(&other) = ids.iter().find(|&&o| t.iface_parts(o).0 == def) {
            format!("`{name}` lists `{}` and `{shown}`; a type implements one instance of an interface", t.iface(other).name)
        } else if !t.iface(id).concrete {
            format!("`{name}` lists `{shown}`; a generic type can list only interfaces with concrete types this round")
        } else {
            ids.push(id);
            continue;
        };
        diags.push(Diagnostic::error(r.span, msg));
    }
    ids
}

/// How a method takes `self`: `None` if it has none, else whether it is `var self`.
fn self_kind(t: &Tables, f: FnId) -> Option<bool> {
    t.func(f).params.first().filter(|p| p.name == "self").map(|p| p.is_var)
}

/// The signature as written in a declaration: `pure hit(var self, var n: int) -> int`.
fn signature_text(t: &Tables, f: FnId) -> String {
    let info = t.func(f);
    let name = info.name.rsplit('.').next().unwrap_or(&info.name);
    let params: Vec<String> = info
        .params
        .iter()
        .map(|p| match (p.name == "self", p.is_var) {
            (true, false) => "self".to_string(),
            (true, true) => "var self".to_string(),
            (false, is_var) => format!("{}{}: {}", if is_var { "var " } else { "" }, p.name, t.ty_name(p.ty)),
        })
        .collect();
    let ret = if info.ret == Ty::Unit { String::new() } else { format!(" -> {}", t.ty_name(info.ret)) };
    format!("{}{name}({}){ret}", if info.is_pure { "pure " } else { "" }, params.join(", "))
}

/// Whether method `got` can stand for the interface's `want`: the same `self`, parameters (types
/// and which are `var`) and result; a `pure` one fits a non-`pure` declaration, not the reverse.
fn fits(t: &Tables, want: FnId, got: FnId) -> bool {
    let (w, g) = (t.func(want), t.func(got));
    if w.ret == Ty::Error || g.ret == Ty::Error || w.params.iter().chain(&g.params).any(|p| p.ty == Ty::Error) {
        return true; // already reported
    }
    let same_params = w.params.len() == g.params.len()
        && self_kind(t, want) == self_kind(t, got)
        && w.params.iter().zip(&g.params).skip(1).all(|(a, b)| a.ty == b.ty && a.is_var == b.is_var);
    same_params && w.ret == g.ret && (g.is_pure || !w.is_pure)
}

fn check_type(
    owner: &str,
    owner_span: Span,
    methods: &std::collections::HashMap<String, FnId>,
    i: IfaceId,
    t: &Tables,
    diags: &mut Vec<Diagnostic>,
) {
    let iface = t.iface(i);
    for &want in &iface.methods {
        let name = t.func(want).name.rsplit('.').next().unwrap_or("").to_string();
        let sig = signature_text(t, want);
        match methods.get(&name) {
            None => {
                let msg = format!("`{owner}` says it is a `{}` but has no method `{name}`; add `{sig}`", iface.name);
                diags.push(Diagnostic::error(owner_span, msg));
            }
            Some(&got) if !fits(t, want, got) => {
                let msg = format!("`{owner}.{name}` doesn't match `{}.{name}`: it should be `{sig}`", iface.name);
                diags.push(Diagnostic::error(t.func(got).span, msg));
            }
            Some(_) => {}
        }
    }
}
