//! The rewrite `scalarize` and `promote` share. Every place rooted at a chosen local (a **split
//! root**) becomes the local holding that leaf. Every store of a plain struct value computes all
//! of the value's leaves first. A store into a split root then goes one leaf at a time; a store
//! into memory is one `Assign` of a struct built from those leaves.
//!
//! A split enum local (see `EnumSplit`) is its tag's local and one local per leaf of each variant:
//! its tag reads the tag's local, a variant's field reads that field's locals, and a variant
//! literal stored into it computes every field first, then sets the tag and that variant's locals.

use std::collections::HashMap;

use crate::ir::*;
use crate::layout::StructLayout;
use crate::leaves::{field_path, is_plain_struct, lane_of, leaf_or_lane, leaf_place, leaves, load, rebuild, under, zero, Leaf};
use crate::visit_mut::{walk_expr, walk_place, walk_stmt, VisitMut};

/// A split root's leaves and the local that holds each.
pub(crate) struct Split {
    pub leaves: Vec<Leaf>,
    pub locals: Vec<LocalIdx>,
}

/// A split enum local: the local holding its tag, and each variant's leaves (paths starting with
/// the field's index in the variant) and their locals. The leaves of the variants not stored are
/// never read but by a copy into another split enum local.
pub(crate) struct EnumSplit {
    pub tag: LocalIdx,
    pub variants: Vec<Split>,
}

pub(crate) struct Rewriter<'a> {
    pub structs: &'a [StructLayout],
    pub locals: &'a mut Vec<Local>,
    pub roots: HashMap<LocalIdx, Split>,
    pub enums: HashMap<LocalIdx, EnumSplit>,
}

