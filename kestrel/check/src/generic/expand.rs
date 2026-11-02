//! The expansion checks (G10): a generic that would need endless copies of itself.
//!
//! Both are Go's "instantiation cycle" check (`types2/monomorph.go`): a node per placeholder; each
//! use `G[args]` adds an edge from every placeholder in `args[i]` to `G`'s placeholder `i`, growing
//! unless `args[i]` is exactly that placeholder; a strongly connected component with a growing
//! edge inside is endless.
//! - The type check (`collect` phase 4) looks at the instances written in generic structs' and
//!   enums' fields (`struct Tree[T] { kids: list[Tree[list[T]]] }`) and generic interfaces'
//!   method signatures (`interface Grow[T] { more(self) -> Grow[list[T]] }`).
//! - The function check (the collection pass, before its walk) looks at the `type_args` of calls,
//!   function values and method calls in generic functions and methods (`deeper[T]` calling
//!   `deeper[list[T]]`). Constructions and variants are not edges (P24): growth through them is
//!   the collection walk's cap's to find, and rule calls (`ParamCall`) add nothing either.
//!
//! A cycle is reported once, at its first declaration in source order, and every declaration of
//! the cycle goes into `generic.endless`: for types before the pending instances are filled, so
//! filling them (fields `Error`) ends; for functions the walk then doesn't run (P9).

use std::collections::{HashMap, HashSet, VecDeque};

use kestrel_syntax::ast::Program;
use kestrel_syntax::visit::each_expr;
use kestrel_syntax::Diagnostic;

use crate::generic::graph::Graph;
use crate::generic::{GenericOwner, ParamId};
use crate::tables::{EnumId, FnId, IfaceId, Res, StructId, Tables};
use crate::ty::Ty;

/// A use `G[args]` inside `from`: an instance written in its fields, or a call in its body.
struct Site {
    from: GenericOwner,
    to: GenericOwner,
    args: Vec<Ty>,
}

/// Phase 4 of `collect`: reports generic structs and enums that would need endless copies and
/// marks them `endless`. Runs before `fill_pending`, over declarations' field types only (the
/// pending instances have no fields yet).
pub(crate) fn check_type_expansion(t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let mut graph = Graph::default();
    let mut sites: Vec<Site> = Vec::new();
    for (i, s) in t.structs.iter().enumerate() {
        if s.instance.is_none() && !s.params.is_empty() {
            let from = GenericOwner::Struct(StructId(i as u32));
            for f in &s.fields {
                walk(t, f.ty, from, &mut graph, &mut sites);
            }
        }
    }
    for (i, e) in t.enums.iter().enumerate() {
        if e.instance.is_none() && !e.params.is_empty() {
            let from = GenericOwner::Enum(EnumId(i as u32));
            for f in e.variants.iter().flat_map(|v| &v.fields) {
                walk(t, f.ty, from, &mut graph, &mut sites);
            }
        }
    }
    for (i, info) in t.ifaces.iter().enumerate() {
        if info.instance.is_none() && !info.params.is_empty() {
            let from = GenericOwner::Iface(IfaceId(i as u32));
            for &m in &info.methods {
                let f = t.func(m);
                // `self` is the interface itself, with its own placeholders.
                for ty in f.params.iter().skip(1).map(|p| p.ty).chain([f.ret]) {
                    walk(t, ty, from, &mut graph, &mut sites);
                }
            }
        }
    }
    report(t, &graph, &sites, Verb::Holds, diags);
}

/// The collection pass's first step: reports generic functions and methods whose calls need ever
/// bigger copies (`deeper[T]` calls `deeper[list[T]]`) and marks them `endless`. Returns whether
/// it reported, in which case the walk doesn't run (P9).
pub(crate) fn check_fn_expansion(program: &Program, t: &mut Tables, diags: &mut Vec<Diagnostic>) -> bool {
    let mut graph = Graph::default();
    let mut sites: Vec<Site> = Vec::new();
    for f in 0..t.fns.len() {
        let info = t.func(FnId(f as u32));
        let Some(from) = info.generic else { continue };
        if info.copy.is_some() || info.is_abstract || info.is_native {
            continue;
        }
        let mut ids = Vec::new();
        each_expr(&info.decl(program).body, &mut |e| ids.push(e.id));
        for id in ids {
            let (Some(Res::Fn(callee) | Res::FnValue(callee)), Some(&args)) = (t.res.get(&id).copied(), t.generic.type_args.get(&id)) else {
                continue;
            };
            let Some(to) = t.func(callee).generic else { continue };
            add_site(t, from, to, t.ty_list(args).to_vec(), &mut graph, &mut sites);
        }
    }
    report(t, &graph, &sites, Verb::Calls, diags)
}

/// How a message says one use leads to the next.
#[derive(Clone, Copy)]
enum Verb {
    Holds,
    Calls,
}

