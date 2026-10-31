//! The reloadable shape (hot reload design, decision 1): a program with no `main` and with
//! `pub setup() -> State` and `pub frame(var s: State) -> bool`, free functions of the entry file.
//! The reloading host calls them itself; everything else runs the `main` KIR generates for them
//! (`let s = setup()`, then `while frame(var s) {}`). `State` is any struct a reload can keep in
//! place: no `Arena`, function value or interface value inside (`reload_fault`).

use kestrel_syntax::ast::Program;
use kestrel_syntax::{Diagnostic, Span};

use crate::snapshot::{reload_fault, FaultKind};
use crate::tables::{FnId, Tables};
use crate::ty::Ty;

/// A reloadable program's two functions and the type of the state `setup` makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReloadShape {
    pub setup: FnId,
    pub frame: FnId,
    pub state: Ty,
}

const NEEDS_BOTH: &str = "a reloadable program needs both `pub setup() -> State` and `pub frame(var s: State) -> bool`";

/// Decides what kind of program this is: a `main` program, or a reloadable one (`t.reload`), or
/// neither (an error). A library (`needs_main` false) is none of them.
pub(crate) fn check_entry(program: &Program, t: &mut Tables, diags: &mut Vec<Diagnostic>, needs_main: bool) {
    if !needs_main {
        return;
    }
    // Only the entry file's own free functions have these plain names (methods and other
    // modules' items are named `Type.name` and `module.name`).
    let find = |name: &str| t.fn_by_name.get(name).copied().filter(|&id| t.func(id).method.is_none());
    let (setup, frame) = (find("setup"), find("frame"));
    let is_pub = |id: FnId| t.func(id).decl(program).is_pub;
    if t.main.is_some() {
        if let (Some(s), Some(f)) = (setup, frame) {
            if is_pub(s) && is_pub(f) {
                diags.push(Diagnostic::error(t.func(s).span, "a program has `main` or `setup` and `frame`, not both"));
            }
        }
        return;
    }
    // Private functions called `setup` or `frame` are only helpers: the program lacks a `main`.
    if !setup.into_iter().chain(frame).any(is_pub) {
        diags.push(Diagnostic::error(Span::new(0, 0), "program needs a `main()` function"));
        return;
    }
    let (s, f) = match (setup, frame) {
        (Some(s), Some(f)) => (s, f),
        (one, other) => {
            let id = one.or(other).expect("one of them");
            diags.push(Diagnostic::error(t.func(id).span, NEEDS_BOTH));
            return;
        }
    };
    let errors = diags.len();
    for id in [s, f] {
        if !is_pub(id) {
            let name = &t.func(id).name;
            diags.push(Diagnostic::error(t.func(id).span, format!("`{name}` must be `pub` in a reloadable program; write `pub {name}`")));
        }
    }
    let (si, fi) = (t.func(s), t.func(f));
    let state = si.ret;
    let state_ok = matches!(state, Ty::Struct(_)) && si.params.is_empty() && si.type_params.is_empty();
    let state_name = if matches!(state, Ty::Struct(_)) { t.ty_name(state) } else { "State".to_string() };
    if !state_ok {
        let msg = format!("`setup` takes no parameters and returns the state, a struct: write `pub setup() -> {state_name}`");
        diags.push(Diagnostic::error(si.span, msg));
    }
    let frame_ok = fi.params.len() == 1 && fi.params[0].is_var && fi.ret == Ty::Bool && fi.type_params.is_empty();
    if !frame_ok {
        let msg = format!("`frame` takes the state as one `var` parameter and returns `bool`: write `pub frame(var s: {state_name}) -> bool`");
        diags.push(Diagnostic::error(fi.span, msg));
    } else if state_ok && fi.params[0].ty != state {
        let (p, other) = (&fi.params[0].name, t.ty_name(fi.params[0].ty));
        let msg = format!("`frame` takes the state `setup` makes: `var {p}: {state_name}`, not `var {p}: {other}`");
        diags.push(Diagnostic::error(fi.span, msg));
    }
    if state_ok {
        let root = if frame_ok { fi.params[0].name.as_str() } else { "s" };
        if let Some(fault) = reload_fault(t, state, root) {
            let (holds, keep, part) = match fault.kind {
                FaultKind::Fn => ("a function value", "functions", "the function value"),
                FaultKind::Arena => ("an `Arena`", "arenas", "the `Arena`"),
                FaultKind::Iface => ("an interface value", "interface values", "the interface value"),
                FaultKind::Gpu => unreachable!("a reload keeps GPU objects"),
            };
            let msg = format!("`setup`'s state can't hold {holds}; keep {keep} out of the state");
            diags.push(Diagnostic::error(si.span, msg).with_help(format!("{part} is `{}`", fault.path)));
        }
    }
    // The host finds the generated `reload_drop` by that name among the exports.
    if let Some(own) = find("reload_drop").filter(|&id| is_pub(id)) {
        diags.push(Diagnostic::error(t.func(own).span, "`reload_drop` is a name the compiler uses; pick another"));
    }
    if diags.len() == errors {
        t.reload = Some(ReloadShape { setup: s, frame: f, state });
    }
}
