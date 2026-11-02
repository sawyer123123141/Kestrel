//! Declaring placeholders: their names and rules (`collect` phases 1 and 2), and the check that
//! two interface rules of one placeholder can both be met (G9, after phase 3).
//!
//! A struct's or enum's placeholders are registered once every type is known by name; a
//! function's inside `add_fn`, where its `FnId` (the owner) is made (P29). Invalid placeholders
//! are still registered, so their uses don't add errors.

use kestrel_syntax::ast::{FnDecl, IfaceRef, Item, Program, TypeParam};
use kestrel_syntax::{Diagnostic, Span};

use crate::collect::BUILTINS;
use crate::collect_ty::TypeScope;
use crate::generic::{GenericOwner, ParamId, ParamInfo};
use crate::tables::{EnumId, FnId, IfaceId, StructId, Tables, BUILTIN_STRUCTS, RESULT};

/// The three built-in rules. They are words only in a rule list.
const RULE_WORDS: [&str; 3] = ["equatable", "ordered", "key"];

/// Phases 1 and 2 for structs, enums and interfaces: registers each generic declaration's
/// placeholders (checking their names), then resolves their rules; and refuses interfaces named
/// after a rule.
pub(crate) fn declare_type_params(program: &Program, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let (mut struct_id, mut enum_id, mut iface_id) = (BUILTIN_STRUCTS, 0, 0);
    let mut declared: Vec<(Vec<ParamId>, &[TypeParam])> = Vec::new();
    for item in &program.items {
        match item {
            Item::Struct(s) => {
                let id = StructId(struct_id);
                struct_id += 1;
                let params = register(&s.type_params, GenericOwner::Struct(id), t, diags);
                t.structs[id.0 as usize].params = params.clone();
                declared.push((params, &s.type_params));
            }
            Item::Enum(e) => {
                let id = EnumId(enum_id);
                enum_id += 1;
                let params = register(&e.type_params, GenericOwner::Enum(id), t, diags);
                t.enums[id.0 as usize].params = params.clone();
                declared.push((params, &e.type_params));
            }
            Item::Interface(i) => {
                let id = IfaceId(iface_id);
                iface_id += 1;
                let params = register(&i.type_params, GenericOwner::Iface(id), t, diags);
                t.ifaces[id.0 as usize].params = params.clone();
                declared.push((params, &i.type_params));
            }
            _ => {}
        }
    }
    for (params, decls) in declared {
        for (&p, decl) in params.iter().zip(decls) {
            apply_rules(p, &decl.rules, &params, t, diags);
        }
    }
    for item in &program.items {
        if let Item::Interface(i) = item {
            let own = i.name.rsplit('.').next().unwrap_or(&i.name);
            if RULE_WORDS.contains(&own) {
                diags.push(Diagnostic::error(i.name_span, format!("`{own}` is a built-in rule's name; pick another")));
            }
        }
    }
}

/// A generic function's placeholders, named and given their rules; `id` is the `FnId` its
/// `FnInfo` will have (P29).
pub(crate) fn declare_fn_params(f: &FnDecl, id: FnId, t: &mut Tables, diags: &mut Vec<Diagnostic>) -> Vec<ParamId> {
    let params = register(&f.type_params, GenericOwner::Fn(id), t, diags);
    for (&p, decl) in params.iter().zip(&f.type_params) {
        apply_rules(p, &decl.rules, &params, t, diags);
    }
    params
}

/// Registers placeholders in order, reporting bad names.
fn register(decls: &[TypeParam], owner: GenericOwner, t: &mut Tables, diags: &mut Vec<Diagnostic>) -> Vec<ParamId> {
    let mut ids = Vec::with_capacity(decls.len());
    for (index, decl) in decls.iter().enumerate() {
        if let Some(msg) = name_error(&decl.name, &decls[..index], t, t.in_builtin(decl.span)) {
            diags.push(Diagnostic::error(decl.span, msg));
        }
        ids.push(ParamId(t.generic.params.len() as u32));
        t.generic.params.push(ParamInfo {
            name: decl.name.clone(),
            span: decl.span,
            owner,
            index: index as u32,
            ifaces: Vec::new(),
            equatable: false,
            ordered: false,
            key: false,
        });
    }
    ids
}

/// What is wrong with a placeholder's name, if anything: it starts with a capital letter (G3),
/// isn't `Result` or the own name of a type (P13, G16) and isn't listed twice. `builtin` is
/// whether it is declared in a built-in module.
fn name_error(name: &str, earlier: &[TypeParam], t: &Tables, builtin: bool) -> Option<String> {
    let own = |n: &String| n.rsplit('.').next() == Some(name);
    if !name.starts_with(char::is_uppercase) {
        Some("a placeholder's name starts with a capital letter, like `T` or `Item`".to_string())
    } else if name == RESULT {
        Some(format!("`{RESULT}` is a built-in type; pick another name"))
    } else if (BUILTINS.contains(&name) && name != "print")
        // A built-in module's placeholder (the prelude's `T` and `E`) isn't checked against the
        // user's types: a user's `struct T` must not refuse it.
        || (!builtin && (t.struct_by_name.keys().any(own) || t.enum_by_name.keys().any(own) || t.iface_by_name.keys().any(own)))
    {
        Some(format!("`{name}` is already a type, so it can't be a placeholder's name; pick another, like `T`"))
    } else if earlier.iter().any(|e| e.name == name) {
        Some(format!("`{name}` is listed twice"))
    } else {
        None
    }
}

