//! Constants: top-level `let NAME = ...`, whose initializers may only use
//! literals, operators, pure built-ins, struct and enum values and other
//! constants (in any file, in any order, as long as they don't loop).

use std::collections::HashMap;

use kestrel_syntax::ast::{Arg, Block, ConstDecl, Expr, ExprKind, InterpPart, Item, Program, Stmt};
use kestrel_syntax::{Diagnostic, Span};

use super::generic_call::user_type_call;
use super::BodyChecker;
use crate::builtins::Builtin;
use crate::tables::{ConstId, Tables};
use crate::ty::Ty;

pub(crate) fn check_consts(program: &Program, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let consts: Vec<&ConstDecl> = program
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Const(c) => Some(c),
            Item::Fn(_) | Item::Struct(_) | Item::Enum(_) | Item::Interface(_) | Item::Use(_) => None,
        })
        .collect();
    // A constant is checked after the constants it uses, whichever file they are in
    // (`palette.SKY = draw.BLUE` works whatever the order of the `use` lines). Where
    // nothing decides, a module's constants (`draw.RED`) come before the entry
    // file's, and each group keeps its source order.
    let in_module = |c: &&ConstDecl| c.bind.name.contains('.');
    let mut roots: Vec<usize> = (0..consts.len()).filter(|&i| in_module(&consts[i])).collect();
    roots.extend((0..consts.len()).filter(|&i| !in_module(&consts[i])));
    let ids = consts.iter().enumerate().map(|(i, c)| (c.bind.name.as_str(), i)).collect();
    let mut order = Order { marks: vec![Mark::Todo; consts.len()], stack: Vec::new(), consts, ids };
    for root in roots {
        order.visit(root, t, diags);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Todo,
    /// On the stack: its constants are being checked first.
    Active,
    Done,
}

/// A depth-first walk over which constant uses which.
struct Order<'a> {
    consts: Vec<&'a ConstDecl>,
    ids: HashMap<&'a str, usize>,
    marks: Vec<Mark>,
    /// The constants being visited, outermost first.
    stack: Vec<usize>,
}

impl Order<'_> {
    fn visit(&mut self, i: usize, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
        if self.marks[i] != Mark::Todo {
            return;
        }
        self.marks[i] = Mark::Active;
        self.stack.push(i);
        let mut uses = Vec::new();
        names_used(&self.consts[i].value, &mut uses);
        for (name, span) in uses {
            let Some(&j) = self.ids.get(name.as_str()) else { continue };
            match self.marks[j] {
                Mark::Todo => self.visit(j, t, diags),
                Mark::Active => self.report_cycle(j, span, t, diags),
                Mark::Done => {}
            }
        }
        self.stack.pop();
        self.marks[i] = Mark::Done;
        check_const(self.consts[i], i, t, diags);
        t.const_order.push(ConstId(i as u32));
    }

    /// `back` is on the stack and the constant at the top uses it: a loop. The
    /// constants in it get the error type, so their uses don't add more errors.
    fn report_cycle(&mut self, back: usize, span: Span, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
        let from = self.stack.iter().position(|&k| k == back).expect("an active constant is on the stack");
        let cycle = &self.stack[from..];
        let name = |k: usize| self.consts[k].bind.name.as_str();
        let msg = if cycle.len() == 1 {
            format!("constant `{}` uses itself; give it a value that doesn't", name(back))
        } else {
            let path: Vec<&str> = cycle.iter().chain([&back]).map(|&k| name(k)).collect();
            format!("constants can't use each other in a loop: {}; give one of them a value that doesn't use the others", path.join(" -> "))
        };
        diags.push(Diagnostic::error(span, msg));
        for &k in cycle {
            t.consts[k].ty = Ty::Error;
            t.const_by_name.insert(self.consts[k].bind.name.clone(), ConstId(k as u32));
        }
    }
}

