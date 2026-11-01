//! `xs.filter(f)`, `xs.map(f)`, `xs.any(f)`, `xs.all(f)`, `xs.sort()` and `xs.sort_by(key)`: the
//! list methods that call a function for the items, and the sort. The first four don't change `xs`;
//! the sorts change its order, so they need a variable to sort (as `add` does) and refuse to run
//! inside a loop over the list.
//!
//! `f` is any function value of type `(T) -> bool` (`(T) -> R` for `map`, where `R` is taken from
//! the function, and `pure (T) -> K` for `sort_by`, whose key is a number, string or bool): a
//! literal, a named function, a stored closure or a parameter. A literal
//! that is directly the argument gets a stack environment, like one passed to a user function
//! (`Tables::stack_closures`), so the call exclusivity rule applies to it. In a `pure` function
//! or a `par for` body the function must be pure: a literal is checked by its frame, and any
//! other function value must have a `pure` type. A sort key must be pure wherever it is.

use kestrel_syntax::ast::{Arg, Expr, ExprKind};
use kestrel_syntax::Span;

use super::closure::Impure;
use super::exclusive::{CalleePlace, RecvChange};
use super::fnvalue::shown_callee;
use super::lists::{written, Overlap};
use super::param_ops::sortable;
use super::BodyChecker;
use crate::sig::{Sig, SigId, SigParam};
use crate::tables::{ListMethod, Res};
use crate::ty::Ty;

/// The method as it is written.
fn word(method: ListMethod) -> &'static str {
    match method {
        ListMethod::Filter => "filter",
        ListMethod::Map => "map",
        ListMethod::Any => "any",
        ListMethod::All => "all",
        ListMethod::SortBy => "sort_by",
        ListMethod::Sort => "sort",
        other => unreachable!("`{other:?}` takes no function"),
    }
}

impl<'a> BodyChecker<'a> {
    /// The call `e` of `method` (one of the four) on `receiver`, which has type `recv_ty`.
    /// `args` is the one argument, already known to be unlabelled and not `var`.
    pub(super) fn list_fn(&mut self, e: &Expr, receiver: &Expr, recv_ty: Ty, method: ListMethod, name_span: Span, args: &[Arg]) -> Ty {
        let elem = self.elem_of(recv_ty).expect("a collection type");
        let name = word(method);
        let f = &args[0].value;
        // The copy `filter` makes of each kept item can't hold an owner.
        let refused = method == ListMethod::Filter && self.t.is_owning(elem);
        if refused {
            let what = self.t.owns_what(elem).replace('`', "");
            // (A receiver that isn't a place, like `xs.map(...)`, has no name to give.)
            let list = match receiver.kind {
                ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::Index { .. } => format!("`{}`", written(receiver)),
                _ => "this list".to_string(),
            };
            let msg = format!("`filter` would copy the items of {list}, which own {what}; use a `for` loop with `remove`");
            self.error(name_span, msg);
        }
        let literal = matches!(f.kind, ExprKind::Closure(_));
        // The function takes an item, read-only. `map` leaves the result open: it is the closure's.
        let item = SigParam { ty: elem, is_var: false };
        // `map` and `sort_by` take the result type from the function.
        let open = matches!(method, ListMethod::Map | ListMethod::SortBy);
        let result = if open { Ty::Error } else { Ty::Bool };
        let sorting = method == ListMethod::SortBy;
        let want = Ty::Fn(self.t.intern_sig(Sig { params: vec![item], ret: result, is_pure: sorting }));
        self.note_stack_closure(&args[0]);
        if literal {
            self.unstored.insert(f.id);
        }
        let found = if open {
            self.infer_result = literal;
            if literal && sorting {
                self.arg_name = Some(name.to_string());
            }
            let found = self.check_expr(f, Some(want));
            self.reject_never(f, found)
        } else {
            self.check_value(f, Some(want));
            self.t.ty(f.id)
        };
        let given = match method {
            ListMethod::Map | ListMethod::SortBy => match self.fn_result(method, f, found, item, elem, literal) {
                Some(r) => Some(r),
                None => return Ty::Error,
            },
            _ => None,
        };
        // What the function does must suit the code around.
        match found {
            // (A key that isn't pure was refused with the literal, or is refused below.)
            Ty::Fn(_) if sorting => {}
            Ty::Fn(_) if literal => {
                if !self.t.closures.get(&f.id).is_some_and(|c| c.is_pure) {
                    let shown = format!("the closure passed to `{name}`");
                    self.closure_impure(f.span, Impure::Value(&shown));
                }
            }
            Ty::Fn(sig) if matches!(self.t.res.get(&f.id), Some(Res::FnValue(_))) => self.check_named_purity(sig, f),
            Ty::Fn(sig) => self.check_value_purity(sig, f.span, &shown_callee(f)),
            _ => {}
        }
        // The function runs while `xs` is read, so it can't be given anything the call changes.
        let how = if sorting { RecvChange::Reordered } else { RecvChange::Read };
        let changes = self.call_changes(args, Some((receiver, how)));
        let mut callee = None;
        if sorting && !literal {
            callee = self.key_place(f);
            self.check_key_purity(f, found);
        }
        self.check_call_captures(e.span, args, &changes, callee);
        if refused {
            return Ty::Error;
        }
        match method {
            ListMethod::SortBy => Ty::Unit,
            ListMethod::Filter => Ty::List(self.t.intern_elem(elem)),
            ListMethod::Map => Ty::List(self.t.intern_elem(given.expect("map has a result"))),
            _ => Ty::Bool,
        }
    }

