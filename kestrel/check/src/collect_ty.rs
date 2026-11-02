//! Resolving written types, map types, and the check that a type can't contain itself.

use std::collections::HashSet;

use kestrel_syntax::ast::{TypeExpr, TypeKind};
use kestrel_syntax::{Diagnostic, Span};

use crate::collect::BUILTINS;
use crate::generic::{GenericOwner, ParamId};
use crate::sig::{Sig, SigParam};
use crate::tables::{EnumId, StructId, Tables};
use crate::ty::Ty;

/// `((int) -> bool)?` can't be told from a function that takes a function, so an optional
/// function type isn't offered; a do-nothing default does the same job.
const OPTIONAL_FN: &str =
    "optional function types aren't supported yet; give the field a do-nothing default instead: `on_hit: (int) = (n) { }`";

/// The placeholders a written type may name, and the generic declaration whose bare name means
/// itself (G2: `Stack` inside `struct Stack[T]` is `Stack[T]`).
#[derive(Clone, Copy)]
pub(crate) struct TypeScope<'a> {
    pub params: &'a [ParamId],
    pub own: Option<GenericOwner>,
}

impl<'a> TypeScope<'a> {
    /// Plain code: no placeholders.
    pub const NONE: TypeScope<'a> = TypeScope { params: &[], own: None };
}

pub(crate) fn resolve_type(ty: &TypeExpr, scope: &TypeScope, t: &mut Tables, diags: &mut Vec<Diagnostic>) -> Ty {
    let msg = match &ty.kind {
        TypeKind::Named { name, args } if args.is_empty() => {
            if let Some(&p) = scope.params.iter().find(|&&p| t.generic.param(p).name == *name) {
                return Ty::Param(p);
            }
            match Ty::from_name(name) {
                Some(Ty::Gpu) if !t.in_builtin(ty.span) => "`GpuHandle` is only for Kestrel's built-in modules".to_string(),
                Some(found) => return found,
                None => match bare_name(name, scope, t) {
                    Ok(found) => return found,
                    Err(msg) => msg,
                },
            }
        }
        TypeKind::Named { name, args } if name == "list" && args.len() == 1 => {
            let elem = resolve_type(&args[0], scope, t, diags);
            return if elem == Ty::Error { Ty::Error } else { Ty::List(t.intern_elem(elem)) };
        }
        TypeKind::Named { name, .. } if name == "list" => "`list[...]` takes one element type".to_string(),
        TypeKind::Named { name, args } if name == "map" && args.len() == 2 => {
            let k = resolve_type(&args[0], scope, t, diags);
            let v = resolve_type(&args[1], scope, t, diags);
            return map_ty(k, v, args[0].span, ty.span, t, diags);
        }
        TypeKind::Named { name, .. } if name == "map" => "`map[...]` takes a key type and a value type".to_string(),
        TypeKind::Named { name, .. } if name == "array" => "write array types as `[T]`".to_string(),
        TypeKind::Named { name, args } => match named_with_args(name, args, scope, t, diags) {
            Ok(found) => return found,
            Err(msg) => msg,
        },
        TypeKind::Array(inner) => {
            let elem = resolve_type(inner, scope, t, diags);
            return if elem == Ty::Error { Ty::Error } else { Ty::Array(t.intern_elem(elem)) };
        }
        TypeKind::Optional(inner) => {
            let elem = resolve_type(inner, scope, t, diags);
            if matches!(elem, Ty::Fn(_)) {
                OPTIONAL_FN.to_string()
            } else {
                return if elem == Ty::Error { Ty::Error } else { Ty::Optional(t.intern_elem(elem)) };
            }
        }
        TypeKind::Fn { params, ret, is_pure } => {
            let params: Vec<SigParam> =
                params.iter().map(|p| SigParam { ty: resolve_type(&p.ty, scope, t, diags), is_var: p.is_var }).collect();
            let ret = ret.as_ref().map_or(Ty::Unit, |r| resolve_type(r, scope, t, diags));
            if ret == Ty::Error || params.iter().any(|p| p.ty == Ty::Error) {
                return Ty::Error;
            }
            return Ty::Fn(t.intern_sig(Sig { params, ret, is_pure: *is_pure }));
        }
    };
    diags.push(Diagnostic::error(ty.span, msg));
    Ty::Error
}