/// The names `e` mentions, each once with where it first appears: the
/// constants it uses, when `e` is a constant expression (see `is_const_expr`). A closure's body
/// is walked whole: it runs later, but the constants it names must be checked first.
fn names_used(e: &Expr, out: &mut Vec<(String, Span)>) {
    let args = |args: &[Arg], out: &mut Vec<(String, Span)>| args.iter().for_each(|a| names_used(&a.value, out));
    match &e.kind {
        ExprKind::Ident(name) => {
            if !out.iter().any(|(n, _)| n == name) {
                out.push((name.clone(), e.span));
            }
        }
        // A constant of function type is called by its name.
        ExprKind::Call { name, args: a, .. } => {
            if !out.iter().any(|(n, _)| n == name) {
                out.push((name.clone(), e.span));
            }
            args(a, out);
        }
        ExprKind::TypeCall { args: a, .. } => args(a, out),
        ExprKind::MethodCall { receiver, args: a, .. } => {
            names_used(receiver, out);
            args(a, out);
        }
        ExprKind::CallValue { callee, args: a } => {
            names_used(callee, out);
            args(a, out);
        }
        ExprKind::Unary { expr, .. } => names_used(expr, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            names_used(lhs, out);
            names_used(rhs, out);
        }
        ExprKind::Field { base, .. } => names_used(base, out),
        ExprKind::Index { base, index } => {
            names_used(base, out);
            names_used(index, out);
        }
        ExprKind::ArrayLit(items) => items.iter().for_each(|i| names_used(i, out)),
        ExprKind::MapLit(entries) => entries.iter().for_each(|(k, v)| {
            names_used(k, out);
            names_used(v, out);
        }),
        ExprKind::Interp(parts) => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    names_used(x, out);
                }
            }
        }
        ExprKind::If { cond, then_block, else_block } => {
            names_used(cond, out);
            names_in_block(then_block, out);
            if let Some(b) = else_block {
                names_in_block(b, out);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            names_used(scrutinee, out);
            arms.iter().for_each(|a| names_in_block(&a.body, out));
        }
        ExprKind::Closure(c) => names_in_block(&c.body, out),
        ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
        // A type applied names no constant (an enum's variants come through its `Field`/`MethodCall`).
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) | ExprKind::TypeApply { .. } => {}
    }
}

fn names_in_block(b: &Block, out: &mut Vec<(String, Span)>) {
    for s in &b.stmts {
        match s {
            Stmt::Let { value, else_block, .. } => {
                names_used(value, out);
                if let Some(b) = else_block {
                    names_in_block(b, out);
                }
            }
            Stmt::Assign { target, value, .. } => {
                names_used(target, out);
                names_used(value, out);
            }
            Stmt::While { cond, body, .. } => {
                names_used(cond, out);
                names_in_block(body, out);
            }
            Stmt::ForRange { start, end, body, .. } => {
                names_used(start, out);
                names_used(end, out);
                names_in_block(body, out);
            }
            Stmt::ForIn { iter, body, .. } => {
                names_used(iter, out);
                names_in_block(body, out);
            }
            Stmt::Arena { body, .. } => names_in_block(body, out),
            Stmt::In { arena, body, .. } => {
                names_used(arena, out);
                names_in_block(body, out);
            }
            Stmt::Return { value: Some(v), .. } | Stmt::Expr(v) => names_used(v, out),
            Stmt::Assume { cond, .. } => names_used(cond, out),
            Stmt::Return { value: None, .. } | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Remove { .. } => {}
        }
    }
}

fn check_const(c: &ConstDecl, i: usize, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    let mut bc = BodyChecker::new(t, diags, None, Ty::Unit, false);
    let want = c.ty.as_ref().map(|ty| bc.resolve(ty));
    let ty = if !is_const_expr(&c.value, bc.t) {
        let (span, msg) = super::stop_in_constant(&c.value).unwrap_or((c.value.span, CONST_EXPR.to_string()));
        bc.error(span, msg);
        Ty::Error
    } else {
        let ty = bc.check_value(&c.value, want);
        match struct_field_read(&c.value, bc.t) {
            Some(span) if ty != Ty::Error => {
                bc.error(span, CONST_EXPR);
                Ty::Error
            }
            _ => ty,
        }
    };
    t.consts[i].ty = ty;
    t.const_by_name.insert(c.bind.name.clone(), ConstId(i as u32));
}

