//! The bindings of inference (pick 2, G7): what each of a callee's placeholders has been worked
//! out to so far, and the one-sided matching that adds to them. The steps that check arguments
//! and report are in `body/infer.rs`.
//!
//! Matching is one-sided: only the callee's own placeholders (those of `owner`) are unknowns.
//! Types on the caller's side are opaque, even when the caller is the same generic (a recursive
//! `walk[T]` binds the callee's `T` to the caller's `T`).

use crate::generic::{GenericOwner, ParamId, Subst, TyListId};
use crate::tables::Tables;
use crate::ty::Ty;

/// What a placeholder was worked out to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Binding {
    pub(crate) ty: Ty,
    /// Met through a `var` argument, inside a type, or by a closure's result (or given by the
    /// expected type): later values must convert to it; it never widens.
    pub(crate) exact: bool,
    /// The argument (by index, as written) that decided it; `None` for the expected type.
    pub(crate) from: Option<usize>,
}

/// Two meetings of one placeholder that can't both hold: the binding so far and the new one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Conflict {
    pub(crate) param: ParamId,
    pub(crate) old: Binding,
    pub(crate) new: Binding,
}

/// The bindings of one call's placeholders, by index.
pub(crate) struct Bindings {
    owner: GenericOwner,
    pub(crate) params: Vec<ParamId>,
    pub(crate) slots: Vec<Option<Binding>>,
}

impl Bindings {
    pub(crate) fn new(t: &Tables, owner: GenericOwner) -> Bindings {
        let params = t.owner_params(owner).to_vec();
        let slots = vec![None; params.len()];
        Bindings { owner, params, slots }
    }

    /// The index of `p` when it is one of the callee's placeholders.
    fn index(&self, t: &Tables, p: ParamId) -> Option<usize> {
        let info = t.generic.param(p);
        (info.owner == self.owner).then_some(info.index as usize)
    }

    /// The first placeholder (in declaration order) nothing has decided.
    pub(crate) fn first_open(&self) -> Option<usize> {
        self.slots.iter().position(Option::is_none)
    }

    /// Whether every callee placeholder in `ty` (in the callee's terms) is decided.
    pub(crate) fn known(&self, t: &Tables, ty: Ty) -> bool {
        match ty {
            Ty::Param(p) => self.index(t, p).is_none_or(|i| self.slots[i].is_some()),
            Ty::Array(e) | Ty::List(e) | Ty::Optional(e) => self.known(t, t.elem(e)),
            Ty::Map(k, v) => self.known(t, t.elem(k)) && self.known(t, t.elem(v)),
            Ty::Fn(sig) => {
                let sig = t.sig(sig);
                sig.params.iter().all(|p| self.known(t, p.ty)) && self.known(t, sig.ret)
            }
            Ty::Struct(s) => t.struct_parts(s).1.into_iter().all(|a| self.known(t, a)),
            Ty::Enum(e) | Ty::Variant(e, _) => t.enum_parts(e).1.into_iter().all(|a| self.known(t, a)),
            Ty::Iface(i) => t.iface_parts(i).1.into_iter().all(|a| self.known(t, a)),
            _ => true,
        }
    }

    /// `ty` (in the callee's terms, `known`) in the caller's terms. Substitution is applied once.
    pub(crate) fn fill_in(&self, t: &mut Tables, ty: Ty) -> Ty {
        debug_assert!(self.known(t, ty), "only a known type is filled in");
        // The open placeholders aren't in `ty`, so what stands for them doesn't matter.
        let tys = self.slots.iter().map(|b| b.map_or(Ty::Error, |b| b.ty)).collect();
        let args = t.intern_ty_list(tys);
        t.subst(ty, Subst { owner: self.owner, args })
    }

    /// The decided types, once every placeholder is.
    pub(crate) fn list(&self, t: &mut Tables) -> TyListId {
        let tys = self.slots.iter().map(|b| b.expect("every placeholder is decided").ty).collect();
        t.intern_ty_list(tys)
    }

    /// Matches the parameter type `param` (the callee's terms) against the value type `value`
    /// (the caller's), binding the callee's placeholders. At the top a placeholder meets loosely
    /// unless `exact` (a `var` argument); the parts inside a type always meet exactly. A value
    /// for `T?` that isn't optional meets `T`.
    pub(crate) fn meet(&mut self, t: &Tables, param: Ty, value: Ty, exact: bool, from: Option<usize>) -> Result<(), Conflict> {
        self.walk(t, param, value, exact, from, false)
    }

