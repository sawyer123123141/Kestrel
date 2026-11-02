use kestrel_syntax::ast::{Arg, Expr, ExprKind, Item, Program};
use kestrel_syntax::{Diagnostic, Span};

use super::args::Receiver;
use super::lists::written;
use super::place::Immutable;
use super::consts::struct_field_read;
use super::generic_call::user_type_call;
use super::{is_const_expr, BodyChecker};
use crate::builtins::Builtin;
use crate::generic::GenericOwner;
use crate::tables::{FnId, ListMethod, LocalId, Res, StructId, Tables, BUILTIN_STRUCTS};
use crate::ty::Ty;

/// The error for a field default that isn't a constant expression.
const DEFAULT_EXPR: &str = "field defaults can only use literals, operators, constants, struct and enum values, and pure built-ins like `sqrt`";

/// Field defaults are constant expressions of the field's type (see `is_const_expr`), built again
/// at every construction that leaves the field out. They can't build a value that owns an arena or
/// a GPU object (other than a fresh `Arena()`).
pub(crate) fn check_field_defaults(program: &Program, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let structs = program.items.iter().filter_map(|it| match it {
        Item::Struct(s) => Some(s),
        _ => None,
    });
    for (sid, s) in structs.enumerate() {
        let sid = sid + BUILTIN_STRUCTS as usize;
        // A generic struct's defaults are checked once, with its placeholders in scope.
        let own = (!s.type_params.is_empty()).then_some(GenericOwner::Struct(StructId(sid as u32)));
        for f in &s.fields {
            let Some(default) = &f.default else { continue };
            let Some(field) = t.structs[sid].fields.iter().find(|g| g.name == f.name) else { continue };
            let ty = field.ty;
            let mut bc = BodyChecker::new(t, diags, None, Ty::Unit, false);
            bc.set_scope(own);
            if !is_const_expr(default, bc.t) {
                let (span, msg) = super::stop_in_constant(default).unwrap_or((default.span, DEFAULT_EXPR.to_string()));
                bc.error(span, msg);
            } else if bc.check_value(default, Some(ty)) == Ty::Error {
                continue;
            } else if let Some(span) = struct_field_read(default, bc.t) {
                bc.error(span, DEFAULT_EXPR);
            } else if builds_owning(default, bc.t) {
                bc.error(default.span, "field defaults can't build a value that owns an `Arena` or a GPU object; set the field when you build it");
            }
        }
    }
}

/// Whether the checked default `e` builds a value that owns an `Arena` or a GPU object, other
/// than a fresh `Arena()`.
fn builds_owning(e: &Expr, t: &Tables) -> bool {
    let any = |args: &[Arg]| args.iter().any(|a| builds_owning(&a.value, t));
    match &e.kind {
        ExprKind::Call { name, .. } if name == "Arena" => false,
        ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => t.is_owning(t.ty(e.id)) || any(args),
        // A constant is built again at each use, so naming an owning one builds one. One that is
        // just an `Arena` is a fresh `Arena()` each time, like writing it out.
        ExprKind::Ident(_) => matches!(t.res.get(&e.id), Some(Res::Const(_))) && t.ty(e.id) != Ty::Arena && t.is_owning(t.ty(e.id)),
        // A written construction or call (`Holder[int](v: 1)`) is tested like the `Call` it stands for.
        ExprKind::TypeCall { args, .. } if user_type_call(e) => t.is_owning(t.ty(e.id)) || any(args),
        ExprKind::TypeCall { args, .. } => any(args),
        ExprKind::Unary { expr, .. } => builds_owning(expr, t),
        ExprKind::Binary { lhs, rhs, .. } => builds_owning(lhs, t) || builds_owning(rhs, t),
        ExprKind::ArrayLit(items) => items.iter().any(|i| builds_owning(i, t)),
        ExprKind::MapLit(entries) => entries.iter().any(|(k, v)| builds_owning(k, t) || builds_owning(v, t)),
        ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
        _ => false,
    }
}

impl<'a> BodyChecker<'a> {
    /// `Point(1, y: 2)`: arguments bind to fields like parameters; defaults fill the rest.
    pub(crate) fn check_construct(&mut self, e: &Expr, sid: StructId, args: &[Arg]) -> Ty {
        self.check_construct_with(e, sid, args, None)
    }

