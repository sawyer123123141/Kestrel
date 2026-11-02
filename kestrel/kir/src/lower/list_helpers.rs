//! The generated search behind `xs.has(v)` and `xs.find(v)` (see `helpers`).

use kestrel_check::Tables;

use super::helpers::{elem_of, expr, with, Helpers};
use crate::ir::*;

impl Helpers {
    /// `find(xs, needle) -> int?`: the index of the first item `== needle`, or `none`. One per
    /// list or array type. Items compare as `==` does (see `eq`): numbers, bools and
    /// `vec4`/`ivec4` by value, strings by their text, structs (the vectors among them) field by
    /// field and enums by tag, then fields. `opt` is `int?`.
    pub(crate) fn find_fn(&mut self, ty: KTy, opt: KTy, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.find.get(&ty) {
            return f;
        }
        let idx = self.reserve();
        self.find.insert(ty, idx);
        let elem = elem_of(ty, t);
        // Local 0 is the list, local 1 the needle (a string is passed by pointer), local 2 the index.
        let (xs, needle, i) = (Place::local(LocalIdx(0)), Place::local(LocalIdx(1)), LocalIdx(2));
        let at = expr(ExprKind::Load(Place::local(i)), KTy::I64);
        // The loop never changes the list, so `i < xs.len` holds throughout.
        let item = with(&xs, Proj::Index { at: at.clone(), check: IndexCheck::Known });
        let same = self.places_equal(&item, &needle, elem, t);
        let found = expr(ExprKind::Cast(Box::new(at)), KTy::I32);
        let hit = Stmt::Return(Some(expr(ExprKind::Some(Box::new(found)), opt)));
        let search = Stmt::ForRange {
            var: i,
            start: expr(ExprKind::Int(0), KTy::I64),
            end: expr(ExprKind::Len(xs), KTy::I64),
            body: vec![Stmt::If { cond: same, then_body: vec![hit], else_body: Vec::new() }],
        };
        let needle_kind = if elem.is_aggregate() { LocalKind::RefParam } else { LocalKind::Value };
        let locals = vec![
            Local { name: "xs".into(), ty, kind: LocalKind::RefParam },
            Local { name: "needle".into(), ty: elem, kind: needle_kind },
            Local { name: "i".into(), ty: KTy::I64, kind: LocalKind::Value },
        ];
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_find_{}", idx.0),
            params: vec![LocalIdx(0), LocalIdx(1)],
            ret: Some(opt),
            locals,
            body: vec![search, Stmt::Return(Some(expr(ExprKind::NoneLit, opt)))],
            is_pure: true,
            io: false,
            mutates_params: false,
            exported: false,
            lines: (0, 0),
            native: None,
        };
        idx
    }
}