/// Whether `e` is a constant expression: literals, operators, constants, pure built-ins, struct
/// and enum values, and array and map literals of those. Constants and field defaults are built
/// again where they're used, so both take these (field defaults also can't build a value that owns
/// an arena or a GPU object: see `check_field_defaults`; loops are found by `check_default_cycles`).
pub(crate) fn is_const_expr(e: &Expr, t: &Tables) -> bool {
    let all = |args: &[Arg]| args.iter().all(|a| is_const_expr(&a.value, t));
    // `State.Idle` and `Shape.Circle(1)`: an enum's values.
    // (A generic enum with its types written, `Maybe[int].Nothing`, is one too.)
    let names_enum = |base: &Expr| names_enum(base, t);
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) | ExprKind::Ident(_) => true,
        ExprKind::Call { name, args, .. } if name == "Arena" => args.is_empty(),
        // `fail` and `exit` never give a value (`stop_in_constant` says so).
        ExprKind::Call { name, .. } if Builtin::global(name).is_some_and(Builtin::never_returns) => false,
        // Pure built-ins (`sqrt(2.0)`) are fine; the random ones change state.
        ExprKind::Call { name, args, .. } if Builtin::global(name).is_some_and(Builtin::is_pure) => all(args),
        // A struct construction (`Color(1, 0, 0)`, `vec2(0, 1)`) is a value like any other. A
        // capitalised name that isn't a function is one meant to be (an unknown struct, an enum, a
        // constant): `check_call` says what's wrong with it.
        ExprKind::Call { name, args, .. } if t.struct_by_name.contains_key(name) || is_simd_name(name, t) || names_no_fn(name, t) => all(args),
        // A component or swizzle of a constant vector (`ORIGIN.x`, `ORIGIN.xy`); the base's type
        // decides it, once checked (`struct_field_read` refuses any other field).
        ExprKind::Field { base, .. } => names_enum(base) || is_const_expr(base, t),
        ExprKind::MethodCall { receiver, args, .. } => names_enum(receiver) && all(args),
        ExprKind::Unary { expr, .. } => is_const_expr(expr, t),
        ExprKind::Binary { lhs, rhs, .. } => is_const_expr(lhs, t) && is_const_expr(rhs, t),
        // Each use builds a fresh array, like any other value.
        ExprKind::ArrayLit(items) => items.iter().all(|i| is_const_expr(i, t)),
        ExprKind::MapLit(entries) => entries.iter().all(|(k, v)| is_const_expr(k, t) && is_const_expr(v, t)),
        // The built-in `array[T](n)`, `list[T]()` and `map[K, V]()`, and a generic struct's
        // construction (`Stack[int]()`), are values; a generic function's call is not (P17).
        ExprKind::TypeCall { name, args, .. } => (!user_type_call(e) || t.struct_by_name.contains_key(name)) && all(args),
        // A generic named with types (`Maybe[int]`) is constant like a bare name (G19).
        ExprKind::TypeApply { .. } => true,
        ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
        // A closure is a value; its body runs only when it is called.
        ExprKind::Closure(_) => true,
        ExprKind::Interp(_)
        | ExprKind::Index { .. }
        | ExprKind::Call { .. }
        | ExprKind::CallValue { .. }
        | ExprKind::If { .. }
        | ExprKind::Match { .. } => false,
    }
}

/// Whether `base` names an enum, with or without types written (`State`, `Maybe[int]`).
fn names_enum(base: &Expr, t: &Tables) -> bool {
    matches!(&base.kind, ExprKind::Ident(n) | ExprKind::TypeApply { name: n, .. } if t.enum_by_name.contains_key(n))
}

/// Whether calling `name` can't be a function call: it's capitalised, an enum or a constant, and
/// no function has that name.
fn names_no_fn(name: &str, t: &Tables) -> bool {
    let capitalised = name.starts_with(|c: char| c.is_ascii_uppercase());
    let other = t.enum_by_name.contains_key(name) || t.consts.iter().any(|c| c.name == name);
    (capitalised || other) && !t.fn_by_name.contains_key(name)
}

/// The error for a constant that isn't a constant expression.
pub(crate) const CONST_EXPR: &str =
    "constants can only use literals, operators, other constants, struct and enum values, and pure built-ins like `sqrt`";

/// The first field read in the checked constant expression `e` that isn't a component or swizzle
/// of a built-in vector (`ORIGIN.x`, `ORIGIN.xy`) or an enum value (`State.Idle`): a constant
/// can't read a struct's field, as it never could (`P.x` with `P` a user struct is refused).
pub(crate) fn struct_field_read(e: &Expr, t: &Tables) -> Option<Span> {
    let in_args = |args: &[Arg]| args.iter().find_map(|a| struct_field_read(&a.value, t));
    match &e.kind {
        ExprKind::Field { base, .. } => {
            if names_enum(base, t) {
                None
            } else if t.vector(t.ty(base.id)).is_some() || t.ty(base.id) == Ty::Error {
                struct_field_read(base, t)
            } else {
                Some(e.span)
            }
        }
        ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } | ExprKind::TypeCall { args, .. } => in_args(args),
        ExprKind::Unary { expr, .. } => struct_field_read(expr, t),
        ExprKind::Binary { lhs, rhs, .. } => struct_field_read(lhs, t).or_else(|| struct_field_read(rhs, t)),
        ExprKind::ArrayLit(items) => items.iter().find_map(|i| struct_field_read(i, t)),
        ExprKind::MapLit(entries) => entries.iter().find_map(|(k, v)| struct_field_read(k, t).or_else(|| struct_field_read(v, t))),
        ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
        _ => None,
    }
}

/// Whether `name` is a SIMD vector type's (`vec4`), whose make is a value like a struct's.
fn is_simd_name(name: &str, t: &Tables) -> bool {
    Ty::from_name(name).is_some_and(|ty| t.vector(ty).is_some())
}
