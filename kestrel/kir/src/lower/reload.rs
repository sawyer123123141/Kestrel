//! What lowering adds for a reloadable program (hot reload design, decisions 1, 3 and 4): the
//! generated `main` that everything but the reloading host runs (`let s = setup()`, then
//! `while frame(var s) {}`, then the state's drop, as the `main` written out would), the exported
//! `reload_drop` that queues the state's GPU objects when a reload resets it, and `ReloadInfo`.

use kestrel_check::{ReloadShape, Tables};

use super::bytes_fingerprint::{checksum_of_text, description};
use super::helpers::{expr, Helpers};
use super::kty;
use crate::ir::*;
use crate::visit_mut::{walk_expr, walk_stmt, VisitMut};

/// The name of the generated `reload_drop`. Its export drops the compiler's prefix, so a host
/// finds it as `reload_drop` (see `kestrel_codegen::export`).
pub(crate) const RELOAD_DROP: &str = "__kestrel_reload_drop";

/// Adds `reload_drop` and the generated `main` to `helpers` (the user's functions are `funcs`);
/// returns the `main`. A user's own `pub reload_drop` stops being exported, so the host's lookup
/// by name finds only the generated one.
pub(super) fn generate(shape: &ReloadShape, t: &Tables, funcs: &mut [Func], helpers: &mut Helpers) -> FuncIdx {
    for f in funcs.iter_mut().filter(|f| f.name == "reload_drop") {
        f.exported = false;
    }
    let state = kty(shape.state);
    let s = Place::local(LocalIdx(0));
    let drop_body = helpers.drop_stmts(s.clone(), state, t);
    let drop = Func {
        name: RELOAD_DROP.to_string(),
        params: vec![LocalIdx(0)],
        ret: None,
        locals: vec![Local { name: "s".into(), ty: state, kind: LocalKind::RefParam }],
        body: drop_body,
        is_pure: false,
        io: false,
        mutates_params: true,
        exported: true,
        lines: (0, 0),
        native: None,
    };
    place(helpers, drop);
    // The user's functions come first, in the checker's order, so a `FnId` is a `FuncIdx`.
    let (setup, frame) = (FuncIdx(shape.setup.0), FuncIdx(shape.frame.0));
    let mut body = vec![
        Stmt::Let(LocalIdx(0), expr(ExprKind::Call(setup, Vec::new()), state)),
        Stmt::While { cond: expr(ExprKind::Call(frame, vec![Arg::Ref(s.clone())]), KTy::Bool), body: Vec::new() },
    ];
    body.extend(helpers.drop_stmts(s, state, t));
    let main = Func {
        name: "main".to_string(),
        params: Vec::new(),
        ret: None,
        locals: vec![Local { name: "s".into(), ty: state, kind: LocalKind::Addressed }],
        body,
        is_pure: false,
        io: true,
        mutates_params: false,
        exported: false,
        lines: (0, 0),
        native: None,
    };
    place(helpers, main)
}

/// Adds `f` as the next helper.
fn place(helpers: &mut Helpers, f: Func) -> FuncIdx {
    let idx = helpers.reserve();
    helpers.funcs[(idx.0 - helpers.base) as usize] = f;
    idx
}

/// The state's description and layout, and the hash of `setup`, of a lowered module `m`.
pub(super) fn info(shape: &ReloadShape, t: &Tables, m: &Module) -> ReloadInfo {
    let state = kty(shape.state);
    let (state_size, state_align) = m.size_align(state);
    let setup_hash = checksum_of_text(setup_text(m, FuncIdx(shape.setup.0)).as_bytes()) as u64;
    ReloadInfo { state_description: description(state, t), setup_hash, state_size, state_align }
}

/// `setup`'s locals and body as text, without lines and with each index into the module (a
/// function, struct, enum or element type) written as what it names, so the text changes when
/// `setup` does and not when code around it moves.
fn setup_text(m: &Module, setup: FuncIdx) -> String {
    let f = &m.funcs[setup.0 as usize];
    let mut body = f.body.clone();
    NoLines.visit_stmts(&mut body);
    let mut text = format!("{:?} {:?} {:?}", f.ret, f.locals, body);
    // An element type may name another; each pass writes out one level.
    while text.contains("ElemIdx(") {
        text = replace_ids(&text, "ElemIdx(", |i| format!("Elem[{:?}]", m.elems[i]));
    }
    text = replace_ids(&text, "StructIdx(", |i| format!("Struct[{}]", m.structs[i].name));
    text = replace_ids(&text, "EnumIdx(", |i| format!("Enum[{}]", m.enums[i].name));
    replace_ids(&text, "FuncIdx(", |i| format!("Func[{}]", stable_name(&m.funcs[i].name)))
}

/// A function's name without the index a generated one ends with (`__kestrel_copy_57`).
fn stable_name(name: &str) -> &str {
    match name.strip_prefix("__kestrel_") {
        Some(_) => name.trim_end_matches(|c: char| c.is_ascii_digit()),
        None => name,
    }
}

/// `text` with each `<prefix><n>)` written as `name(n)`.
fn replace_ids(text: &str, prefix: &str, name: impl Fn(usize) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(prefix) {
        out.push_str(&rest[..at]);
        let after = &rest[at + prefix.len()..];
        let digits = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
        match after[..digits].parse::<usize>() {
            Ok(n) if after[digits..].starts_with(')') => {
                out.push_str(&name(n));
                rest = &after[digits + 1..];
            }
            _ => {
                out.push_str(prefix);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Sets every line in the code it visits to 0.
struct NoLines;

impl VisitMut for NoLines {
    fn visit_stmt(&mut self, s: &mut Stmt) {
        if let Stmt::SetLine(line) | Stmt::Call(_, _, line) = s {
            *line = 0;
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &mut Expr) {
        e.line = 0;
        walk_expr(self, e);
    }
}