/// Reports each growing cycle at its first declaration and marks every declaration in it
/// `endless`; returns whether any was reported.
fn report(t: &mut Tables, graph: &Graph, sites: &[Site], verb: Verb, diags: &mut Vec<Diagnostic>) -> bool {
    let mut reported: HashSet<GenericOwner> = HashSet::new();
    let mut any = false;
    for cycle in graph.growing_cycles() {
        let owners: HashSet<GenericOwner> = cycle.iter().map(|&p| t.generic.param(p).owner).collect();
        let first = *owners.iter().min_by_key(|&&o| (t.owner_item(o), owner_key(o))).expect("a cycle has placeholders");
        if reported.insert(first) {
            let (msg, help) = message(t, first, &cycle, &owners, graph, sites, verb);
            let d = Diagnostic::error(t.owner_span(first), msg);
            diags.push(match help {
                Some(help) => d.with_help(help),
                None => d,
            });
            any = true;
        }
        t.generic.endless.extend(owners);
    }
    any
}

/// Adds the edges of every instance in `ty` (a field type of declaration `from`), arguments included.
fn walk(t: &Tables, ty: Ty, from: GenericOwner, graph: &mut Graph, sites: &mut Vec<Site>) {
    let (to, args) = match ty {
        Ty::Struct(id) => {
            let (def, args) = t.struct_parts(id);
            (GenericOwner::Struct(def), args)
        }
        Ty::Enum(id) | Ty::Variant(id, _) => {
            let (def, args) = t.enum_parts(id);
            (GenericOwner::Enum(def), args)
        }
        Ty::Iface(id) => {
            let (def, args) = t.iface_parts(id);
            (GenericOwner::Iface(def), args)
        }
        Ty::Array(e) | Ty::List(e) | Ty::Optional(e) => return walk(t, t.elem(e), from, graph, sites),
        Ty::Map(k, v) => {
            walk(t, t.elem(k), from, graph, sites);
            return walk(t, t.elem(v), from, graph, sites);
        }
        Ty::Fn(sig) => {
            let sig = t.sig(sig);
            for p in &sig.params {
                walk(t, p.ty, from, graph, sites);
            }
            return walk(t, sig.ret, from, graph, sites);
        }
        _ => return,
    };
    if args.is_empty() {
        return;
    }
    add_site(t, from, to, args.clone(), graph, sites);
    for a in args {
        walk(t, a, from, graph, sites);
    }
}

/// Records the use `to[args]` inside `from`: an edge from each placeholder in `args[i]` to `to`'s
/// placeholder `i`, growing unless `args[i]` is that placeholder itself.
fn add_site(t: &Tables, from: GenericOwner, to: GenericOwner, args: Vec<Ty>, graph: &mut Graph, sites: &mut Vec<Site>) {
    let site = sites.len();
    let targets = t.owner_params(to).to_vec();
    for (&a, &target) in args.iter().zip(&targets) {
        let mut found = Vec::new();
        placeholders(t, a, &mut found);
        for p in found {
            graph.add(p, target, a != Ty::Param(p), site);
        }
    }
    sites.push(Site { from, to, args });
}

/// Every placeholder in `ty` (an instance's through its arguments; a declaration's own).
fn placeholders(t: &Tables, ty: Ty, found: &mut Vec<ParamId>) {
    match ty {
        Ty::Param(p) => {
            if !found.contains(&p) {
                found.push(p);
            }
        }
        Ty::Struct(id) => t.struct_parts(id).1.into_iter().for_each(|a| placeholders(t, a, found)),
        Ty::Enum(id) | Ty::Variant(id, _) => t.enum_parts(id).1.into_iter().for_each(|a| placeholders(t, a, found)),
        Ty::Iface(id) => t.iface_parts(id).1.into_iter().for_each(|a| placeholders(t, a, found)),
        Ty::Array(e) | Ty::List(e) | Ty::Optional(e) => placeholders(t, t.elem(e), found),
        Ty::Map(k, v) => {
            placeholders(t, t.elem(k), found);
            placeholders(t, t.elem(v), found);
        }
        Ty::Fn(sig) => {
            let sig = t.sig(sig);
            for p in &sig.params {
                placeholders(t, p.ty, found);
            }
            placeholders(t, sig.ret, found);
        }
        _ => {}
    }
}

/// A tie-break for `owner_item` (never needed between items, which differ).
fn owner_key(o: GenericOwner) -> (u8, u32) {
    match o {
        GenericOwner::Struct(s) => (0, s.0),
        GenericOwner::Enum(e) => (1, e.0),
        GenericOwner::Fn(f) => (2, f.0),
        GenericOwner::Iface(i) => (3, i.0),
    }
}

