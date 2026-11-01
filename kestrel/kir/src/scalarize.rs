//! Keeps plain structs in registers. After inlining, a local of a plain struct type that nothing
//! needs the address of is split into one `Value` local per leaf (`vec2` into `.x` and `.y`), and
//! every struct store reads all of its leaves before storing any (see `split`). A number that had
//! to live in memory only for a `var` argument that inlining removed goes back to being a `Value`.
//!
//! An enum local whose variants hold only plain fields is split too, into a tag local and one
//! local per leaf of each variant (spec decision 5 of the performance round), when nothing needs
//! its address and it is only ever given variant literals or another split enum local's value, and
//! only read by its tag, its variants' fields, or a copy into another split enum local. Such a
//! local is never stored to or loaded from memory whole, so nothing can see the leaves of a
//! variant it doesn't hold: `checksum` and `to_bytes` of one read its tag, then only the fields
//! of the variant the tag names, and printing it or `first_difference` take it whole, which keeps
//! it in memory.

use std::collections::{HashMap, HashSet};

use crate::ir::*;
use crate::layout::EnumLayout;
use crate::leaves::{enum_leaves, field_path, leaf_name, leaves, Leaf};
use crate::split::{EnumSplit, Rewriter, Split};
use crate::visit::{walk, Node};
use crate::visit_mut::VisitMut;

pub fn scalarize(m: &mut Module) {
    let Module { funcs, structs, enums: layouts, .. } = m;
    for f in funcs.iter_mut().filter(|f| f.native.is_none()) {
        let pinned = pinned(&f.body, &f.locals);
        let mut roots = HashMap::new();
        for i in 0..f.locals.len() {
            let l = LocalIdx(i as u32);
            let local = &f.locals[i];
            if local.kind != LocalKind::Addressed || pinned.contains(&l) {
                continue;
            }
            let Some(all) = leaves(structs, local.ty) else { continue };
            if !local.ty.is_aggregate() {
                f.locals[i].kind = LocalKind::Value;
                continue;
            }
            let (name, ty) = (local.name.clone(), local.ty);
            let locals = all
                .iter()
                .map(|leaf| {
                    f.locals.push(Local { name: leaf_name(structs, &name, ty, leaf), ty: leaf.ty, kind: LocalKind::Value });
                    LocalIdx(f.locals.len() as u32 - 1)
                })
                .collect();
            f.locals[i].kind = LocalKind::Unused;
            roots.insert(l, Split { leaves: all, locals });
        }
        let mut enums = HashMap::new();
        for (l, variants) in enum_roots(structs, layouts, f, &pinned) {
            let name = f.locals[l.0 as usize].name.clone();
            let KTy::Enum(en) = f.locals[l.0 as usize].ty else { unreachable!("an enum root is an enum") };
            let mut new = |name: String, ty: KTy| {
                f.locals.push(Local { name, ty, kind: LocalKind::Value });
                LocalIdx(f.locals.len() as u32 - 1)
            };
            let tag = new(format!("{name}.tag"), KTy::U8);
            let variants = variants
                .into_iter()
                .enumerate()
                .map(|(v, leaves)| {
                    let fields = &layouts[en.0 as usize].variants[v].fields;
                    let locals = leaves.iter().map(|leaf| new(variant_leaf_name(structs, &name, fields, leaf), leaf.ty)).collect();
                    Split { leaves, locals }
                })
                .collect();
            f.locals[l.0 as usize].kind = LocalKind::Unused;
            enums.insert(l, EnumSplit { tag, variants });
        }
        // Even with no split locals, a plain struct value stored to memory is built from locals first.
        let mut rw = Rewriter { structs, locals: &mut f.locals, roots, enums };
        rw.visit_stmts(&mut f.body);
    }
}

/// `name.field.field` for a leaf of a variant (whose fields are `fields`) of an enum local `name`.
fn variant_leaf_name(structs: &[crate::layout::StructLayout], name: &str, fields: &[crate::layout::FieldLayout], leaf: &Leaf) -> String {
    let field = &fields[leaf.path[0] as usize];
    let inner = Leaf { path: leaf.path[1..].to_vec(), ty: leaf.ty };
    leaf_name(structs, &format!("{name}.{}", field.name), field.ty, &inner)
}

/// The enum locals of `f` to split, with each variant's leaves: see the module comment. A local
/// copied into another one is split only when that one is too, so the set is found by dropping
/// locals until every copy is between two kept ones.
fn enum_roots(structs: &[crate::layout::StructLayout], layouts: &[EnumLayout], f: &Func, pinned: &HashSet<LocalIdx>) -> Vec<(LocalIdx, Vec<Vec<Leaf>>)> {
    let mut roots: HashMap<LocalIdx, Vec<Vec<Leaf>>> = HashMap::new();
    for (i, local) in f.locals.iter().enumerate() {
        let l = LocalIdx(i as u32);
        if let (KTy::Enum(en), LocalKind::Addressed, false) = (local.ty, local.kind, pinned.contains(&l)) {
            if let Some(variants) = enum_leaves(structs, &layouts[en.0 as usize]) {
                roots.insert(l, variants);
            }
        }
    }
    loop {
        let keep: HashSet<LocalIdx> = roots.keys().copied().collect();
        let bad: Vec<LocalIdx> = keep.iter().copied().filter(|&l| !enum_uses_ok(&f.body, l, &keep)).collect();
        if bad.is_empty() {
            break;
        }
        for l in bad {
            roots.remove(&l);
        }
    }
    let mut out: Vec<_> = roots.into_iter().collect();
    out.sort_by_key(|(l, _)| *l);
    out
}

