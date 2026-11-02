//! Which types can be `T` (R3, R7): not owning, not an optional, not a function type and not a
//! type holding a function (R3, G6), and meeting every rule of the placeholder (R7).
//!
//! Written instances (`Stack[X]`) are checked where they are written. During `collect` the
//! arguments are queued (owning-ness and instances' fields aren't known yet) and checked after
//! `compute_owning` (phase 7); in bodies they are checked at once, and an instance with a refused
//! argument isn't made (the written type is `Error`). Queued instances already exist, but they
//! are only in a program with errors, which is never lowered, and `compute_owning` gives every
//! instance its declaration's owning-ness, so none is owning because of its arguments.

use kestrel_syntax::{Diagnostic, Span};

use crate::collect_ty::TypeScope;
use crate::generic::{GenericOwner, ParamId};
use crate::iface::implements;
use crate::tables::{IfaceId, Tables};
use crate::ty::Ty;

/// A written type argument waiting for phase 7: `arg` for `param` of `generic`, written at `at`
/// in a scope with these placeholders.
#[derive(Clone, Debug)]
pub(crate) struct QueuedArg {
    arg: Ty,
    param: ParamId,
    generic: String,
    at: Span,
    params: Vec<ParamId>,
    own: Option<GenericOwner>,
}

/// A rule a placeholder may carry.
#[derive(Clone, Copy)]
enum Rule {
    Iface(IfaceId),
    Equatable,
    Ordered,
    Key,
}

impl Tables {
    /// R3, then R7, for `arg` as `param` of `generic` (its name as `ty_name` writes it): the
    /// error, if `arg` can't be `param`. `at` is where the type was written (the caller reports
    /// there); `scope` the placeholders of the surrounding generic, which meet the rules they carry.
    pub(crate) fn check_type_arg(&self, arg: Ty, param: ParamId, generic: &str, _at: Span, scope: &TypeScope) -> Option<String> {
        if arg == Ty::Error {
            return None;
        }
        let (x, p) = (self.ty_name(arg), self.param_name(param));
        if self.is_owning(arg) {
            return Some(format!("`{x}` owns {}, so it can't be `{p}` yet", self.owns_what(arg)));
        }
        // `Result` alone may hold an optional (`from_bytes[int?]` gives `Result[int?, string]`).
        let in_result = matches!(self.generic.param(param).owner, GenericOwner::Enum(e) if Some(e) == self.result);
        if matches!(arg, Ty::Optional(_)) && !in_result {
            return Some(format!("`{x}` is an optional, so it can't be `{p}` yet; hold it in a struct field instead"));
        }
        if matches!(arg, Ty::Fn(_)) {
            return Some(format!("`{x}` is a function type, so it can't be `{p}` yet"));
        }
        if self.holds_fn(arg) {
            return Some(format!("`{x}` holds a function value, so it can't be `{p}` yet"));
        }
        let missing = self.rules_of(param).into_iter().find(|&r| !self.meets(arg, r))?;
        let what = match missing {
            Rule::Iface(i) => format!("`{}`", self.iface(i).name),
            Rule::Equatable => "`equatable` (numbers, bools, strings, vectors, and structs and enums of them are)".to_string(),
            Rule::Ordered => "`ordered` (numbers, bools and strings are)".to_string(),
            Rule::Key => "a `key` (integers, bools and strings are)".to_string(),
        };
        let mut msg = format!("`{x}` isn't {what}, so it can't be `{p}` in `{generic}`");
        if let Ty::Param(u) = arg {
            if scope.params.contains(&u) {
                let mut rules: Vec<String> = self.rules_of(u).into_iter().map(|r| self.rule_name(r)).collect();
                rules.push(self.rule_name(missing));
                msg.push_str(&format!("; add the rule: `[{}: {}]`", self.param_name(u), rules.join(" + ")));
            }
        }
        Some(msg)
    }

    /// R3 and R7 for the written arguments `tys` of `owner` (at `spans`). During `collect` they
    /// are queued and `false` is returned; afterwards each refused one is reported and the result
    /// says whether any was.
    pub(crate) fn check_written_args(
        &mut self,
        owner: GenericOwner,
        tys: &[Ty],
        spans: &[Span],
        scope: &TypeScope,
        diags: &mut Vec<Diagnostic>,
    ) -> bool {
        let params = self.owner_params(owner).to_vec();
        let generic = self.owner_name(owner).to_string();
        let mut refused = false;
        for ((&arg, &param), &at) in tys.iter().zip(&params).zip(spans) {
            if let Some(queue) = &mut self.generic.queued_args {
                let (params, own) = (scope.params.to_vec(), scope.own);
                queue.push(QueuedArg { arg, param, generic: generic.clone(), at, params, own });
            } else if let Some(msg) = self.check_type_arg(arg, param, &generic, at, scope) {
                diags.push(Diagnostic::error(at, msg));
                refused = true;
            }
        }
        refused
    }

    /// Phase 7 of `collect`, after `compute_owning`: the queued arguments' checks, and from here
    /// on written arguments are checked at once.
    pub(crate) fn check_queued_args(&mut self, diags: &mut Vec<Diagnostic>) {
        let Some(queue) = self.generic.queued_args.take() else { return };
        for q in queue {
            let scope = TypeScope { params: &q.params, own: q.own };
            if let Some(msg) = self.check_type_arg(q.arg, q.param, &q.generic, q.at, &scope) {
                diags.push(Diagnostic::error(q.at, msg));
            }
        }
    }

