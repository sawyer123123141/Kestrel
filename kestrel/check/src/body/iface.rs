//! Interface values in function bodies (the interfaces spec's "Using an interface as a type" and
//! "Values and memory"): converting a concrete value to one, passing one to an interface
//! parameter (lent or boxed), calling through one, and the rules that keep their memory sound.
//!
//! A **coercion** (`Tables::coerce` maps the expression to the interface type, or to an optional
//! of it) is shallow: only a struct or enum value that lists the interface converts. KIR turns
//! each into a `MakeIface`. It is **lent** (the value points at the argument where it is) when
//! the expression is in `Tables::iface_borrows`, and **boxed** (copied into the current arena)
//! otherwise. Only a call argument for a parameter of exactly the interface type is ever lent.

use kestrel_syntax::ast::{Expr, ExprKind};

use super::lists::written;
use super::BodyChecker;
use crate::iface::implements;
use crate::tables::{EnumId, FnId, IfaceId, Res, StructId, Tables};
use crate::ty::Ty;

impl Tables {
    /// The function a call resolved to: the user function or method of `Res::Fn`, or the abstract
    /// method of an interface call or a call through a `T`'s rule (the one the call is checked
    /// against).
    pub(crate) fn callee(&self, res: Option<&Res>) -> Option<FnId> {
        match res? {
            Res::Fn(f) => Some(*f),
            Res::IfaceCall { iface, slot } | Res::ParamCall { iface, slot, .. } => Some(self.iface(*iface).methods[*slot as usize]),
            _ => None,
        }
    }

    /// The structs and enums that list interface `i`, in declaration order (structs first).
    fn implementors(&self, i: IfaceId) -> Vec<Ty> {
        let structs = (0..self.structs.len()).filter(|&s| self.structs[s].ifaces.contains(&i)).map(|s| Ty::Struct(StructId(s as u32)));
        let enums = (0..self.enums.len()).filter(|&e| self.enums[e].ifaces.contains(&i)).map(|e| Ty::Enum(EnumId(e as u32)));
        structs.chain(enums).collect()
    }
}

/// How a message writes an argument that isn't a place: a call as `Pool()`, else as `written`
/// shows it, else in words.
fn shown_value(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Call { name, args, .. } if args.is_empty() => format!("`{name}()`"),
        ExprKind::MethodCall { receiver, name, args, .. } if args.is_empty() => format!("`{}.{name}()`", written(receiver)),
        _ => match written(e).as_str() {
            "_" => "this value".to_string(),
            w => format!("`{w}`"),
        },
    }
}

impl<'a> BodyChecker<'a> {
    /// The interface `to` is, or an optional of, when a concrete `from` converts to it.
    pub(super) fn iface_target(&self, from: Ty, to: Ty) -> Option<IfaceId> {
        let i = match to {
            Ty::Iface(i) => i,
            Ty::Optional(e) => match self.t.elem(e) {
                Ty::Iface(i) => i,
                _ => return None,
            },
            _ => return None,
        };
        implements(self.t, from, i).then_some(i)
    }

    /// Records the conversion of `e`, a struct or enum value, to interface `to` (or `to?`) when
    /// `from` lists it. The value is boxed here (a lent argument goes through `check_arg_value`
    /// instead), so an owning one is refused. Returns whether it applied.
    pub(super) fn coerce_to_iface(&mut self, e: &Expr, from: Ty, to: Ty) -> bool {
        let Some(i) = self.iface_target(from, to) else { return false };
        self.t.coerce.insert(e.id, to);
        if self.t.is_owning(from) {
            let (name, what) = (self.tn(from), self.t.owns_what(from));
            let msg = format!("`{name}` owns {what}, so it can't be stored as a `{}`; keep it in a `list[{name}]`", self.t.iface(i).name);
            self.error(e.span, msg);
        }
        true
    }

