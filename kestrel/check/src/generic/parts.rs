//! The size cap at interning (P42): a type's parts, and the instance whose type list passes
//! `MAX_PARTS`.
//!
//! P41 stops the collection walk before a type grows past 1,000 parts, but plain code can grow
//! one too: each line of `let x2 = Pair(a: x1, b: x1)` doubles it. Instance names are built when
//! an instance is interned, so such a name would double with each line and run the checker (and
//! the language server, on every edit) out of memory. An instance whose type list passes the cap
//! is interned with a short name (`Pair[...]`), and the checker reports it once, at the expression
//! that interned it, which gets `Error`.

use kestrel_syntax::{Diagnostic, Span};

use crate::generic::{GenericOwner, TyListId, MAX_PARTS};
use crate::tables::Tables;
use crate::ty::Ty;

impl Tables {
    /// How many parts `ty` has: 1 plus its parts' (an instance's type arguments, an element, a
    /// map's key and value, a function's parameters and result). Saturating and memoised.
    pub(crate) fn type_parts(&mut self, ty: Ty) -> u32 {
        if let Some(&n) = self.generic.parts.get(&ty) {
            return n;
        }
        let parts = match ty {
            Ty::Struct(s) => self.strukt(s).instance.map(|i| self.ty_list(i.args).to_vec()).unwrap_or_default(),
            Ty::Enum(e) | Ty::Variant(e, _) => self.enm(e).instance.map(|i| self.ty_list(i.args).to_vec()).unwrap_or_default(),
            Ty::Iface(i) => self.iface(i).instance.map(|i| self.ty_list(i.args).to_vec()).unwrap_or_default(),
            Ty::Array(e) | Ty::List(e) | Ty::Optional(e) => vec![self.elem(e)],
            Ty::Map(k, v) => vec![self.elem(k), self.elem(v)],
            Ty::Fn(sig) => {
                let sig = self.sig(sig);
                sig.params.iter().map(|p| p.ty).chain([sig.ret]).collect()
            }
            _ => Vec::new(),
        };
        let n = parts.into_iter().fold(1u32, |n, p| n.saturating_add(self.type_parts(p)));
        self.generic.parts.insert(ty, n);
        n
    }

    /// Whether the type list `args` of an instance of `owner` passes the cap; the first such
    /// instance since the last report is recorded for the checker to report (P42).
    pub(crate) fn too_big(&mut self, owner: GenericOwner, args: TyListId) -> bool {
        let tys = self.ty_list(args).to_vec();
        let total = tys.into_iter().fold(0u32, |n, a| n.saturating_add(self.type_parts(a)));
        let big = total > MAX_PARTS;
        if big && self.generic.grew.is_none() {
            self.generic.grew = Some(owner);
        }
        big
    }

    /// The error for an instance that passed the cap since the last call, at `at` (the
    /// expression that interned it), or at its generic's declaration when no expression did.
    pub(crate) fn take_grown(&mut self, at: Option<Span>) -> Option<Diagnostic> {
        let owner = self.generic.grew.take()?;
        let name = self.owner_name(owner);
        let msg = format!("making copies of `{name}` doesn't end: each use makes a bigger `{name}` (stopped after a type grew to 1,000 parts)");
        Some(Diagnostic::error(at.unwrap_or_else(|| self.owner_span(owner)), msg))
    }
}
