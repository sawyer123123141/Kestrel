use std::collections::{HashMap, HashSet};

use kestrel_syntax::ast::{EnumDecl, Expr, ExprKind, FieldDecl, FnDecl, Item, Program, StructDecl};
use kestrel_syntax::{Diagnostic, Span};

use crate::builtins::Builtin;
use crate::collect_ty::{check_self_containment, resolve_type, TypeScope};
use crate::generic::declare::{check_rule_clashes, declare_fn_params, declare_type_params};
use crate::generic::expand::check_type_expansion;
use crate::generic::GenericOwner;
use crate::sig::{Sig, SigParam};
use crate::tables::{
    is_prelude_result, ConstInfo, EnumId, EnumInfo, FieldInfo, FnId, FnInfo, IfaceId, IfaceInfo, ParamSig, StructId, StructInfo, Tables,
    VariantInfo, BUILTIN_STRUCTS, RESULT,
};
use crate::ty::Ty;
use crate::vectors::COMPONENTS;

pub(crate) const BUILTINS: &[&str] = &[
    "print", "int", "i64", "u8", "float", "f64", "bool", "string", "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4", "array", "list", "map", "Arena",
    "GpuHandle",
];

/// Names an item, local or parameter can't take: types and constructors (`BUILTINS`),
/// and every global built-in function or constant.
pub(crate) fn is_builtin_name(name: &str) -> bool {
    BUILTINS.contains(&name) || Builtin::GLOBAL_NAMES.contains(&name)
}

/// The prefix of the names of the functions the compiler makes (`__kestrel_constants`,
/// `__kestrel_copy_N`, ...). A user name with it would collide with one of them.
const COMPILER_PREFIX: &str = "__kestrel_";

/// Whether no item, local or parameter may take `name`: a built-in name, or one that starts with
/// `COMPILER_PREFIX` (see `reserved_name_error`).
pub fn is_reserved_name(name: &str) -> bool {
    is_builtin_name(name) || name.starts_with(COMPILER_PREFIX)
}

/// The error for a name an item, local or parameter can't take, if it is one: a built-in name,
/// or one that starts with `COMPILER_PREFIX`. `shown` is the name as the message writes it.
pub(crate) fn reserved_name_error(name: &str, shown: &str) -> Option<String> {
    if name == RESULT {
        Some(format!("`{RESULT}` is a built-in type; pick another name"))
    } else if is_builtin_name(name) {
        Some(format!("`{shown}` is a built-in name; pick another"))
    } else if name.starts_with(COMPILER_PREFIX) {
        Some(format!("names starting with `{COMPILER_PREFIX}` are reserved for the compiler; pick another"))
    } else {
        None
    }
}