    /// Whether the checked expression `e` is a struct or enum value (or a `T`, which is one in
    /// every copy) converted to an interface value by boxing: a copy of it goes into the current
    /// arena.
    pub(super) fn boxes_iface(&self, e: &Expr) -> bool {
        if !matches!(self.t.ty(e.id), Ty::Struct(_) | Ty::Enum(_) | Ty::Param(_)) || self.t.iface_borrows.contains(&e.id) {
            return false;
        }
        self.t.coerce.get(&e.id).is_some_and(|&to| self.boxes_into(to))
    }

    /// Whether a value converted to `to` is boxed as an interface value: `to` is an interface, its
    /// optional, or a `Result` whose `T` is one of those (the `Ok` wrap converts to `T` first).
    fn boxes_into(&self, to: Ty) -> bool {
        match to {
            Ty::Iface(_) => true,
            Ty::Optional(x) => matches!(self.t.elem(x), Ty::Iface(_)),
            other => self.t.result_parts(other).is_some_and(|(value, _)| matches!(value, Ty::Iface(_) | Ty::Optional(_)) && self.boxes_into(value)),
        }
    }

    /// An argument for a non-`var` parameter of type `want`. For an interface type, a struct or
    /// enum place (see `is_kir_place`) is **lent**: recorded in `iface_borrows`, so even an
    /// owning one may be passed. Anything else converts as a store would (boxed), and an owning
    /// one that isn't a place is refused.
    pub(super) fn check_arg_value(&mut self, e: &Expr, want: Ty) -> Ty {
        let found = self.check_expr(e, self.ok_hint(e, Some(want)));
        self.finish_arg_value(e, found, want)
    }

    /// The rest of `check_arg_value`, once `e` has been checked and found to be `found`.
    pub(super) fn finish_arg_value(&mut self, e: &Expr, found: Ty, want: Ty) -> Ty {
        let Ty::Iface(i) = want else { return self.finish_value(e, found, Some(want)) };
        if found == Ty::Never {
            return self.reject_never(e, found);
        }
        if implements(self.t, found, i) {
            if self.is_kir_place(e) {
                self.t.coerce.insert(e.id, want);
                self.t.iface_borrows.insert(e.id);
                return want;
            }
            if self.t.is_owning(found) {
                let msg = format!("{} owns {}; store it in a `let` first, then pass it", shown_value(e), self.t.owns_what(found));
                self.error(e.span, msg);
                self.t.coerce.insert(e.id, want);
                return want;
            }
        }
        self.coerce(e, found, want);
        want
    }

    /// A `var` argument `a` of type `ty` for a `var` parameter of type `want`: when `want` is
    /// exactly an interface that `ty` implements, the argument is lent. (Nothing else converts
    /// under `var`: the callee could write `none`, or another type, into it.) Returns whether it
    /// applied.
    pub(super) fn lend_var_arg(&mut self, a: &Expr, ty: Ty, want: Ty) -> bool {
        let Ty::Iface(i) = want else { return false };
        if !implements(self.t, ty, i) {
            return false;
        }
        self.t.coerce.insert(a.id, want);
        self.t.iface_borrows.insert(a.id);
        true
    }

