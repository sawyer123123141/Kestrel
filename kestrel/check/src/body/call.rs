use kestrel_syntax::ast::{Arg, Expr, ExprKind};
use kestrel_syntax::Span;

use super::args::CallSig;
use super::closure::Impure;
use super::nested::Later;
use super::BodyChecker;
use crate::builtins::Builtin;
use crate::generic::GenericOwner;
use crate::tables::{FnId, Res};
use crate::ty::Ty;

const CONVERSIONS: &[(&str, Ty)] = &[
    ("int", Ty::Int),
    ("i64", Ty::I64),
    ("u8", Ty::U8),
    ("float", Ty::Float),
    ("f64", Ty::F64),
];

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_call(&mut self, e: &Expr, want: Option<Ty>) -> Ty {
        let ExprKind::Call { name, name_span, args } = &e.kind else { unreachable!("check_call on non-call") };
        // A local or parameter of function type: `f(x)` calls through its value.
        if let Some(local) = self.lookup_local(name) {
            return self.check_local_call(e, local, name, *name_span, args);
        }
        if name == "print" {
            return self.check_print(e, args);
        }
        if name == "Arena" {
            self.t.res.insert(e.id, Res::NewArena);
            if !args.is_empty() {
                self.error(e.span, "`Arena()` takes no arguments");
                for a in args {
                    self.check_expr(&a.value, None);
                }
            }
            return Ty::Arena;
        }
        if let Some(&(_, to)) = CONVERSIONS.iter().find(|(n, _)| n == name) {
            return self.check_convert(e, to, args);
        }
        if let Some(b) = Builtin::global(name) {
            return self.check_builtin_call(e, b, args);
        }
        if let Some(vt) = Ty::from_name(name).filter(|&t| self.t.vector(t).is_some()) {
            return self.check_vec_make(e, vt, args);
        }
        if let Some(&sid) = self.t.struct_by_name.get(name) {
            if self.t.vector(Ty::Struct(sid)).is_some() {
                return self.check_vec_make(e, Ty::Struct(sid), args);
            }
            // A generic struct's types are worked out, except inside its own declaration, where
            // its bare name means itself (G2).
            if !self.t.strukt(sid).params.is_empty() && self.own != Some(GenericOwner::Struct(sid)) {
                return self.check_inferred_construct(e, sid, name, *name_span, args, want);
            }
            return self.check_construct(e, sid, args);
        }
        let Some(&fid) = self.t.fn_by_name.get(name) else {
            if self.param_named(name).is_some() {
                self.error(
                    *name_span,
                    format!("`{name}` is a placeholder, so it can't be built here; take a value of it as a parameter instead"),
                );
            } else if let Some(&eid) = self.t.enum_by_name.get(name) {
                // A real variant makes the example, with `(...)` when it has fields.
                let example = self.variant_example(eid, name);
                self.error_help(
                    *name_span,
                    format!("`{name}` is an enum, so it can't be called like a struct"),
                    format!("build a value from one of its variants, like {example}"),
                );
            } else if self.t.consts.iter().any(|c| c.name == *name) {
                // A constant of function type is called by its name, like a local.
                if let Some((c, sig)) = self.t.const_by_name.get(name).and_then(|&c| match self.t.consts[c.0 as usize].ty {
                    Ty::Fn(sig) => Some((c, sig)),
                    _ => None,
                }) {
                    return self.check_const_call(e, c, sig, *name_span, args);
                }
                self.error_help(*name_span, format!("`{name}` is a constant, not a function"), format!("use it without `()`: `{name}`"));
            } else if let Some(msg) = self.bare_variant(name) {
                self.error(*name_span, msg);
            } else if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                self.error_help(
                    *name_span,
                    format!("unknown struct `{name}`"),
                    format!("declare it at the top level of a file (`struct {name} {{ ... }}`), or bring it in from a module with `use {{ {name} from m }}`"),
                );
            } else {
                self.unknown_function(*name_span, name, args);
            }
            for a in args {
                self.check_expr(&a.value, None);
            }
            return Ty::Error;
        };
        // A generic function's types are worked out from the arguments and `want`.
        if self.is_generic_fn(fid) {
            return self.check_inferred_call(e, fid, name, *name_span, args, want);
        }
        self.t.res.insert(e.id, Res::Fn(fid));
        self.check_callee_purity(*name_span, fid);
        self.check_user_call(e, fid, args)
    }

    /// A call statement whose result is thrown away.
    pub(crate) fn check_discarded_call(&mut self, e: &Expr) -> Ty {
        self.discarded = Some(e.id);
        let ty = self.check_expr(e, None);
        let res = self.t.res.get(&e.id).copied();
        // A user function or method, or an interface method called through an interface value.
        let callee = self.t.callee(res.as_ref());
        match res {
            // A pure function changes nothing but its `var` arguments.
            Some(Res::Fn(_) | Res::IfaceCall { .. } | Res::ParamCall { .. })
                if callee.is_some_and(|f| self.t.fns[f.0 as usize].is_pure && !self.t.fns[f.0 as usize].params.iter().any(|p| p.is_var)) =>
            {
                let name = self.t.fns[callee.expect("a call").0 as usize].name.clone();
                self.error(e.span, format!("the result of pure function `{name}` is unused, so this line does nothing"));
            }
            // (`fail` and `exit` never return: a statement is how they are used.)
            Some(Res::Builtin(b)) if b.is_pure() && !b.never_returns() => {
                self.error(e.span, format!("the result of `{}` is unused, so this line does nothing", b.name()));
            }
            Some(Res::MapRead) => {
                self.error(e.span, "this only reads the map, so the line does nothing");
            }
            Some(Res::List(m)) if !m.changes_receiver() => {
                self.error(e.span, "this only reads the list, so the line does nothing");
            }
            Some(Res::Str(_)) => {
                self.error(e.span, "this only reads the string, so the line does nothing");
            }
            Some(Res::Convert(_) | Res::Construct(_) | Res::SimdMake(_) | Res::Variant(..) | Res::NewArena) => {
                self.error(e.span, "this line has no effect; did you mean to continue the line above?");
            }
            _ => {}
        }
        ty
    }

    fn check_print(&mut self, e: &Expr, args: &[Arg]) -> Ty {
        self.t.res.insert(e.id, Res::Print);
        // In a closure this makes the closure impure, not the function around it.
        if !self.closure_impure(e.span, Impure::Print) {
            if self.is_pure {
                self.error(e.span, "pure functions can't call `print`");
            } else if !self.par.is_empty() {
                self.error(e.span, "`par for` bodies can't print: iterations run at the same time");
            }
            if let Some(f) = self.fn_id {
                self.t.fns[f.0 as usize].io = true;
            }
        }
        for a in args {
            if a.label.is_some() || a.is_var {
                self.error(a.span, "`print` arguments can't be labelled or `var`");
            }
            self.check_printable(&a.value);
        }
        // Each value is kept until all of them have been evaluated, then printed.
        let values: Vec<&Expr> = args.iter().map(|a| &a.value).collect();
        self.check_kept_parts(&values, Later::Argument);
        Ty::Unit
    }

    /// A value `print` or an interpolated string can show.
    pub(crate) fn check_printable(&mut self, e: &Expr) {
        let ty = self.check_expr(e, None);
        let ty = self.reject_never(e, ty);
        if ty == Ty::Unit {
            self.error(e.span, "this has no value to print");
        }
    }

    fn check_convert(&mut self, e: &Expr, to: Ty, args: &[Arg]) -> Ty {
        self.t.res.insert(e.id, Res::Convert(to));
        if args.len() != 1 || args[0].label.is_some() || args[0].is_var {
            self.error(e.span, format!("`{}(...)` takes exactly one value", to.name()));
            for a in args {
                self.check_expr(&a.value, None);
            }
            return to;
        }
        let from = self.check_expr(&args[0].value, Some(to));
        if !from.is_numeric() && from != Ty::Error {
            let msg = format!("can't convert {} to {}", self.tn(from), to.name());
            self.error(args[0].value.span, msg);
        }
        to
    }

    pub(crate) fn check_callee_purity(&mut self, span: Span, callee: FnId) {
        let info = &self.t.fns[callee.0 as usize];
        if !info.is_pure {
            let name = info.name.clone();
            if self.closure_impure(span, Impure::Call(&name)) {
                return;
            }
        }
        let info = &self.t.fns[callee.0 as usize];
        if self.is_pure && !info.is_pure {
            let name = info.name.clone();
            self.error_help(
                span,
                format!("pure functions can only call pure functions; `{name}` isn't pure"),
                format!("mark `{name}` as `pure` if it has no effects"),
            );
        } else if let (false, Some((msg, help))) = (info.is_pure, self.par_call_error(&info.name)) {
            self.error_help(span, msg, help);
        }
    }

    fn check_user_call(&mut self, e: &Expr, fid: FnId, args: &[Arg]) -> Ty {
        self.check_args(e, CallSig::of(self.t, fid), args, None, None)
    }
}