    /// A placeholder's rules: its interfaces in the order written, then the words (`equatable`
    /// only when neither `ordered` nor `key`, which carry it, is there).
    fn rules_of(&self, p: ParamId) -> Vec<Rule> {
        let info = self.generic.param(p);
        let mut rules: Vec<Rule> = info.ifaces.iter().map(|&i| Rule::Iface(i)).collect();
        if info.equatable && !info.ordered && !info.key {
            rules.push(Rule::Equatable);
        }
        if info.ordered {
            rules.push(Rule::Ordered);
        }
        if info.key {
            rules.push(Rule::Key);
        }
        rules
    }

    /// A placeholder's rules as a message writes them: its interfaces, then the words.
    pub(crate) fn rule_names(&self, p: ParamId) -> Vec<String> {
        self.rules_of(p).into_iter().map(|r| self.rule_name(r)).collect()
    }

    /// Whether some type that meets `p`'s rules may be an interface type (P39). Under R7 an
    /// interface type meets only its own interface rule and is never `equatable` (which `ordered`
    /// and `key` set too), so a placeholder with a built-in rule or two interface rules isn't one.
    pub(crate) fn param_may_be_iface(&self, p: ParamId) -> bool {
        let info = self.generic.param(p);
        !info.equatable && info.ifaces.len() <= 1
    }

    fn rule_name(&self, r: Rule) -> String {
        match r {
            Rule::Iface(i) => self.iface(i).name.clone(),
            Rule::Equatable => "equatable".to_string(),
            Rule::Ordered => "ordered".to_string(),
            Rule::Key => "key".to_string(),
        }
    }

    /// Whether `arg` meets rule `r`: an interface type meets its own rule only; a placeholder the
    /// rules it carries; other types by `implements` and the sets in the spec.
    fn meets(&self, arg: Ty, r: Rule) -> bool {
        match (arg, r) {
            (Ty::Iface(j), Rule::Iface(i)) => i == j,
            (Ty::Iface(_), _) => false,
            (_, Rule::Iface(i)) => implements(self, arg, i),
            (Ty::Param(q), _) => {
                let info = self.generic.param(q);
                match r {
                    Rule::Equatable => info.equatable,
                    Rule::Ordered => info.ordered,
                    Rule::Key => info.key,
                    Rule::Iface(_) => unreachable!("matched above"),
                }
            }
            (_, Rule::Equatable) => self.equatable(arg),
            (_, Rule::Ordered) => matches!(arg, Ty::Int | Ty::I64 | Ty::U8 | Ty::Float | Ty::F64 | Ty::Bool | Ty::Str),
            (_, Rule::Key) => matches!(arg, Ty::Int | Ty::I64 | Ty::U8 | Ty::Bool | Ty::Str),
        }
    }
}

impl Tables {
    /// Whether `==` compares values of `ty` (decision 19): numbers, bools, strings and vectors; a
    /// struct whose fields are and an enum whose variants' fields are (a bare variant always is),
    /// judged with an instance's own types; a placeholder with the rule. Lists, arrays, maps,
    /// optionals, interface and function values, arenas and GPU objects aren't.
    pub(crate) fn equatable(&self, ty: Ty) -> bool {
        self.not_equatable(ty, &mut Vec::new()).is_none()
    }

    /// Why `==` can't compare two values of struct or enum `ty`, as the refusal says it: "its
    /// field `chunks` is a list". `None` when it can, or `ty` is neither.
    pub(crate) fn equatable_refusal(&self, ty: Ty) -> Option<String> {
        if !matches!(ty, Ty::Struct(_) | Ty::Enum(_)) {
            return None;
        }
        self.part_not_equatable(ty, &mut vec![ty]).map(|why| format!("its {why}"))
    }

    /// Why a value of `ty` can't be compared, as "is a list" or "is a `World`, whose field
    /// `chunks` is a list"; `None` when it can. `seen` stops a walk through a type that contains
    /// itself (only possible through a collection, which already isn't equatable, or an error
    /// reported elsewhere).
    fn not_equatable(&self, ty: Ty, seen: &mut Vec<Ty>) -> Option<String> {
        let what = match ty {
            _ if ty.is_numeric() => return None,
            Ty::Bool | Ty::Str | Ty::Vec4 | Ty::IVec4 | Ty::Error => return None,
            Ty::Param(p) if self.generic.param(p).equatable => return None,
            Ty::Struct(_) | Ty::Enum(_) if seen.contains(&ty) => return None,
            Ty::Struct(_) | Ty::Enum(_) => {
                seen.push(ty);
                let why = self.part_not_equatable(ty, seen)?;
                return Some(format!("is a `{}`, whose {why}", self.ty_name(ty)));
            }
            Ty::List(_) => "a list",
            Ty::Array(_) => "an array",
            Ty::Map(..) => "a map",
            Ty::Optional(_) => "an optional",
            Ty::Iface(_) => "an interface value",
            Ty::Fn(_) => "a function value",
            Ty::Arena => "an `Arena`",
            Ty::Gpu => "a GPU object",
            _ => return Some(format!("is a `{}`", self.ty_name(ty))),
        };
        Some(format!("is {what}"))
    }

    /// The first field of struct or enum `ty`, in declaration order, that `==` can't compare:
    /// "field `chunks` is a list", or "variant `Full`'s field `items` is an array".
    fn part_not_equatable(&self, ty: Ty, seen: &mut Vec<Ty>) -> Option<String> {
        match ty {
            Ty::Struct(s) => {
                for f in &self.strukt(s).fields {
                    if let Some(why) = self.not_equatable(f.ty, seen) {
                        return Some(format!("field `{}` {why}", f.name));
                    }
                }
            }
            Ty::Enum(e) => {
                for v in &self.enm(e).variants {
                    for f in &v.fields {
                        if let Some(why) = self.not_equatable(f.ty, seen) {
                            return Some(format!("variant `{}`'s field `{}` {why}", v.name, f.name));
                        }
                    }
                }
            }
            _ => {}
        }
        None
    }
}