    /// `check_construct`, where `checked` (when given) holds the type each argument was already
    /// checked to (a vector make checks its values first, to tell its forms apart; inference
    /// checks and moves them in source order, `infer`), so they are only finished here.
    pub(crate) fn check_construct_with(&mut self, e: &Expr, sid: StructId, args: &[Arg], checked: Option<&[Ty]>) -> Ty {
        self.t.res.insert(e.id, Res::Construct(sid));
        let info = self.t.strukt(sid).clone();
        let names: Vec<&str> = info.fields.iter().map(|f| f.name.as_str()).collect();
        let (slots, bound) = self.bind_args(&info.name, &names, args, "field");
        let mut ok = bound;
        // In source order, the order they run in: a move must come after every read.
        for (ai, a) in args.iter().enumerate() {
            let Some(fi) = slots.iter().position(|s| *s == Some(ai)) else {
                if checked.is_none() {
                    self.check_expr(&a.value, None);
                }
                continue;
            };
            if a.is_var {
                self.error(a.span, "constructor arguments can't be `var`");
            }
            let field_ty = info.fields[fi].ty;
            self.going_into(&a.value, super::narrow::Site::Field, |c| match checked {
                Some(tys) => c.finish_value(&a.value, tys[ai], Some(field_ty)),
                None => c.check_value(&a.value, Some(field_ty)),
            });
            // Pre-checked arguments were moved as they were checked (`infer`; a vector make's
            // numbers move nothing).
            if checked.is_none() {
                self.consume(&a.value);
            }
        }
        // `vec2()` and `ivec3()` are zero vectors, like Godot's `Vector2()`.
        let vector = self.t.vector(Ty::Struct(sid)).is_some();
        let zero_vector = vector && args.is_empty();
        let missing: Vec<String> = slots
            .iter()
            .enumerate()
            .filter(|&(fi, slot)| slot.is_none() && !info.fields[fi].has_default && !zero_vector)
            .map(|(fi, _)| format!("`{}`", info.fields[fi].name))
            .collect();
        if bound && !self.missing_fields(e, &info.name, &missing, vector) {
            ok = false;
        }
        if ok {
            self.t.ctor_args.insert(e.id, slots);
        }
        Ty::Struct(sid)
    }

    /// Reports the fields (written with backticks) a make of `owner` leaves out, if any; a vector
    /// gets help on its forms. Returns whether none are missing.
    pub(super) fn missing_fields(&mut self, e: &Expr, owner: &str, missing: &[String], vector: bool) -> bool {
        let msg = match missing {
            [] => return true,
            [one] => format!("missing field {one} for `{owner}`"),
            [init @ .., last] => format!("missing fields {} and {last} for `{owner}`", init.join(", ")),
        };
        if vector {
            self.error_help(e.span, msg, format!("give every component, one value for all of them, or none for zeros: `{owner}()`"));
        } else {
            self.error(e.span, msg);
        }
        false
    }

    /// `base.name`; `want` is the expected type, which a generic enum's variant may take its
    /// types from (P8).
    pub(crate) fn check_field(&mut self, e: &Expr, want: Option<Ty>) -> Ty {
        let ExprKind::Field { base, name, name_span } = &e.kind else { unreachable!("check_field on non-field") };
        // A generic enum's variant (`Maybe[int].Nothing`) comes before today's bare-name test.
        if let Some(ty) = self.generic_variant_value(e, base, name, *name_span, want) {
            return ty;
        }
        if let Some(id) = self.enum_named(base) {
            return self.variant_value(e, id, name, *name_span);
        }
        let base_ty = self.check_field_base(base);
        if matches!(base_ty, Ty::Array(_) | Ty::List(_) | Ty::Map(..)) {
            return self.collection_field(base_ty, name, *name_span);
        }
        if base_ty == Ty::Str {
            return self.str_field(e, name, *name_span);
        }
        if self.method_value_error(base_ty, name, *name_span) {
            return Ty::Error;
        }
        if self.t.vector(base_ty).is_some_and(|k| k.simd) {
            return self.simd_lane(e, base_ty, name, *name_span);
        }
        let (owner, fields) = match base_ty {
            Ty::Struct(sid) => {
                let info = self.t.strukt(sid);
                (info.name.clone(), &info.fields)
            }
            Ty::Variant(id, v) => (self.tn(base_ty), &self.t.enm(id).variants[v as usize].fields),
            Ty::Error => return Ty::Error,
            Ty::Param(p) => return self.param_field_error(base, p, name, *name_span),
            Ty::Optional(_) => {
                let msg = format!("{} may be `none`, so it has no field `{name}`", self.tn(base_ty));
                self.error_help(*name_span, msg, "unwrap it first: `let x = ... else { ... }`");
                return Ty::Error;
            }
            other => {
                let msg = format!("{} has no field `{name}`", self.tn(other));
                self.error(*name_span, msg);
                return Ty::Error;
            }
        };
        match fields.iter().position(|f| &f.name == name) {
            Some(i) => {
                let ty = fields[i].ty;
                self.t.field_index.insert(e.id, i as u32);
                ty
            }
            None => match self.swizzle(base_ty, name) {
                Some((comps, ty)) => {
                    self.t.swizzles.insert(e.id, comps);
                    ty
                }
                None => {
                    self.error(*name_span, format!("`{owner}` has no field `{name}`"));
                    Ty::Error
                }
            },
        }
    }

