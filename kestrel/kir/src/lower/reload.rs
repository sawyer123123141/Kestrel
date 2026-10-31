//! What lowering adds for a reloadable program (hot reload design, decisions 1, 3 and 4): the
//! generated `main` that everything but the reloading host runs (`let s = setup()`, then
//! `while frame(var s) {}`, then the state's drop, as the `main` written out would), the exported
//! `reload_drop` that queues the state's GPU objects when a reload resets it, and `ReloadInfo`.

use kestrel_check::{ReloadShape, Tables};

use super::bytes_fingerprint::description;
use super::helpers::{expr, Helpers};
use super::kty;
use crate::ir::*;

/// The name of the generated `reload_drop`. Its export drops the compiler's prefix, so a host
/// finds it as `reload_drop` (see `kestrel_codegen::export`).
pub const RELOAD_DROP: &str = "__kestrel_reload_drop";

/// Adds `reload_drop` and the generated `main` to `helpers`, after the user's functions;
/// returns the `main`. (The checker refuses a `pub reload_drop` of the program's own.)
pub(super) fn generate(shape: &ReloadShape, t: &Tables, helpers: &mut Helpers) -> FuncIdx {
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
    let setup_hash = super::reload_hash::setup_hash(m, FuncIdx(shape.setup.0));
    ReloadInfo { state_description: description(state, t), setup_hash, state_size, state_align }
}