/// A struct, enum or interface named with no types. A generic one needs its types, except inside
/// its own declaration (G2).
fn bare_name(name: &str, scope: &TypeScope, t: &Tables) -> Result<Ty, String> {
    let (ty, owner, count) = if let Some(&id) = t.struct_by_name.get(name) {
        (Ty::Struct(id), GenericOwner::Struct(id), t.strukt(id).params.len())
    } else if let Some(&id) = t.enum_by_name.get(name) {
        (Ty::Enum(id), GenericOwner::Enum(id), t.enm(id).params.len())
    } else if let Some(&id) = t.iface_by_name.get(name) {
        (Ty::Iface(id), GenericOwner::Iface(id), t.iface(id).params.len())
    } else {
        return Err(format!("unknown type `{name}`"));
    };
    if count == 0 || scope.own == Some(owner) {
        return Ok(ty);
    }
    let ints = vec!["int"; count].join(", ");
    Err(format!("`{name}` needs its types, like `{name}[{ints}]`"))
}

/// `Name[A, B]` for a name other than `list`, `map` and `array`: an instance of a generic struct,
/// enum or interface (interned, so each is one type) when the count of types matches.
fn named_with_args(name: &str, args: &[TypeExpr], scope: &TypeScope, t: &mut Tables, diags: &mut Vec<Diagnostic>) -> Result<Ty, String> {
    let (owner, count) = if let Some(&id) = t.struct_by_name.get(name) {
        (GenericOwner::Struct(id), t.strukt(id).params.len())
    } else if let Some(&id) = t.enum_by_name.get(name) {
        (GenericOwner::Enum(id), t.enm(id).params.len())
    } else if let Some(&id) = t.iface_by_name.get(name) {
        (GenericOwner::Iface(id), t.iface(id).params.len())
    } else if BUILTINS.contains(&name)
        || scope.params.iter().any(|&p| t.generic.param(p).name == name)
    {
        return Err(format!("`{name}` takes no types"));
    } else {
        return Err(format!("unknown type `{name}`"));
    };
    match count {
        0 => return Err(format!("`{name}` takes no types")),
        1 if args.len() != 1 => return Err(format!("`{name}` takes 1 type, found {}", args.len())),
        n if n != args.len() => return Err(format!("`{name}` takes {n} types, found {}", args.len())),
        _ => {}
    }
    let mut tys = Vec::with_capacity(args.len());
    for a in args {
        tys.push(resolve_type(a, scope, t, diags));
    }
    if tys.contains(&Ty::Error) {
        return Ok(Ty::Error);
    }
    // R3/R7: queued during `collect`; in bodies a refused argument makes no instance.
    let spans: Vec<Span> = args.iter().map(|a| a.span).collect();
    if t.check_written_args(owner, &tys, &spans, scope, diags) {
        return Ok(Ty::Error);
    }
    let list = t.intern_ty_list(tys);
    Ok(match owner {
        GenericOwner::Struct(def) => Ty::Struct(t.intern_struct(def, list)),
        GenericOwner::Enum(def) => Ty::Enum(t.intern_enum(def, list)),
        GenericOwner::Iface(def) => Ty::Iface(t.intern_iface(def, list)),
        GenericOwner::Fn(_) => unreachable!("a type's name is a struct's, an enum's or an interface's"),
    })
}

/// `map[k, v]`, once `k` is known to work as a key (else an error at
/// `key_span`). Value types are checked by `check_map_values`, which needs
/// to know which types own an arena.
pub(crate) fn map_ty(k: Ty, v: Ty, key_span: Span, span: Span, t: &mut Tables, diags: &mut Vec<Diagnostic>) -> Ty {
    if k == Ty::Error || v == Ty::Error {
        return Ty::Error;
    }
    if let Ty::Param(p) = k {
        if !t.generic.param(p).key {
            let name = t.param_name(p);
            let msg = format!("`{name}` can't be a map key without the rule `key`: write `[{name}: key]`");
            diags.push(Diagnostic::error(key_span, msg));
            return Ty::Error;
        }
    } else if !(k.is_int() || matches!(k, Ty::Bool | Ty::Str)) {
        let msg = if k.is_float() {
            format!("map keys must be integers, bools or strings; `{}` keys would be unreliable", t.ty_name(k))
        } else {
            format!("map keys must be integers, bools or strings, not `{}`", t.ty_name(k))
        };
        diags.push(Diagnostic::error(key_span, msg));
        return Ty::Error;
    }
    let ty = Ty::Map(t.intern_elem(k), t.intern_elem(v));
    t.map_types.push((ty, span));
    ty
}

