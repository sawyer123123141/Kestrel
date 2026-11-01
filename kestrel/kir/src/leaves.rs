//! Plain types: numbers, bools, and structs made only of those and of other plain structs. A
//! plain type's leaves are the scalars at its bottom, found through nested structs, in field
//! order; `scalarize` and `promote` keep each leaf in a local of its own.

use std::ops::Range;

use crate::ir::*;
use crate::layout::{EnumLayout, StructLayout};

/// Plain structs with more leaves than this stay in memory.
pub const MAX_LEAVES: usize = 16;

/// One scalar at the bottom of a plain type.
#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    /// Field indexes from the top of the type down to the scalar (empty for a scalar type).
    pub path: Vec<u32>,
    pub ty: KTy,
}

/// The leaves of `ty` in field order, or `None` when it isn't plain: it holds something other
/// than numbers and bools, has no leaves, or has more than `MAX_LEAVES`.
pub fn leaves(structs: &[StructLayout], ty: KTy) -> Option<Vec<Leaf>> {
    let mut out = Vec::new();
    collect(structs, ty, &mut Vec::new(), &mut out)?;
    (!out.is_empty() && out.len() <= MAX_LEAVES).then_some(out)
}

fn collect(structs: &[StructLayout], ty: KTy, path: &mut Vec<u32>, out: &mut Vec<Leaf>) -> Option<()> {
    match ty {
        // A vector is one leaf: it stays one SIMD value, never four lanes.
        KTy::Bool | KTy::U8 | KTy::I32 | KTy::I64 | KTy::F32 | KTy::F64 | KTy::F32X4 | KTy::I32X4 => out.push(Leaf { path: path.clone(), ty }),
        KTy::Struct(s) => {
            for (i, f) in structs[s.0 as usize].fields.iter().enumerate() {
                path.push(i as u32);
                collect(structs, f.ty, path, out)?;
                path.pop();
                if out.len() > MAX_LEAVES {
                    return None;
                }
            }
        }
        _ => return None,
    }
    Some(())
}

/// The leaves of each variant of an enum whose variants hold only plain fields, by variant: each
/// leaf's path starts with its field's index in the variant. `None` when a field isn't plain, or
/// when the tag and the leaves are more than `MAX_LEAVES` together.
pub fn enum_leaves(structs: &[StructLayout], en: &EnumLayout) -> Option<Vec<Vec<Leaf>>> {
    let mut out = Vec::with_capacity(en.variants.len());
    let mut count = 1;
    for v in &en.variants {
        let mut leaves = Vec::new();
        for (i, f) in v.fields.iter().enumerate() {
            collect(structs, f.ty, &mut vec![i as u32], &mut leaves)?;
        }
        count += leaves.len();
        out.push(leaves);
    }
    (count <= MAX_LEAVES).then_some(out)
}

/// Whether `ty` is a plain struct (a plain type that isn't a scalar).
pub fn is_plain_struct(structs: &[StructLayout], ty: KTy) -> bool {
    matches!(ty, KTy::Struct(_)) && leaves(structs, ty).is_some()
}

/// The field indexes of a path made only of `Proj::Field` steps (every path inside a plain
/// type), or `None` for any other step. A trailing `Proj::Lane` (a lane of a vector leaf) is
/// left out: it stays on whatever place replaces the leaf's (see `lane_of`).
pub fn field_path(path: &[Proj]) -> Option<Vec<u32>> {
    let fields = match path.last() {
        Some(Proj::Lane(_)) => &path[..path.len() - 1],
        _ => path,
    };
    fields.iter().map(|step| if let Proj::Field(i) = step { Some(*i) } else { None }).collect()
}

/// The lane a path ends in, if it ends in one.
pub fn lane_of(path: &[Proj]) -> Option<u8> {
    match path.last() {
        Some(Proj::Lane(i)) => Some(*i),
        _ => None,
    }
}

/// The place of the leaf local `l` standing for a place whose path ended in `lane` (if any): the
/// local, or its lane.
pub fn leaf_or_lane(l: LocalIdx, lane: Option<u8>) -> Place {
    Place { root: l, path: lane.map(Proj::Lane).into_iter().collect() }
}

/// The leaves under `prefix` (a field path into the type), which are next to each other.
pub fn under(all: &[Leaf], prefix: &[u32]) -> Range<usize> {
    let start = all.iter().position(|l| l.path.starts_with(prefix)).expect("a field path into the type");
    let len = all[start..].iter().take_while(|l| l.path.starts_with(prefix)).count();
    start..start + len
}

/// A place's path extended by a leaf's field steps.
pub fn leaf_place(base: &Place, leaf: &Leaf) -> Place {
    let mut p = base.clone();
    p.path.extend(leaf.path.iter().map(|&i| Proj::Field(i)));
    p
}

/// The value of type `ty` built from the leaf values `vals` (in leaf order): the scalar itself,
/// or a `StructLit` tree.
pub fn rebuild(structs: &[StructLayout], ty: KTy, vals: &mut impl Iterator<Item = Expr>, line: u32) -> Expr {
    match ty {
        KTy::Struct(s) => {
            let fields = structs[s.0 as usize].fields.iter().map(|f| rebuild(structs, f.ty, vals, line)).collect();
            Expr { kind: ExprKind::StructLit(s, fields), ty, line }
        }
        _ => vals.next().expect("a value for every leaf"),
    }
}

/// `name.field.field` for a leaf of a local named `name` of type `ty`.
pub fn leaf_name(structs: &[StructLayout], name: &str, ty: KTy, leaf: &Leaf) -> String {
    let mut out = name.to_string();
    let mut ty = ty;
    for &i in &leaf.path {
        let KTy::Struct(s) = ty else { unreachable!("a leaf path goes through structs") };
        let f = &structs[s.0 as usize].fields[i as usize];
        out.push('.');
        out.push_str(&f.name);
        ty = f.ty;
    }
    out
}

/// A zero of scalar type `ty`; a vector's is a `Splat` of its lane's zero.
pub fn zero(ty: KTy, line: u32) -> Expr {
    let kind = match ty {
        KTy::F32X4 | KTy::I32X4 => ExprKind::Splat(Box::new(zero(ty.lane(), line))),
        KTy::F32 | KTy::F64 => ExprKind::Float(0.0),
        KTy::Bool => ExprKind::Bool(false),
        _ => ExprKind::Int(0),
    };
    Expr { kind, ty, line }
}

/// A load of a local (no path).
pub fn load(l: LocalIdx, ty: KTy, line: u32) -> Expr {
    Expr { kind: ExprKind::Load(Place::local(l)), ty, line }
}