    /// Whether KIR's `place()` (`kir/src/lower/call.rs`) takes `e` for a place, so a `MakeIface`
    /// can point at it. This mirrors that function, and must change with it: a local or
    /// parameter (a capture read inside a closure included), a constant KIR reads in place
    /// (`Tables::const_read_in_place`, which `kir::lower::consts::hoisted` asks too), a field of a place
    /// or a lane of a `vec4` (a swizzle, `.len` or an enum variant is neither), and an element of a place
    /// that is an array or list. A map lookup `m[k]` and a string index `s[i]` build a value, so
    /// they are not places.
    pub(super) fn is_kir_place(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Ident(_) => match self.t.res.get(&e.id) {
                Some(Res::Local(_)) => true,
                Some(&Res::Const(c)) => self.t.const_read_in_place(self.t.consts[c.0 as usize].ty),
                _ => false,
            },
            ExprKind::Index { base, .. } => !matches!(self.t.ty(base.id), Ty::Map(..) | Ty::Str) && self.is_kir_place(base),
            ExprKind::Field { base, .. } => self.is_field_or_lane(e) && self.is_kir_place(base),
            _ => false,
        }
    }

    /// The method `name` of interface `i`: its abstract function and its slot in the method table.
    pub(super) fn iface_method(&self, i: IfaceId, name: &str) -> Option<(FnId, u32)> {
        let methods = &self.t.iface(i).methods;
        let slot = methods.iter().position(|&m| self.t.func(m).name.rsplit('.').next() == Some(name))?;
        Some((methods[slot], slot as u32))
    }

    /// `d = value` where `d` is a `var` parameter of interface type: the caller's place may hold
    /// a different type than the one assigned, which wouldn't fit. Reported after `value` is
    /// checked; returns whether it was refused. A `T` that may be an interface type counts too
    /// (P39): in a copy where `T` is an interface type, the caller's concrete place is lent to the
    /// `var` parameter, and writing another type into it would corrupt it. The target's type
    /// decides, whatever the value's expression is (`a = b`, `a = xs.pop()`, an `if`).
    pub(super) fn check_iface_replace(&mut self, target: &Expr, value: &Expr, target_ty: Ty) -> bool {
        let (iface, placeholder) = match target_ty {
            Ty::Iface(i) => (Some(i), None),
            // A `T` with a built-in rule or two interface rules is never an interface type; any
            // other may be one in some copy, and its interface rule gives the example.
            Ty::Param(p) if !self.t.param_may_be_iface(p) => return false,
            Ty::Param(p) => (self.t.generic.param(p).ifaces.first().copied(), Some(p)),
            _ => return false,
        };
        if !matches!(target.kind, ExprKind::Ident(_)) {
            return false;
        }
        let Some(root) = self.place_root(target) else { return false };
        if !self.t.local(root).is_var_param || self.immutable(root).is_some() {
            return false;
        }
        // A type other than the one being assigned, as the example of what the caller may hold.
        let assigned = self.t.ty(value.id);
        let all = iface.map(|i| self.t.implementors(i)).unwrap_or_default();
        let example = match all.iter().find(|&&t| t != assigned).or(all.first()) {
            Some(&t) => format!("`{}`", self.tn(t)),
            None => "value of another type".to_string(),
        };
        let name = self.t.local(root).name.clone();
        let msg = format!("can't replace `{name}`: it may be the caller's {example}");
        match placeholder {
            // With no interface rule, a built-in one rules interface types out (and there are no
            // methods to change it through). With one, an `equatable` type couldn't meet it: the
            // generic would be useless.
            Some(p) if iface.is_none() => {
                let pn = self.t.param_name(p).to_string();
                self.error_help(target.span, msg, format!("if `{pn}` is never an interface type, give it a built-in rule, like `[{pn}: equatable]`"));
            }
            _ => self.error(target.span, format!("{msg}; change it through its methods")),
        }
        true
    }

    /// For an unannotated array literal whose first item is an `elem` and another is a `found`:
    /// the name of an interface that these two different struct or enum types both list.
    pub(super) fn mixed_by_iface(&self, elem: Ty, found: Ty) -> Option<String> {
        if elem == found {
            return None;
        }
        let (a, b) = (elem, found);
        let ifaces = |t: Ty| match t {
            Ty::Struct(s) => self.t.strukt(s).ifaces.clone(),
            Ty::Enum(e) => self.t.enm(e).ifaces.clone(),
            _ => Vec::new(),
        };
        let theirs = ifaces(b);
        ifaces(a).into_iter().find(|i| theirs.contains(i)).map(|i| self.t.iface(i).name.clone())
    }
}