/// The error (and, for a type, its help) for a cycle reported at `first`: the chain made by
/// following a cycle of uses from `first` back to it twice (`Tree[T]` holds a `Tree[list[T]]`,
/// which holds a `Tree[list[list[T]]]`; `deeper[T]` calls `deeper[list[T]]`, which calls ...).
fn message(
    t: &Tables,
    first: GenericOwner,
    cycle: &[ParamId],
    owners: &HashSet<GenericOwner>,
    graph: &Graph,
    sites: &[Site],
    verb: Verb,
) -> (String, Option<String>) {
    let name = t.owner_name(first).to_string();
    let own: Vec<String> = t.owner_params(first).iter().map(|&p| t.param_name(p).to_string()).collect();
    let written = |args: &[String]| format!("{name}[{}]", args.join(", "));
    let path = cycle_path(first, cycle, owners, graph, sites);
    let once = compose(t, &path, sites, own.clone());
    let twice = compose(t, &path, sites, once.clone());
    let (own, once, twice) = (written(&own), written(&once), written(&twice));
    // An interface uses the instances in its methods' signatures; a type holds those in its fields.
    let holds = if matches!(first, GenericOwner::Iface(_)) { "uses" } else { "holds" };
    match verb {
        Verb::Holds => (
            format!("`{name}` would need endless copies: `{own}` {holds} a `{once}`, which {holds} a `{twice}`, and so on"),
            Some(format!("use the same placeholders (`{own}`), or types that don't grow")),
        ),
        Verb::Calls => (format!("`{name}` would need endless copies: `{own}` calls `{once}`, which calls `{twice}`, and so on"), None),
    }
}

/// Sites leading from `first` through a growing site of the cycle and back to `first`, each
/// between declarations of the cycle.
fn cycle_path(first: GenericOwner, cycle: &[ParamId], owners: &HashSet<GenericOwner>, graph: &Graph, sites: &[Site]) -> Vec<usize> {
    let grow = graph
        .edges
        .iter()
        .find(|e| e.growing && cycle.contains(&e.from) && cycle.contains(&e.to))
        .expect("a growing cycle has a growing edge")
        .site;
    let mut path = route(first, sites[grow].from, owners, sites);
    path.push(grow);
    path.extend(route(sites[grow].to, first, owners, sites));
    path
}

/// The shortest run of sites from `from` to `to` (empty when they are the same), between
/// declarations of `owners`.
fn route(from: GenericOwner, to: GenericOwner, owners: &HashSet<GenericOwner>, sites: &[Site]) -> Vec<usize> {
    let mut prev: HashMap<GenericOwner, usize> = HashMap::new();
    let mut queue = VecDeque::from([from]);
    let mut seen = HashSet::from([from]);
    while let Some(at) = queue.pop_front() {
        if at == to {
            break;
        }
        for (i, s) in sites.iter().enumerate() {
            if s.from == at && owners.contains(&s.to) && seen.insert(s.to) {
                prev.insert(s.to, i);
                queue.push_back(s.to);
            }
        }
    }
    let mut path = Vec::new();
    let mut at = to;
    while at != from {
        let Some(&i) = prev.get(&at) else { break };
        path.push(i);
        at = sites[i].from;
    }
    path.reverse();
    path
}

/// The arguments `first` is given at the end of `path`, written in terms of `start` (the text for
/// each of `first`'s placeholders).
fn compose(t: &Tables, path: &[usize], sites: &[Site], start: Vec<String>) -> Vec<String> {
    let mut env = start;
    for &i in path {
        let params = t.owner_params(sites[i].from).to_vec();
        env = sites[i].args.iter().map(|&a| show(t, a, &params, &env)).collect();
    }
    env
}

/// `ty` as `ty_name` writes it, with each of `params` written as its text in `env`.
fn show(t: &Tables, ty: Ty, params: &[ParamId], env: &[String]) -> String {
    let all = |tys: Vec<Ty>| tys.into_iter().map(|a| show(t, a, params, env)).collect::<Vec<_>>().join(", ");
    match ty {
        Ty::Param(p) => match params.iter().position(|&q| q == p) {
            Some(i) if i < env.len() => env[i].clone(),
            _ => t.param_name(p).to_string(),
        },
        Ty::Struct(id) => {
            let (def, args) = t.struct_parts(id);
            if args.is_empty() { t.ty_name(ty) } else { format!("{}[{}]", t.strukt(def).name, all(args)) }
        }
        Ty::Enum(id) => {
            let (def, args) = t.enum_parts(id);
            if args.is_empty() { t.ty_name(ty) } else { format!("{}[{}]", t.enm(def).name, all(args)) }
        }
        Ty::Iface(id) => {
            let (def, args) = t.iface_parts(id);
            if args.is_empty() { t.ty_name(ty) } else { format!("{}[{}]", t.iface(def).name, all(args)) }
        }
        Ty::Array(e) => format!("[{}]", show(t, t.elem(e), params, env)),
        Ty::List(e) => format!("list[{}]", show(t, t.elem(e), params, env)),
        Ty::Optional(e) => format!("{}?", show(t, t.elem(e), params, env)),
        Ty::Map(k, v) => format!("map[{}, {}]", show(t, t.elem(k), params, env), show(t, t.elem(v), params, env)),
        Ty::Fn(sig) => {
            let sig = t.sig(sig);
            let ps: Vec<String> =
                sig.params.iter().map(|p| format!("{}{}", if p.is_var { "var " } else { "" }, show(t, p.ty, params, env))).collect();
            let ret = if sig.ret == Ty::Unit { String::new() } else { format!(" -> {}", show(t, sig.ret, params, env)) };
            format!("{}({}){ret}", if sig.is_pure { "pure " } else { "" }, ps.join(", "))
        }
        other => t.ty_name(other),
    }
}