/// Records every type, function signature and constant before bodies are
/// checked. Type names come first so any type can refer to any other. The phases (generics spec,
/// "Checker: declarations"): 1-2 names, placeholders and their rules; 3 fields and signatures
/// (instances named there wait, pending, and their arguments' R3/R7 checks are queued); G9;
/// 4 the type expansion check; 5 the pending instances filled; 6 conformance, default allocation
/// (each followed by the instances' sync), self-containment and natives; 7 owning-ness, then the
/// queued R3/R7 checks.
pub(crate) fn collect(program: &Program, t: &mut Tables, diags: &mut Vec<Diagnostic>, needs_main: bool) {
    check_names(program, diags);
    t.generic.queued_args = Some(Vec::new());
    add_builtin_vectors(t);
    for (item_idx, item) in program.items.iter().enumerate() {
        match item {
            Item::Struct(s) => {
                let id = StructId(t.structs.len() as u32);
                t.struct_by_name.entry(s.name.clone()).or_insert(id);
                t.structs.push(StructInfo {
                    name: s.name.clone(),
                    item: item_idx,
                    span: s.name_span,
                    fields: Vec::new(),
                    methods: HashMap::new(),
                    ifaces: Vec::new(),
                    params: Vec::new(),
                    instance: None,
                    // A generic declaration's fields use its placeholders (P27).
                    concrete: s.type_params.is_empty(),
                });
            }
            Item::Enum(e) => {
                let id = EnumId(t.enums.len() as u32);
                t.enum_by_name.entry(e.name.clone()).or_insert(id);
                // The prelude's `Result` is the type of that name, even beside a refused one.
                if is_prelude_result(&e.name, e.name_span, &program.builtin_ranges) {
                    t.result = Some(id);
                    t.enum_by_name.insert(e.name.clone(), id);
                }
                t.enums.push(EnumInfo {
                    name: e.name.clone(),
                    item: item_idx,
                    span: e.name_span,
                    variants: Vec::new(),
                    methods: HashMap::new(),
                    ifaces: Vec::new(),
                    params: Vec::new(),
                    instance: None,
                    concrete: e.type_params.is_empty(),
                });
            }
            Item::Interface(i) => {
                let id = IfaceId(t.ifaces.len() as u32);
                t.iface_by_name.entry(i.name.clone()).or_insert(id);
                t.ifaces.push(IfaceInfo { name: i.name.clone(), item: item_idx, span: i.name_span, methods: Vec::new() });
            }
            Item::Fn(_) | Item::Const(_) | Item::Use(_) => {}
        }
    }
    declare_type_params(program, t, diags);
    t.generic.collecting = true;
    let (mut struct_id, mut enum_id, mut iface_id) = (BUILTIN_STRUCTS, 0, 0);
    for (item_idx, item) in program.items.iter().enumerate() {
        match item {
            Item::Fn(f) => {
                let id = add_fn(f, item_idx, None, &TypeScope::NONE, t, diags);
                if f.name == "main" && t.main.is_none() {
                    if !f.type_params.is_empty() {
                        diags.push(Diagnostic::error(f.name_span, "`main` can't have placeholders"));
                    }
                    if !f.params.is_empty() || f.ret.is_some() {
                        diags.push(Diagnostic::error(f.name_span, "`main` takes no parameters and returns nothing"));
                    }
                    t.main = Some(id);
                }
                t.fn_by_name.entry(f.name.clone()).or_insert(id);
            }
            Item::Const(c) => {
                t.consts.push(ConstInfo { name: c.bind.name.clone(), ty: Ty::Error, item: item_idx });
            }
            Item::Struct(s) => {
                add_struct_members(s, item_idx, StructId(struct_id), t, diags);
                struct_id += 1;
            }
            Item::Enum(e) => {
                add_enum_members(e, item_idx, EnumId(enum_id), t, diags);
                enum_id += 1;
            }
            Item::Interface(i) => {
                crate::iface::add_iface_members(i, item_idx, IfaceId(iface_id), t, diags);
                iface_id += 1;
            }
            Item::Use(_) => {}
        }
    }
    // G9 compares the interfaces' method signatures, made in the loop above (P26).
    check_rule_clashes(program, t, diags);
    // Endless declarations are marked before filling, so filling their instances ends (G10).
    check_type_expansion(t, diags);
    t.fill_pending();
    crate::iface::check_conformance(program, t, diags);
    t.sync_instance_ifaces();
    mark_default_allocation(program, t);
    t.sync_instance_field_flags();
    check_self_containment(t, diags);
    crate::natives::check_natives(program, t, diags);
    t.compute_owning();
    t.check_queued_args(diags);
    // A program without `main` may be reloadable instead (`setup` and `frame`).
    crate::reload::check_entry(program, t, diags, needs_main);
}

/// `vec2` and `vec3` (structs of floats) and `ivec2` and `ivec3` (structs of ints), always
/// `StructId` 0 to 3 (see `VEC2` ... `IVEC3`).
fn add_builtin_vectors(t: &mut Tables) {
    for (name, dims, lane) in [("vec2", 2, Ty::Float), ("vec3", 3, Ty::Float), ("ivec2", 2, Ty::Int), ("ivec3", 3, Ty::Int)] {
        let id = StructId(t.structs.len() as u32);
        t.struct_by_name.insert(name.to_string(), id);
        let fields = COMPONENTS[..dims]
            .iter()
            .map(|c| FieldInfo { name: c.to_string(), ty: lane, span: Span::default(), has_default: false, default_allocates: false })
            .collect();
        let (methods, ifaces, params) = (HashMap::new(), Vec::new(), Vec::new());
        let span = Span::default();
        t.structs.push(StructInfo { name: name.to_string(), item: usize::MAX, span, fields, methods, ifaces, params, instance: None, concrete: true });
    }
}