    /// `receiver.name(args)`; `want` is the expected type, which a generic enum's variant may take
    /// its types from.
    pub(crate) fn check_method_call(&mut self, e: &Expr, want: Option<Ty>) -> Ty {
        let ExprKind::MethodCall { receiver, name, name_span, args } = &e.kind else { unreachable!("not a method call") };
        if let Some(ty) = self.generic_variant_call(e, receiver, name, *name_span, args, want) {
            return ty;
        }
        if let Some(id) = self.enum_named(receiver) {
            return self.variant_construct(e, id, name, *name_span, args);
        }
        let receiver_ty = self.check_expr(receiver, None);
        let builtin = if matches!(receiver_ty, Ty::Array(_) | Ty::List(_)) {
            Some(self.list_method(e, receiver, receiver_ty, args))
        } else if let Ty::Map(..) = receiver_ty {
            Some(self.map_method(e, receiver, receiver_ty, args))
        } else if receiver_ty == Ty::Str {
            Some(self.str_method(e, name, *name_span, args))
        } else {
            None
        };
        if let Some(ty) = builtin {
            if self.reads_receiver_first(e) {
                let later: Vec<&Expr> = args.iter().map(|a| &a.value).collect();
                self.check_changed_later(&[receiver], &later);
            }
            return ty;
        }
        if let (Some(_), Some(b)) = (self.t.vector(receiver_ty), Builtin::vector_method(name)) {
            return self.check_vector_method(e, b, receiver, receiver_ty, args);
        }
        // `b.on_click()` calls a field of function type, when no method has that name.
        if let Some((field, sig)) = self.fn_field(receiver_ty, name) {
            return self.check_field_call(e, receiver, name, field, sig, args);
        }
        let mut rule_call = None;
        let found = match receiver_ty {
            // A `T` has the methods its rules have.
            Ty::Param(p) => self.param_method(p, name).map(|(f, res)| {
                rule_call = Some(res);
                f
            }),
            Ty::Struct(sid) => {
                let info = self.t.strukt(sid);
                info.methods.get(name).copied().ok_or_else(|| Some(format!("`{}` has no method `{name}`", info.name)))
            }
            Ty::Enum(id) => {
                let info = self.t.enm(id);
                info.methods.get(name).copied().ok_or_else(|| Some(format!("`{}` has no method `{name}`", info.name)))
            }
            Ty::Iface(id) => {
                self.iface_method(id, name).map(|(f, _)| f).ok_or_else(|| Some(format!("`{}` has no method `{name}`", self.t.iface(id).name)))
            }
            Ty::Error => Err(None),
            other => Err(Some(format!("{} has no method `{name}`", self.tn(other)))),
        };
        let fid = match found {
            Ok(fid) => fid,
            Err(msg) => {
                if let Some(msg) = msg {
                    self.error(*name_span, msg);
                }
                for a in args {
                    self.check_expr(&a.value, None);
                }
                return Ty::Error;
            }
        };
        // A call through an interface value names the method by its slot in the method table.
        let res = match receiver_ty {
            Ty::Iface(iface) => Res::IfaceCall { iface, slot: self.iface_method(iface, name).expect("found above").1 },
            _ => rule_call.unwrap_or(Res::Fn(fid)),
        };
        // A method declared without `self` has no receiver to take (and nothing calls it yet).
        if self.t.func(fid).params.first().is_none_or(|p| p.name != "self") {
            self.no_self_method(fid, name, *name_span, receiver_ty, args);
            return Ty::Error;
        }
        self.t.res.insert(e.id, res);
        self.check_callee_purity(*name_span, fid);
        let by_pointer = self.t.func(fid).params[0].is_var || receiver_ty.is_aggregate();
        let mut recv = Receiver { expr: receiver, var_root: None, read_root: None };
        if self.t.func(fid).params[0].is_var {
            recv.var_root = self.receiver_root(receiver, name);
            if recv.var_root.is_some() && self.map_loop_inside(receiver) {
                let msg = format!("can't call `{name}` on `{}` while looping over a map inside it; call it after the loop", written(receiver));
                self.error(receiver.span, msg);
            }
        } else if receiver_ty.is_aggregate() {
            recv.read_root = self.place_root(receiver);
        }
        // A method of an instance (or of its own declaration) is checked with its types filled in.
        let sig = self.method_sig(e, fid, receiver_ty);
        let ret = self.check_args(e, sig, args, Some(recv), None);
        if by_pointer {
            let later: Vec<&Expr> = args.iter().map(|a| &a.value).collect();
            self.check_changed_later(&[receiver], &later);
        }
        ret
    }