impl Rewriter<'_> {
    pub(crate) fn temp(&mut self, name: &str, ty: KTy, kind: LocalKind) -> LocalIdx {
        self.locals.push(Local { name: name.to_string(), ty, kind });
        LocalIdx(self.locals.len() as u32 - 1)
    }

    /// Whether `l` is a split root: a plain struct's, or an enum's.
    fn is_split(&self, l: LocalIdx) -> bool {
        self.roots.contains_key(&l) || self.enums.contains_key(&l)
    }

    /// The leaves `p` (rooted at a split root) is among, and its path into them: a split struct's
    /// leaves, or the leaves of the variant a split enum's place names.
    fn split_of(&self, p: &Place) -> Option<(&Split, Vec<u32>)> {
        if let Some(split) = self.roots.get(&p.root) {
            return Some((split, field_path(&p.path).expect("a split root's places are field paths")));
        }
        let split = self.enums.get(&p.root)?;
        let Some(&Proj::Variant(v, f)) = p.path.first() else { unreachable!("a split enum's places are its variants' fields") };
        let mut path = vec![f];
        path.extend(field_path(&p.path[1..]).expect("a variant field's places are field paths"));
        Some((&split.variants[v as usize], path))
    }

    /// The place of the leaf at `p`, when `p` is rooted at a split root: the local holding it, or
    /// a lane of that local when `p` is a lane of a vector leaf.
    fn leaf_local(&self, p: &Place) -> Option<Place> {
        let (split, path) = self.split_of(p)?;
        let k = split.leaves.iter().position(|l| l.path == path).expect("a scalar place is a leaf");
        Some(leaf_or_lane(split.locals[k], lane_of(&p.path)))
    }

    /// The value at `p` (rooted at a split root): its leaf local (or a lane of it), or a
    /// `StructLit` of them.
    fn read(&self, p: &Place, ty: KTy, line: u32) -> Expr {
        if lane_of(&p.path).is_some() {
            let at = self.leaf_local(p).expect("a split root's place");
            return Expr { kind: ExprKind::Load(at), ty, line };
        }
        let (split, path) = self.split_of(p).expect("a split root's place");
        let range = under(&split.leaves, &path);
        let mut vals = range.map(|k| load(split.locals[k], split.leaves[k].ty, line));
        rebuild(self.structs, ty, &mut vals, line)
    }

    /// Whether `e` is a plain struct value that is copied as a whole between two places in
    /// memory, the one struct store that isn't split.
    fn memory_copy(&self, e: &Expr) -> bool {
        matches!(&e.kind, ExprKind::Load(q) if !self.is_split(q.root))
    }

    /// `e` in a fresh local, unless it is a constant: the result can be read later, after other
    /// code has run, and still give `e`'s value.
    fn hold(&mut self, mut e: Expr, out: &mut Vec<Stmt>) -> Expr {
        self.visit_expr(&mut e);
        if matches!(e.kind, ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_)) {
            return e;
        }
        let (ty, line) = (e.ty, e.line);
        let t = self.temp("leaf", ty, LocalKind::Value);
        out.push(Stmt::Let(t, e));
        load(t, ty, line)
    }

    /// `p` with every index expression that isn't a constant or a local evaluated now, into a
    /// local, so the place can be used several times and still mean one element.
    fn stable(&mut self, mut p: Place, out: &mut Vec<Stmt>) -> Place {
        for at in p.path.iter_mut().filter_map(Proj::at_mut) {
            let simple = match &at.kind {
                ExprKind::Int(_) => true,
                ExprKind::Load(q) => q.path.is_empty() && self.locals[q.root.0 as usize].kind == LocalKind::Value,
                _ => false,
            };
            if !simple {
                let (ty, line) = (at.ty, at.line);
                let t = self.temp("index", ty, LocalKind::Value);
                let v = std::mem::replace(at, load(t, ty, line));
                out.push(Stmt::Let(t, v));
            }
        }
        p
    }

    /// The leaves of `e`, a plain struct value, in leaf order. The code computing them goes to
    /// `out` in the order codegen evaluates `e`; each leaf is a constant or a load of a local that
    /// holds it.
    pub(crate) fn split(&mut self, e: Expr, out: &mut Vec<Stmt>) -> Vec<Expr> {
        let Expr { kind, ty, line } = e;
        match kind {
            ExprKind::StructLit(_, fields) => {
                let mut vals = Vec::new();
                for f in fields {
                    if f.ty.is_aggregate() {
                        vals.extend(self.split(f, out));
                    } else {
                        vals.push(self.hold(f, out));
                    }
                }
                vals
            }
            ExprKind::Load(mut p) => {
                self.visit_place(&mut p);
                let all = leaves(self.structs, ty).expect("a plain struct");
                if let Some((split, path)) = self.split_of(&p) {
                    let range = under(&split.leaves, &path);
                    let reads: Vec<Expr> = range.map(|k| load(split.locals[k], split.leaves[k].ty, line)).collect();
                    return reads.into_iter().map(|r| self.hold(r, out)).collect();
                }
                let p = self.stable(p, out);
                all.iter().map(|l| self.hold(Expr { kind: ExprKind::Load(leaf_place(&p, l)), ty: l.ty, line }, out)).collect()
            }
            ExprKind::FieldOf(base, i) if is_plain_struct(self.structs, base.ty) => {
                let base_ty = base.ty;
                let vals = self.split(*base, out);
                let all = leaves(self.structs, base_ty).expect("a plain struct");
                vals[under(&all, &[i])].to_vec()
            }
            ExprKind::Seq(stmts, value) => {
                let mut stmts = stmts;
                self.visit_stmts(&mut stmts);
                out.extend(stmts);
                self.split(*value, out)
            }
            ExprKind::If { mut cond, then_body, then_val, else_body, else_val } => {
                self.visit_expr(&mut cond);
                let all = leaves(self.structs, ty).expect("a plain struct");
                let temps: Vec<LocalIdx> = all
                    .iter()
                    .map(|l| {
                        let t = self.temp("leaf", l.ty, LocalKind::Value);
                        out.push(Stmt::Let(t, zero(l.ty, line)));
                        t
                    })
                    .collect();
                let then_body = self.branch(then_body, then_val, &temps);
                let else_body = self.branch(else_body, else_val, &temps);
                out.push(Stmt::If { cond: *cond, then_body, else_body });
                temps.iter().zip(&all).map(|(&t, l)| load(t, l.ty, line)).collect()
            }
            kind => {
                // A call's result, or any other value built in memory: keep it in a temporary.
                let mut e = Expr { kind, ty, line };
                self.visit_expr(&mut e);
                let t = self.temp("value", ty, LocalKind::Addressed);
                out.push(Stmt::Let(t, e));
                let at = Place::local(t);
                let all = leaves(self.structs, ty).expect("a plain struct");
                // Read each leaf into a local now, so every leaf is a local and a store of the
                // result into memory is built from locals.
                all.iter().map(|l| self.hold(Expr { kind: ExprKind::Load(leaf_place(&at, l)), ty: l.ty, line }, out)).collect()
            }
        }
    }

    /// One branch of a struct-valued `if`: its statements, then its value's leaves stored in `temps`.
    /// A branch without a value never finishes, so it stores nothing.
    fn branch(&mut self, body: Vec<Stmt>, val: Option<Box<Expr>>, temps: &[LocalIdx]) -> Vec<Stmt> {
        let mut body = body;
        self.visit_stmts(&mut body);
        if let Some(v) = val {
            let vals = self.split(*v, &mut body);
            for (&t, v) in temps.iter().zip(vals) {
                body.push(Stmt::Assign(Place::local(t), v));
            }
        }
        body
    }

    /// `dest = e`, rewritten. All of `e`'s leaves are computed first, then:
    /// - a leaf of a split root is stored in its local, and a plain struct value into a split root
    ///   is stored one leaf at a time;
    /// - a plain struct value into memory is one `Assign` of a `StructLit` of those leaves, which
    ///   codegen stores with a single place computation (one bounds check).
    ///
    /// `dest`'s index expressions run after `e`, as codegen orders a store.
    fn store(&mut self, dest: Place, e: Expr, out: &mut Vec<Stmt>) {
        let mut dest = dest;
        if !e.ty.is_aggregate() {
            let mut e = e;
            self.visit_expr(&mut e);
            self.visit_place(&mut dest);
            let dest = match self.leaf_local(&dest) {
                Some(at) => at,
                None => dest,
            };
            out.push(Stmt::Assign(dest, e));
            return;
        }
        let (ty, line) = (e.ty, e.line);
        let vals = self.split(e, out);
        self.visit_place(&mut dest);
        if !self.is_split(dest.root) {
            // Every leaf is a constant or a local by now, so this one store reads no memory. Codegen
            // finds `dest` once and runs its index expressions here, after the leaves are computed.
            out.push(Stmt::Assign(dest, rebuild(self.structs, ty, &mut vals.into_iter(), line)));
            return;
        }
        // A split root's places are field paths, with no index expressions to hold.
        for (leaf, v) in leaves(self.structs, ty).expect("a plain struct").iter().zip(vals) {
            let p = leaf_place(&dest, leaf);
            let p = match self.leaf_local(&p) {
                Some(at) => at,
                None => p,
            };
            out.push(Stmt::Assign(p, v));
        }
    }

    /// `l = e` for a split enum local `l`: `e` is a variant literal, a split enum local, or an `if`
    /// or a block whose values are those (the store goes into each arm).
    fn enum_store(&mut self, l: LocalIdx, e: Expr, out: &mut Vec<Stmt>) {
        let Expr { kind, line, .. } = e;
        match kind {
            ExprKind::VariantLit(_, v, fields) => {
                let mut vals = Vec::new();
                for f in fields {
                    if f.ty.is_aggregate() {
                        vals.extend(self.split(f, out));
                    } else {
                        vals.push(self.hold(f, out));
                    }
                }
                let split = &self.enums[&l];
                out.push(Stmt::Assign(Place::local(split.tag), Expr { kind: ExprKind::Int(i64::from(v)), ty: KTy::U8, line }));
                for (&local, val) in split.variants[v as usize].locals.iter().zip(vals) {
                    out.push(Stmt::Assign(Place::local(local), val));
                }
            }
            ExprKind::Load(q) if q.path.is_empty() => {
                if q.root == l {
                    return;
                }
                let (to, from) = (&self.enums[&l], &self.enums[&q.root]);
                let mut pairs = vec![(to.tag, from.tag, KTy::U8)];
                for (a, b) in to.variants.iter().zip(&from.variants) {
                    pairs.extend(a.locals.iter().zip(&b.locals).zip(&a.leaves).map(|((&t, &f), leaf)| (t, f, leaf.ty)));
                }
                out.extend(pairs.into_iter().map(|(t, f, ty)| Stmt::Assign(Place::local(t), load(f, ty, line))));
            }
            ExprKind::If { mut cond, then_body, then_val, else_body, else_val } => {
                self.visit_expr(&mut cond);
                let then_body = self.enum_branch(l, then_body, then_val);
                let else_body = self.enum_branch(l, else_body, else_val);
                out.push(Stmt::If { cond: *cond, then_body, else_body });
            }
            ExprKind::Seq(stmts, value) => {
                let mut stmts = stmts;
                self.visit_stmts(&mut stmts);
                out.extend(stmts);
                self.enum_store(l, *value, out);
            }
            _ => unreachable!("an enum local is split only when each value stored into it is one `enum_store` takes"),
        }
    }

    /// One branch of an `if` whose value goes into the split enum local `l`. A branch without a
    /// value never finishes, so it stores nothing.
    fn enum_branch(&mut self, l: LocalIdx, body: Vec<Stmt>, val: Option<Box<Expr>>) -> Vec<Stmt> {
        let mut body = body;
        self.visit_stmts(&mut body);
        if let Some(v) = val {
            self.enum_store(l, *v, &mut body);
        }
        body
    }

    fn stmt(&mut self, s: Stmt, out: &mut Vec<Stmt>) {
        match s {
            Stmt::Let(l, e) if self.enums.contains_key(&l) => self.enum_store(l, e, out),
            Stmt::Assign(p, e) if p.path.is_empty() && self.enums.contains_key(&p.root) => self.enum_store(p.root, e, out),
            Stmt::Let(l, e) if self.roots.contains_key(&l) => self.store(Place::local(l), e, out),
            Stmt::Assign(p, e) if self.is_split(p.root) => self.store(p, e, out),
            Stmt::Assign(p, e) if is_plain_struct(self.structs, e.ty) && !self.memory_copy(&e) => self.store(p, e, out),
            mut s => {
                walk_stmt(self, &mut s);
                out.push(s);
            }
        }
    }
}