    /// Step 2: matches the callee's result type `ret` against the expected type, filling only the
    /// placeholders still open; it never changes a decided one.
    pub(crate) fn fill(&mut self, t: &Tables, ret: Ty, expected: Ty) {
        // A value goes where its optional is expected: `T` then stands for what's inside.
        let expected = match (ret, expected) {
            (Ty::Optional(_), _) => expected,
            (_, Ty::Optional(inner)) => t.elem(inner),
            _ => expected,
        };
        let filled = self.walk(t, ret, expected, true, None, true);
        debug_assert!(filled.is_ok(), "filling never conflicts");
    }

    fn walk(&mut self, t: &Tables, param: Ty, value: Ty, exact: bool, from: Option<usize>, fill: bool) -> Result<(), Conflict> {
        let parts = |a: &[Ty], b: &[Ty], this: &mut Bindings| -> Result<(), Conflict> {
            for (&p, &v) in a.iter().zip(b) {
                this.walk(t, p, v, true, from, fill)?;
            }
            Ok(())
        };
        match (param, value) {
            (Ty::Param(p), _) => match self.index(t, p) {
                Some(i) => self.bind(i, value, exact, from, fill),
                None => Ok(()),
            },
            // A list goes where an array is expected only as a native's argument, and the
            // mismatch is reported when the argument is finished; the parts still decide.
            (Ty::Array(e) | Ty::List(e), Ty::Array(f) | Ty::List(f)) => parts(&[t.elem(e)], &[t.elem(f)], self),
            (Ty::Optional(e), Ty::Optional(f)) => parts(&[t.elem(e)], &[t.elem(f)], self),
            (Ty::Optional(e), _) if !fill => self.walk(t, t.elem(e), value, exact, from, fill),
            (Ty::Map(k, v), Ty::Map(k2, v2)) => parts(&[t.elem(k), t.elem(v)], &[t.elem(k2), t.elem(v2)], self),
            (Ty::Fn(a), Ty::Fn(b)) => {
                let (a, b) = (t.sig(a), t.sig(b));
                if a.params.len() != b.params.len() {
                    return Ok(());
                }
                let mut want: Vec<Ty> = a.params.iter().map(|p| p.ty).collect();
                let mut got: Vec<Ty> = b.params.iter().map(|p| p.ty).collect();
                want.push(a.ret);
                got.push(b.ret);
                parts(&want, &got, self)
            }
            (Ty::Struct(a), Ty::Struct(b)) => {
                let ((da, aa), (db, ab)) = (t.struct_parts(a), t.struct_parts(b));
                if da != db || aa.is_empty() {
                    return Ok(());
                }
                parts(&aa, &ab, self)
            }
            (Ty::Enum(a), Ty::Enum(b) | Ty::Variant(b, _)) => {
                let ((da, aa), (db, ab)) = (t.enum_parts(a), t.enum_parts(b));
                if da != db || aa.is_empty() {
                    return Ok(());
                }
                parts(&aa, &ab, self)
            }
            // An interface value for `Handler[T]`: its instance's types decide (decision 18 reads
            // them from a struct's or enum's list).
            (Ty::Iface(a), Ty::Iface(b)) => {
                let ((da, aa), (db, ab)) = (t.iface_parts(a), t.iface_parts(b));
                if da != db || aa.is_empty() {
                    return Ok(());
                }
                parts(&aa, &ab, self)
            }
            _ => Ok(()),
        }
    }

    /// One meeting of placeholder `i` with `value`. Loose numbers widen to their common type (the
    /// earlier arguments convert); an exact binding never widens, and a loose value meeting it
    /// must convert to it.
    fn bind(&mut self, i: usize, value: Ty, exact: bool, from: Option<usize>, fill: bool) -> Result<(), Conflict> {
        // Nothing to decide from: an error was reported, or the value is no value.
        if matches!(value, Ty::Error | Ty::Never | Ty::Unit | Ty::Variant(..)) {
            return Ok(());
        }
        let new = Binding { ty: value, exact, from };
        let Some(old) = self.slots[i] else {
            self.slots[i] = Some(new);
            return Ok(());
        };
        if fill || old.ty == value {
            // A loose binding met again exactly is now exact, decided by the exact meeting.
            if !fill && exact && !old.exact {
                self.slots[i] = Some(new);
            }
            return Ok(());
        }
        let met = match (old.exact, exact) {
            (false, false) => Ty::common(old.ty, value).map(|ty| Binding { ty, exact: false, from: if ty == old.ty { old.from } else { from } }),
            (true, false) => value.coerces_to(old.ty).then_some(old),
            (false, true) => old.ty.coerces_to(value).then_some(new),
            (true, true) => None,
        };
        match met {
            Some(b) => {
                self.slots[i] = Some(b);
                Ok(())
            }
            None => Err(Conflict { param: self.params[i], old, new }),
        }
    }
}