/// Top-level names must be unique and can't reuse a built-in name.
fn check_names(program: &Program, diags: &mut Vec<Diagnostic>) {
    let mut seen: HashSet<&str> = HashSet::new();
    for item in &program.items {
        let (name, span) = match item {
            Item::Fn(f) => (&f.name, f.name_span),
            Item::Const(c) => (&c.bind.name, c.bind.span),
            Item::Struct(s) => (&s.name, s.name_span),
            Item::Enum(e) => (&e.name, e.name_span),
            Item::Interface(i) => (&i.name, i.name_span),
            Item::Use(_) => continue,
        };
        // Items from other modules are named `module.name`.
        let own = name.rsplit('.').next().unwrap_or(name);
        if is_prelude_result(name, span, &program.builtin_ranges) {
            seen.insert(name);
        } else if let Some(msg) = reserved_name_error(own, name) {
            diags.push(Diagnostic::error(span, msg));
        } else if !seen.insert(name) {
            diags.push(Diagnostic::error(span, format!("`{name}` is defined twice")));
        }
    }
}

/// Resolves field types; reports duplicate field names.
fn fields_of(owner: &str, decls: &[FieldDecl], scope: &TypeScope, t: &mut Tables, diags: &mut Vec<Diagnostic>) -> Vec<FieldInfo> {
    let mut fields: Vec<FieldInfo> = Vec::new();
    for f in decls {
        if fields.iter().any(|g| g.name == f.name) {
            diags.push(Diagnostic::error(f.name_span, format!("`{owner}` has two fields named `{}`", f.name)));
            continue;
        }
        let ty = resolve_type(&f.ty, scope, t, diags);
        // `mark_default_allocation` fills in `default_allocates` once every field type is known.
        fields.push(FieldInfo { name: f.name.clone(), ty, span: f.name_span, has_default: f.default.is_some(), default_allocates: false });
    }
    fields
}

/// Sets `FieldInfo::default_allocates` for every struct field with a default. It needs all
/// field types (a struct's heap memory depends on its fields), so it runs after they are collected.
fn mark_default_allocation(program: &Program, t: &mut Tables) {
    let mut struct_id = BUILTIN_STRUCTS as usize;
    for item in &program.items {
        let Item::Struct(s) = item else { continue };
        for decl in &s.fields {
            let Some(default) = &decl.default else { continue };
            let Some(i) = t.structs[struct_id].fields.iter().position(|f| f.name == decl.name) else { continue };
            let ty = t.structs[struct_id].fields[i].ty;
            t.structs[struct_id].fields[i].default_allocates = default_allocates(default, ty, t);
        }
        struct_id += 1;
    }
}

/// Whether building `default` as a value of type `ty` puts memory in the current arena. Anything
/// that holds heap memory does, except the cases that point at no arena: `none`, a string literal
/// (or a string constant, which is one), and an empty array (an empty list still grows in its arena).
fn default_allocates(default: &Expr, ty: Ty, t: &Tables) -> bool {
    if !t.has_heap(ty) {
        return false;
    }
    // A `T?` default is a `T` or `none`.
    let inner = match ty {
        Ty::Optional(e) => t.elem(e),
        other => other,
    };
    match &default.kind {
        ExprKind::None | ExprKind::Str(_) => false,
        // A closure that captures nothing has no environment, like a named function.
        ExprKind::Closure(_) => false,
        ExprKind::Ident(name) if inner == Ty::Str => !t.consts.iter().any(|c| &c.name == name),
        // A named function has no environment: it holds no memory.
        ExprKind::Ident(name) if matches!(inner, Ty::Fn(_)) && t.fn_by_name.contains_key(name) => false,
        ExprKind::ArrayLit(items) if items.is_empty() => matches!(inner, Ty::List(_)),
        _ => true,
    }
}

/// A field or variant name a method can't reuse; `is_fn` for a field of function type.
struct Taken<'a> {
    name: &'a str,
    is_fn: bool,
}

/// Registers methods of `(owner, its item index)`; `taken` names (of `taken_noun`s: fields or
/// variants) can't be reused.
fn methods_of(
    (owner, item_idx): (&str, usize),
    decls: &[FnDecl],
    taken: &[Taken],
    taken_noun: &str,
    scope: &TypeScope,
    t: &mut Tables,
    diags: &mut Vec<Diagnostic>,
) -> HashMap<String, FnId> {
    let mut methods = HashMap::new();
    for (mi, m) in decls.iter().enumerate() {
        let clash = taken.iter().find(|k| k.name == m.name);
        if clash.is_some_and(|k| k.is_fn) {
            // `b.on_click()` would mean both.
            let msg = format!("`{owner}` has a field and a method both called `{}`; rename one", m.name);
            diags.push(Diagnostic::error(m.name_span, msg));
            continue;
        }
        if clash.is_some() {
            diags.push(Diagnostic::error(m.name_span, format!("`{owner}` already has a {taken_noun} named `{}`", m.name)));
            continue;
        }
        if methods.contains_key(&m.name) {
            diags.push(Diagnostic::error(m.name_span, format!("`{owner}` has two methods named `{}`", m.name)));
            continue;
        }
        let fid = add_fn(m, item_idx, Some((mi, owner)), scope, t, diags);
        methods.insert(m.name.clone(), fid);
    }
    methods
}