impl VisitMut for Rewriter<'_> {
    fn visit_stmts(&mut self, list: &mut Vec<Stmt>) {
        let mut out = Vec::with_capacity(list.len());
        for s in std::mem::take(list) {
            self.stmt(s, &mut out);
        }
        *list = out;
    }

    fn visit_expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Tag(p) if self.enums.contains_key(&p.root) => {
                *e = load(self.enums[&p.root].tag, KTy::U8, e.line);
            }
            ExprKind::Load(p) if self.is_split(p.root) => {
                walk_place(self, p);
                *e = self.read(p, e.ty, e.line);
            }
            ExprKind::FieldOf(base, i) if is_plain_struct(self.structs, base.ty) => {
                let (i, line, ty) = (*i, e.line, e.ty);
                let base = std::mem::replace(&mut **base, zero(KTy::Bool, line));
                let base_ty = base.ty;
                let mut pre = Vec::new();
                let vals = self.split(base, &mut pre);
                let all = leaves(self.structs, base_ty).expect("a plain struct");
                let value = rebuild(self.structs, ty, &mut vals[under(&all, &[i])].iter().cloned(), line);
                *e = if pre.is_empty() { value } else { Expr { kind: ExprKind::Seq(pre, Box::new(value)), ty, line } };
            }
            _ => walk_expr(self, e),
        }
    }
}