/// Resolves a placeholder's rules: interfaces (an instance with its types, which may name the
/// placeholders `params` of the same list: `[G: Rollback[I], I: equatable]`), or the three words.
fn apply_rules(p: ParamId, rules: &[IfaceRef], params: &[ParamId], t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let (mut ifaces, mut equatable, mut ordered, mut key) = (Vec::new(), false, false, false);
    let mut seen: Vec<String> = Vec::new();
    let scope = TypeScope { params, own: None };
    for r in rules {
        let (rule, span) = (r.to_sexpr(), r.span);
        if seen.contains(&rule) {
            diags.push(Diagnostic::error(span, format!("`{rule}` is listed twice")));
            continue;
        }
        seen.push(rule.clone());
        if RULE_WORDS.contains(&r.name.as_str()) && !r.args.is_empty() {
            diags.push(Diagnostic::error(span, format!("`{}` takes no types", r.name)));
            continue;
        }
        match r.name.as_str() {
            "equatable" => equatable = true,
            "ordered" => ordered = true,
            "key" => key = true,
            _ if !t.iface_by_name.contains_key(&r.name) => {
                let msg = format!("`{}` isn't an interface or a built-in rule (`equatable`, `ordered` or `key`)", r.name);
                diags.push(Diagnostic::error(span, msg));
                // Its methods are unknown: a call of one isn't reported again.
                t.generic.refused_rules.insert(p);
            }
            _ => {
                let Some(i) = crate::iface::resolve_ref(r, &scope, t, diags) else {
                    t.generic.refused_rules.insert(p);
                    continue;
                };
                // Decision 17: one instance of an interface on one placeholder.
                let def = t.iface_parts(i).0;
                if ifaces.contains(&i) {
                    diags.push(Diagnostic::error(span, format!("`{rule}` is listed twice")));
                } else if let Some(&other) = ifaces.iter().find(|&&o| t.iface_parts(o).0 == def) {
                    let (name, a, b) = (t.param_name(p), &t.iface(other).name, &t.iface(i).name);
                    diags.push(Diagnostic::error(span, format!("`{name}` can't have both `{a}` and `{b}`; keep one")));
                } else {
                    ifaces.push(i);
                }
            }
        }
    }
    let info = &mut t.generic.params[p.0 as usize];
    info.ifaces = ifaces;
    // `ordered` and `key` include `equatable` (G4).
    info.equatable = equatable || ordered || key;
    info.ordered = ordered;
    info.key = key;
}

/// G9 (after phase 3, P26): two interface rules of one placeholder whose methods share a name
/// must give them the same signature, or no type could meet both.
pub(crate) fn check_rule_clashes(program: &Program, t: &Tables, diags: &mut Vec<Diagnostic>) {
    for info in t.generic.params.iter().filter(|p| p.ifaces.len() >= 2) {
        let decl = match info.owner {
            GenericOwner::Struct(s) => match &program.items[t.strukt(s).item] {
                Item::Struct(d) => &d.type_params,
                _ => continue,
            },
            GenericOwner::Enum(e) => match &program.items[t.enm(e).item] {
                Item::Enum(d) => &d.type_params,
                _ => continue,
            },
            GenericOwner::Fn(f) => &t.func(f).decl(program).type_params,
            GenericOwner::Iface(i) => match &program.items[t.iface(i).item] {
                Item::Interface(d) => &d.type_params,
                _ => continue,
            },
        };
        // Each interface rule once, with where it is written: the instance of the interface it
        // names (one per interface, decision 17).
        let mut rules: Vec<(IfaceId, Span)> = Vec::new();
        for r in &decl[info.index as usize].rules {
            let Some(&def) = t.iface_by_name.get(&r.name) else { continue };
            let found = info.ifaces.iter().find(|&&i| t.iface_parts(i).0 == def && !rules.iter().any(|(j, _)| *j == i));
            if let Some(&i) = found {
                rules.push((i, r.span));
            }
        }
        for later in 1..rules.len() {
            let (b, span) = rules[later];
            if let Some((a, m)) = rules[..later].iter().find_map(|&(a, _)| clash(t, a, b).map(|m| (a, m))) {
                let msg = format!("no type can be both `{}` and `{}`: their `{m}` methods differ", t.iface(a).name, t.iface(b).name);
                diags.push(Diagnostic::error(span, msg));
            }
        }
    }
}

/// The first method of `a` that `b` has with another signature.
fn clash(t: &Tables, a: IfaceId, b: IfaceId) -> Option<String> {
    let short = |f: FnId| t.func(f).name.rsplit('.').next().unwrap_or("").to_string();
    for &ma in &t.iface(a).methods {
        let name = short(ma);
        let Some(&mb) = t.iface(b).methods.iter().find(|&&mb| short(mb) == name) else { continue };
        if !same_signature(t, ma, mb) {
            return Some(name);
        }
    }
    None
}

/// The same `self` (`self` or `var self`), parameters (types and which are `var`) and result,
/// compared both ways with `pure` ignored (P26: a call through `T` is `pure` when either is).
fn same_signature(t: &Tables, a: FnId, b: FnId) -> bool {
    let (a, b) = (t.func(a), t.func(b));
    if a.ret == crate::ty::Ty::Error || b.ret == crate::ty::Ty::Error || a.params.iter().chain(&b.params).any(|p| p.ty == crate::ty::Ty::Error) {
        return true; // already reported
    }
    a.params.len() == b.params.len()
        && a.params.first().map(|p| p.is_var) == b.params.first().map(|p| p.is_var)
        && a.params.iter().zip(&b.params).skip(1).all(|(x, y)| x.ty == y.ty && x.is_var == y.is_var)
        && a.ret == b.ret
}