    /// A call of a method declared without `self`: it has no receiver to take.
    fn no_self_method(&mut self, fid: FnId, name: &str, name_span: Span, receiver_ty: Ty, args: &[Arg]) {
        let owner = match receiver_ty {
            Ty::Struct(sid) => self.t.strukt(sid).name.clone(),
            Ty::Enum(id) => self.t.enm(id).name.clone(),
            other => self.tn(other),
        };
        // An instance of a generic one (`Box[int]`) is declared as `Box`.
        let owner = owner.split('[').next().unwrap_or_default().to_string();
        let more = if self.t.func(fid).params.is_empty() { "" } else { ", ..." };
        let msg = format!(
            "`{name}` has no `self` parameter, so it can't be called on a value; make `self` its first parameter, like `{name}(self{more})`, or move it out of `{owner}` as a plain function"
        );
        self.error(name_span, msg);
        for a in args {
            self.check_expr(&a.value, None);
        }
    }

    /// Whether the built-in method call `e` takes its receiver before it evaluates the arguments,
    /// and reads through it after, so an argument may not change the receiver. A string receiver
    /// is a value (its pointer and length), and a list method that searches, joins or calls a
    /// function takes the list's header (or, for `sort_by`, its address). `add`, `insert`,
    /// `remove_at` and the map methods evaluate their arguments first and find the receiver's place
    /// after (the methods with no arguments have nothing to check).
    fn reads_receiver_first(&self, e: &Expr) -> bool {
        match self.t.res.get(&e.id) {
            Some(Res::Str(_)) => true,
            Some(Res::List(m)) => !matches!(m, ListMethod::Insert | ListMethod::RemoveAt),
            _ => false,
        }
    }

    /// The variable a `var self` call changes, once it's known it may change.
    pub(super) fn receiver_root(&mut self, receiver: &Expr, method: &str) -> Option<LocalId> {
        let Some(root) = self.place_root(receiver) else {
            let msg = format!("`{method}` changes its receiver, so it needs a variable, not a temporary");
            self.error(receiver.span, msg);
            return None;
        };
        self.check_not_matched(receiver);
        let name = self.t.local(root).name.clone();
        let immutable = self.immutable(root);
        // One of a loop's own variables says why it can't change (see `loop_vars`).
        if let (Some(_), Some((n, why, help))) = (immutable, self.loop_var_reason(receiver)) {
            self.error_help(receiver.span, format!("can't call `{method}` on `{n}`: {why}"), help);
            return None;
        }
        let msg = match immutable {
            None => {
                self.mark_changed(root);
                self.check_par_write(receiver);
                return Some(root);
            }
            Some(Immutable::Let) => format!("can't call `{method}` on `{name}`: it changes `{name}`, which was declared with `let`"),
            Some(Immutable::Param) => format!("can't call `{method}` on parameter `{name}`: it changes `{name}`, which isn't `var`"),
            Some(Immutable::SelfParam) => {
                format!("can't call `{method}` on `self`: it changes `self`, but the method takes `self`, not `var self`")
            }
            Some(Immutable::Captured) => Self::captured_message(&name),
        };
        self.error(receiver.span, msg);
        None
    }
}
