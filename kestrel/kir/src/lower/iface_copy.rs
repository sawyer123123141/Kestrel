//! The copy function of a method table: `(data: i64) -> i64`, which gives a copied interface value
//! its own concrete value (`Stmt::CopyIface` calls it through the table, with the value's data
//! pointer, and stores the address it returns).
//!
//! It copies the concrete value's bytes into a new block of the current arena
//! (`ExprKind::BlockDup`, sized by the concrete type), points a reference local at the block and
//! gives that value its own copy of every string, list, map, function value and interface value
//! inside it (`fixup`), like any other copy. So a copy shares nothing with its source, whether the
//! source was boxed or borrowed (pointing at a caller's place).
//!
//! An owning type (one holding an `Arena` or a GPU object) can't be copied: the checker keeps such
//! values from being stored as interface values where it knows the concrete type, so only a callee
//! storing a borrowed parameter gets here. Its table's copy function stops the program with the
//! spec's message, which names both types but no line.

use kestrel_check::{IfaceId, Tables, Ty};

use super::helpers::{expr, Helpers};
use super::{kty, layout};
use crate::ir::*;

/// The run-time error of copying owning type `ty` as interface `iface`.
fn owning_message(ty: Ty, iface: IfaceId, t: &Tables) -> String {
    let name = t.ty_name(ty);
    format!(
        "can't copy a `{name}` into a `{}`: it owns {}; pass it as `var` or keep it in a `list[{name}]`",
        t.iface(iface).name,
        t.owns_what(ty)
    )
}

impl Helpers {
    /// The copy function of the table of concrete type `ty` as interface `iface` (see the module's
    /// comment). One per table.
    pub(crate) fn iface_copy_fn(&mut self, ty: Ty, iface: IfaceId, t: &Tables) -> FuncIdx {
        let idx = self.reserve();
        let concrete = kty(ty);
        let (data, copy, value) = (LocalIdx(0), LocalIdx(1), LocalIdx(2));
        let locals = vec![
            Local { name: "data".into(), ty: KTy::I64, kind: LocalKind::Value },
            Local { name: "copy".into(), ty: KTy::I64, kind: LocalKind::Value },
            Local { name: "value".into(), ty: concrete, kind: LocalKind::RefParam },
        ];
        let load = |l: LocalIdx| expr(ExprKind::Load(Place::local(l)), KTy::I64);
        let body = if t.is_owning(ty) {
            vec![Stmt::Fail { message: expr(ExprKind::StrLit(owning_message(ty, iface, t)), KTy::Str), line: 0 }]
        } else {
            let (size, align) = layout::size_align(t, concrete);
            let dup = ExprKind::BlockDup { src: Box::new(load(data)), size, align: align.max(1) };
            let mut body = vec![Stmt::Let(copy, expr(dup, KTy::I64)), Stmt::BindEnv { local: value, env: load(copy), offset: 0 }];
            body.extend(self.fixup(Place::local(value), concrete, t));
            body.push(Stmt::Return(Some(load(copy))));
            body
        };
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_iface_copy_{}", idx.0),
            params: vec![data],
            ret: Some(KTy::I64),
            locals,
            body,
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