    /// A named function given as the function: in a `pure` function or a `par for` body it must
    /// be declared `pure`.
    fn check_named_purity(&mut self, sig: SigId, f: &Expr) {
        if self.t.sig(sig).is_pure {
            return;
        }
        let shown = shown_callee(f);
        if self.closure_impure(f.span, Impure::Value(&format!("`{shown}`"))) {
            return;
        }
        if self.is_pure {
            let msg = format!("`{shown}` isn't `pure`, so a `pure` function can't call it");
            self.error_help(f.span, msg, format!("declare it as `pure {shown}(...)`"));
        } else if !self.par.is_empty() {
            self.error(f.span, format!("`par for` bodies can only call `pure` functions; `{shown}` isn't `pure`"));
        }
    }

    /// The type of the items `map`'s function `f` (of type `found`) gives, or of the keys
    /// `sort_by`'s gives, or `None` after reporting why it can't be used.
    fn fn_result(&mut self, method: ListMethod, f: &Expr, found: Ty, item: SigParam, elem: Ty, literal: bool) -> Option<Ty> {
        let (name, sorting) = (word(method), method == ListMethod::SortBy);
        let gives = if sorting { "a number, string or bool" } else { "a value" };
        let Ty::Fn(id) = found else {
            if found != Ty::Error {
                let msg = format!("`{name}` needs a function that takes `{}` and gives {gives}, found {}", self.tn(elem), self.tn(found));
                self.error(f.span, msg);
            }
            return None;
        };
        let sig = self.t.sig(id).clone();
        if sig.params != [item] {
            let msg = format!("`{name}` needs a function that takes `{}`, found `{}`", self.tn(elem), self.t.sig_name(id));
            self.error(f.span, msg);
            return None;
        }
        match sig.ret {
            // Said already.
            Ty::Error => None,
            Ty::Unit if sorting => {
                self.error(f.span, "`sort_by` needs a key that is a number, string or bool; this one gives nothing");
                None
            }
            Ty::Unit => {
                let what = if literal { "closure" } else { "function" };
                self.error(f.span, format!("`map` needs a {what} that gives a value"));
                None
            }
            ret if sorting && !sortable(self.t, ret) => {
                // A placeholder is told which rule it needs; anything else what a key may be.
                if !self.list_param_rule("sort_by", ret, f.span) {
                    let msg = format!("`sort_by` needs a key that is a number, string or bool; this one gives `{}`", self.tn(ret));
                    self.error(f.span, msg);
                }
                None
            }
            ret => Some(ret),
        }
    }

    /// The place a `sort_by` key that isn't a literal is read from, if it is read from one.
    fn key_place(&self, f: &Expr) -> Option<CalleePlace> {
        if matches!(self.t.res.get(&f.id), Some(Res::FnValue(_))) {
            return None;
        }
        Some(CalleePlace { key: self.source_key(f)?, shown: shown_callee(f) })
    }

    /// A key that isn't a literal must be a pure function: sorting calls it about twice per
    /// comparison, in no promised order.
    fn check_key_purity(&mut self, f: &Expr, found: Ty) {
        let Ty::Fn(sig) = found else { return };
        if self.t.sig(sig).is_pure {
            return;
        }
        let shown = shown_callee(f);
        if matches!(self.t.res.get(&f.id), Some(Res::FnValue(_))) {
            let msg = format!("`{shown}` isn't `pure`, but `sort_by` needs a pure function");
            self.error_help(f.span, msg, format!("declare it as `pure {shown}(...)`"));
        } else {
            let msg = format!("`{shown}` isn't a pure function type, but `sort_by` needs a pure one");
            self.error_help(f.span, msg, format!("give it the type `pure {}`", self.t.sig_name(sig)));
        }
    }

    /// `xs.sort()` and `xs.sort_by(key)`: `xs` changes order, so it needs a variable, and no loop
    /// over it (or over a list holding it) may be running.
    pub(super) fn list_sort(&mut self, e: &Expr, receiver: &Expr, recv_ty: Ty, method: ListMethod, name_span: Span, args: &[Arg]) -> Ty {
        let name = word(method);
        // Lowering sorts numbers and bools by keys in an `[i64]` or `[f64]` (see `kir`'s
        // `lower::sort`), whose element types need interning.
        self.t.intern_elem(Ty::I64);
        self.t.intern_elem(Ty::F64);
        if self.receiver_root(receiver, name).is_some() {
            if let Some(Overlap { list, .. }) = self.overlapping_loop(receiver) {
                self.error(e.span, format!("can't sort `{list}` while looping over it; sort it after the loop"));
            }
        }
        if method == ListMethod::SortBy {
            return self.list_fn(e, receiver, recv_ty, method, name_span, args);
        }
        let elem = self.elem_of(recv_ty).expect("a collection type");
        if self.list_param_rule("sort", elem, name_span) {
            return Ty::Error;
        }
        if !(sortable(self.t, elem) || elem == Ty::Error) {
            let msg = format!("`sort` needs numbers, strings or bools; `{}` holds `{}`; use `sort_by` with a key", self.tn(recv_ty), self.tn(elem));
            self.error(name_span, msg);
            return Ty::Error;
        }
        Ty::Unit
    }
}