/// The values an `if` or a block yields (through nested ones), or `e` itself.
fn yielded<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match &e.kind {
        ExprKind::If { then_val, else_val, .. } => {
            for v in [then_val, else_val].into_iter().flatten() {
                yielded(v, out);
            }
        }
        ExprKind::Seq(_, v) => yielded(v, out),
        _ => out.push(e),
    }
}

/// Whether `e` is a whole read of a local (no path) in `set`.
fn whole_read_of(e: &Expr, set: &HashSet<LocalIdx>) -> Option<LocalIdx> {
    match &e.kind {
        ExprKind::Load(q) if q.path.is_empty() && set.contains(&q.root) => Some(q.root),
        _ => None,
    }
}

/// Whether `body` uses the enum local `l` only as a split one can be used, when the locals in
/// `keep` are split too: every value stored into it whole is a variant literal or a whole read of a
/// kept local, and every place of it is its tag, a field of a variant (read or written), its
/// whole value stored into, or its whole value read to be stored into a kept local.
fn enum_uses_ok(body: &[Stmt], l: LocalIdx, keep: &HashSet<LocalIdx>) -> bool {
    let (mut ok, mut whole, mut accounted) = (true, 0, 0);
    let stored = |to: LocalIdx, e: &Expr, ok: &mut bool, accounted: &mut usize| {
        let mut vals = Vec::new();
        yielded(e, &mut vals);
        if to == l {
            *ok &= vals.iter().all(|v| matches!(v.kind, ExprKind::VariantLit(..)) || whole_read_of(v, keep).is_some());
        }
        if keep.contains(&to) {
            *accounted += vals.iter().filter(|v| whole_read_of(v, keep) == Some(l)).count();
        }
    };
    walk(body, &mut |n| match n {
        Node::Place(p, _) if p.root == l => {
            if p.path.is_empty() {
                whole += 1;
            } else {
                ok &= matches!(p.path[0], Proj::Variant(..)) && field_path(&p.path[1..]).is_some();
            }
        }
        Node::Stmt(Stmt::Let(to, e)) => stored(*to, e, &mut ok, &mut accounted),
        Node::Stmt(Stmt::Assign(p, e)) if p.path.is_empty() => {
            stored(p.root, e, &mut ok, &mut accounted);
            accounted += usize::from(p.root == l);
        }
        Node::Expr(Expr { kind: ExprKind::Tag(p), .. }) if p.root == l && p.path.is_empty() => accounted += 1,
        _ => {}
    });
    ok && whole == accounted
}

/// The locals whose address something needs: passed as `var` (to a call or a `par for`) or lent
/// to an interface value, bound
/// by a loop, or used whole by an operation on memory (moved, dropped, measured, ...). (An enum's
/// tag read doesn't count: `enum_roots` decides about enum locals.)
fn pinned(body: &[Stmt], locals: &[Local]) -> HashSet<LocalIdx> {
    let mut out = HashSet::new();
    let args = |args: &[Arg], out: &mut HashSet<LocalIdx>| {
        for a in args {
            if let Arg::Ref(p) = a {
                out.insert(p.root);
            }
        }
    };
    walk(body, &mut |n| match n {
        Node::Stmt(Stmt::Call(_, a, _)) | Node::Stmt(Stmt::ParFor { args: a, .. }) => args(a, &mut out),
        Node::Expr(Expr { kind: ExprKind::Call(_, a), .. }) => args(a, &mut out),
        // An indirect call takes the address of the function value it calls, and of its `var` arguments.
        Node::Stmt(Stmt::CallIndirect { callee, args: a, .. })
        | Node::Expr(Expr { kind: ExprKind::CallIndirect { callee, args: a, .. }, .. }) => {
            out.insert(callee.root);
            args(a, &mut out);
        }
        // A call through an interface value reads it in memory; a borrowed one takes its place's
        // address (as a `var` argument does).
        Node::Stmt(Stmt::CallVirtual { recv, args: a, .. }) | Node::Expr(Expr { kind: ExprKind::CallVirtual { recv, args: a, .. }, .. }) => {
            out.insert(recv.root);
            args(a, &mut out);
        }
        Node::Expr(Expr { kind: ExprKind::MakeIface { data: IfaceData::Borrowed(p), .. }, .. }) => {
            out.insert(p.root);
        }
        Node::Stmt(
            Stmt::Bind(_, p)
            | Stmt::ListAdd(p, _)
            | Stmt::SetLen(p, _)
            | Stmt::List(_, p, _)
            | Stmt::Dup(p)
            | Stmt::FreeArena(p)
            | Stmt::DropGpu(p)
            | Stmt::PushArena(Some(p), _)
            | Stmt::MapSet(p, ..)
            | Stmt::MapUnlock(p, _)
            | Stmt::CopyFnEnv(p)
            | Stmt::CopyIface(p),
        ) => {
            out.insert(p.root);
        }
        Node::Stmt(Stmt::CopyItems(dst, src)) => {
            out.insert(dst.root);
            out.insert(src.root);
        }
        Node::Expr(Expr { kind: ExprKind::Tag(p), .. }) if !matches!(locals[p.root.0 as usize].ty, KTy::Enum(_)) => {
            out.insert(p.root);
        }
        Node::Expr(Expr { kind: ExprKind::Move(p) | ExprKind::Len(p) | ExprKind::Cap(p) | ExprKind::FnEnv(p) | ExprKind::Map(_, p, _) | ExprKind::ReadBytes(p, _), .. }) => {
            out.insert(p.root);
        }
        _ => {}
    });
    out
}