/// Reports map types whose values own an arena or a GPU object (once per type).
pub(crate) fn check_map_values(t: &Tables, diags: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    for &(ty, span) in &t.map_types {
        let Ty::Map(_, v) = ty else { continue };
        let v = t.elem(v);
        if t.is_owning(v) && seen.insert(ty) {
            let what = if t.holds_arena(v) { "an arena" } else { "a GPU object" };
            let msg = format!(
                "maps can't hold values that own {what} yet (`{}` has one); keep them in a list and map keys to indexes",
                t.ty_name(v)
            );
            diags.push(Diagnostic::error(span, msg));
        }
    }
}

/// The types a value of `ty` holds directly inside it. An optional holds its
/// payload in place, so `Node?` inside `Node` is as endless as `Node` is; an
/// array, list or map keeps its items elsewhere, so those can hold their own type.
fn contained(t: &Tables, ty: Ty) -> Vec<Ty> {
    match ty {
        // A placeholder is a leaf (R4): what it stands for is checked in each instance.
        Ty::Param(_) => Vec::new(),
        Ty::Optional(e) => vec![t.elem(e)],
        Ty::Struct(s) => t.strukt(s).fields.iter().map(|f| f.ty).collect(),
        Ty::Enum(e) => t.enm(e).variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty)).collect(),
        _ => Vec::new(),
    }
}

/// How to build a recursive type anyway.
fn self_help(name: &str) -> String {
    format!("to make `{name}` recursive, hold a `list[{name}]` or an array `[{name}]` instead of a `{name}` or `{name}?`; they keep their items elsewhere")
}

/// A type can't hold itself, directly or through others: it would be infinitely large. The walk
/// goes through instances (their filled fields), but only declarations and plain structs and
/// enums are reported, never an instance: a generic declaration contains itself when it reaches
/// itself or one of its instances (`struct G[T] { v: G[int]? }`). Declarations that would need
/// endless copies are skipped (already reported; their instances have no fields).
pub(crate) fn check_self_containment(t: &Tables, diags: &mut Vec<Diagnostic>) {
    fn reaches(t: &Tables, from: Ty, target: &dyn Fn(Ty) -> bool, seen: &mut Vec<Ty>) -> bool {
        for inner in contained(t, from) {
            if target(inner) {
                return true;
            }
            if !seen.contains(&inner) {
                seen.push(inner);
                if reaches(t, inner, target, seen) {
                    return true;
                }
            }
        }
        false
    }
    let before = diags.len();
    let mut instance_loops: Option<(Span, String)> = None;
    for (i, s) in t.structs.iter().enumerate() {
        let (ty, id) = (Ty::Struct(StructId(i as u32)), StructId(i as u32));
        if t.generic.endless.contains(&GenericOwner::Struct(id)) {
            continue;
        }
        if let Some(inst) = s.instance {
            if instance_loops.is_none() && reaches(t, ty, &|x| x == ty, &mut Vec::new()) {
                instance_loops = Some((t.strukt(inst.def).span, format!("struct `{}`", t.strukt(inst.def).name)));
            }
            continue;
        }
        let copy_of_self = |x: Ty| x == ty || matches!(x, Ty::Struct(o) if t.strukt(o).instance.is_some_and(|n| n.def == id));
        if reaches(t, ty, &copy_of_self, &mut Vec::new()) {
            diags.push(Diagnostic::error(s.span, format!("struct `{}` contains itself", s.name)).with_help(self_help(&s.name)));
        }
    }
    for (i, e) in t.enums.iter().enumerate() {
        let (ty, id) = (Ty::Enum(EnumId(i as u32)), EnumId(i as u32));
        if t.generic.endless.contains(&GenericOwner::Enum(id)) {
            continue;
        }
        if let Some(inst) = e.instance {
            if instance_loops.is_none() && reaches(t, ty, &|x| x == ty, &mut Vec::new()) {
                instance_loops = Some((t.enm(inst.def).span, format!("enum `{}`", t.enm(inst.def).name)));
            }
            continue;
        }
        let copy_of_self = |x: Ty| x == ty || matches!(x, Ty::Enum(o) if t.enm(o).instance.is_some_and(|n| n.def == id));
        if reaches(t, ty, &copy_of_self, &mut Vec::new()) {
            diags.push(Diagnostic::error(e.span, format!("enum `{}` contains itself", e.name)).with_help(self_help(&e.name)));
        }
    }
    // An instance that holds itself is always held by a reported type above; if none was
    // reported, refuse the program anyway, at the instance's declaration (it would be endless).
    if let (true, Some((span, what))) = (diags.len() == before, instance_loops) {
        let bare = what.split('`').nth(1).unwrap_or_default().to_string();
        diags.push(Diagnostic::error(span, format!("{what} contains itself")).with_help(self_help(&bare)));
    }
}