fn add_struct_members(s: &StructDecl, item_idx: usize, id: StructId, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let params = t.strukt(id).params.clone();
    let scope = TypeScope { params: &params, own: (!params.is_empty()).then_some(GenericOwner::Struct(id)) };
    let fields = fields_of(&s.name, &s.fields, &scope, t, diags);
    let names: Vec<Taken> = fields.iter().map(|f| Taken { name: &f.name, is_fn: matches!(f.ty, Ty::Fn(_)) }).collect();
    let methods = methods_of((&s.name, item_idx), &s.methods, &names, "field", &scope, t, diags);
    let info = &mut t.structs[id.0 as usize];
    info.fields = fields;
    info.methods = methods;
}

fn add_enum_members(e: &EnumDecl, item_idx: usize, id: EnumId, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let params = t.enm(id).params.clone();
    let scope = TypeScope { params: &params, own: (!params.is_empty()).then_some(GenericOwner::Enum(id)) };
    let mut variants: Vec<VariantInfo> = Vec::new();
    for v in &e.variants {
        if variants.iter().any(|w| w.name == v.name) {
            diags.push(Diagnostic::error(v.name_span, format!("`{}` has two variants named `{}`", e.name, v.name)));
            continue;
        }
        let fields = fields_of(&v.name, &v.fields, &scope, t, diags);
        variants.push(VariantInfo { name: v.name.clone(), span: v.name_span, fields });
    }
    if e.variants.is_empty() {
        diags.push(Diagnostic::error(e.name_span, format!("`{}` needs at least one variant", e.name)));
    }
    let names: Vec<Taken> = variants.iter().map(|v| Taken { name: &v.name, is_fn: false }).collect();
    let methods = methods_of((&e.name, item_idx), &e.methods, &names, "variant", &scope, t, diags);
    let info = &mut t.enums[id.0 as usize];
    info.variants = variants;
    info.methods = methods;
}

/// Registers a function's signature. `method` is (index in its type, type name); `outer` is the
/// type scope of a method's struct or enum. A generic function's placeholders are registered
/// here, where its `FnId` is known (P29).
pub(crate) fn add_fn(
    f: &FnDecl,
    item_idx: usize,
    method: Option<(usize, &str)>,
    outer: &TypeScope,
    t: &mut Tables,
    diags: &mut Vec<Diagnostic>,
) -> FnId {
    // Resolving types makes no functions, so this is the id `t.fns.push` below gives.
    let id = FnId(t.fns.len() as u32);
    let type_params = if f.type_params.is_empty() { Vec::new() } else { declare_fn_params(f, id, t, diags) };
    let (scope, generic) = if type_params.is_empty() {
        (*outer, outer.own)
    } else {
        (TypeScope { params: &type_params, own: None }, Some(GenericOwner::Fn(id)))
    };
    let mut params = Vec::new();
    for p in &f.params {
        let ty = resolve_type(&p.ty, &scope, t, diags);
        params.push(ParamSig { name: p.bind.name.clone(), ty, is_var: p.is_var });
    }
    let ret = f.ret.as_ref().map_or(Ty::Unit, |r| resolve_type(r, &scope, t, diags));
    assert_eq!(id.0 as usize, t.fns.len(), "resolving a signature added a function");
    let sig = Sig { params: params.iter().map(|p| SigParam { ty: p.ty, is_var: p.is_var }).collect(), ret, is_pure: f.is_pure };
    let sig = t.intern_sig(sig);
    let name = match method {
        Some((_, owner)) => format!("{owner}.{}", f.name),
        None => f.name.clone(),
    };
    t.fns.push(FnInfo {
        name,
        item: item_idx,
        method: method.map(|(mi, _)| mi),
        is_pure: f.is_pure,
        is_native: f.is_native,
        params,
        ret,
        sig,
        span: f.name_span,
        locals: Vec::new(),
        // A native may do anything; only `pure` ones promise not to.
        io: f.is_native && !f.is_pure,
        mutates_params: false,
        is_abstract: false,
        generic,
        type_params,
        copy: None,
    });
    id
}
