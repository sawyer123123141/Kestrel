//! The reloadable shape (hot reload design, decision 1): a program with `pub setup() -> State` and
//! `pub frame(var s: State) -> bool` and no `main`, and the rule for what its state may hold.

use kestrel_check::{check, reload_fault, FaultKind, Tables};
use kestrel_syntax::parse;

/// Checks `src`; with `builtin` the whole file counts as a built-in module (so it may name
/// `GpuHandle`).
fn run(src: &str, builtin: bool) -> (Option<Tables>, Vec<String>) {
    let (mut p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    if builtin {
        p.builtin_ranges = std::iter::once(0..src.len() as u32).collect();
    }
    let (t, d) = check(&p);
    (t, d.into_iter().filter(|d| d.is_error()).map(|d| d.message).collect())
}

const SHAPE: &str = "struct Game {
    n: int
}
pub setup() -> Game {
    Game(n: 0)
}
pub frame(var s: Game) -> bool {
    s.n += 1
    s.n < 3
}
";

#[test]
fn a_setup_and_frame_program_needs_no_main() {
    let (t, errors) = run(SHAPE, false);
    assert!(errors.is_empty(), "{errors:?}");
    let t = t.expect("tables");
    let shape = t.reload.as_ref().expect("the reloadable shape");
    assert_eq!(t.func(shape.setup).name, "setup");
    assert_eq!(t.func(shape.frame).name, "frame");
    assert_eq!(t.ty_name(shape.state), "Game");
    assert!(t.main.is_none());
}

#[test]
fn a_main_program_has_no_reload_shape() {
    let src = format!("{SHAPE}main() {{}}\n").replace("pub setup", "setup");
    let (t, errors) = run(&src, false);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(t.expect("tables").reload.is_none());
    // Helpers called `setup` and `frame` in a `main` program are only functions.
    let src = format!("{SHAPE}main() {{}}\n").replace("pub ", "");
    assert!(run(&src, false).1.is_empty());
}

#[test]
fn the_shape_rules_are_refused_with_a_sentence() {
    let both = format!("{SHAPE}main() {{}}\n");
    assert_eq!(run(&both, false).1, ["a program has `main` or `setup` and `frame`, not both"]);
    let only_setup = "struct Game {\n    n: int\n}\npub setup() -> Game {\n    Game(n: 0)\n}\n";
    let needs = "a reloadable program needs both `pub setup() -> State` and `pub frame(var s: State) -> bool`";
    assert_eq!(run(only_setup, false).1, [needs]);
    let other = SHAPE.replace("pub frame(var s: Game)", "pub frame(var s: World)") + "struct World {\n    n: int\n}\n";
    assert_eq!(run(&other, false).1, ["`frame` takes the state `setup` makes: `var s: Game`, not `var s: World`"]);
    let private = SHAPE.replace("pub frame", "frame");
    assert_eq!(run(&private, false).1, ["`frame` must be `pub` in a reloadable program; write `pub frame`"]);
    let not_var = SHAPE.replace("frame(var s: Game)", "frame(s: Game)").replace("    s.n += 1\n", "");
    let frame_shape = "`frame` takes the state as one `var` parameter and returns `bool`: write `pub frame(var s: Game) -> bool`";
    assert_eq!(run(&not_var, false).1, [frame_shape]);
    let no_bool = SHAPE.replace(") -> bool", ")").replace("    s.n < 3\n", "");
    assert_eq!(run(&no_bool, false).1, [frame_shape]);
    let setup_shape = "`setup` takes no parameters and returns the state, a struct: write `pub setup() -> Game`";
    let with_param = SHAPE.replace("pub setup()", "pub setup(n: int)").replace("Game(n: 0)", "Game(n: n)");
    assert_eq!(run(&with_param, false).1, [setup_shape]);
    let not_struct = "pub setup() -> int {\n    0\n}\npub frame(var s: int) -> bool {\n    false\n}\n";
    let setup_shape = "`setup` takes no parameters and returns the state, a struct: write `pub setup() -> State`";
    assert_eq!(run(not_struct, false).1, [setup_shape]);
}

#[test]
fn the_state_may_hold_gpu_objects_but_not_functions_arenas_or_interfaces() {
    let decls = "struct Mesh {
    id: GpuHandle
    n: int
}
interface Shape {
    area(self) -> float
}
struct Good {
    meshes: list[Mesh?]
    one: Mesh?
    n: int
}
struct WithFn {
    n: int
    on_hit: (int) -> int
}
struct WithArena {
    mem: Arena
}
struct WithShape {
    shape: Shape
}
main() {}
";
    let (t, errors) = run(decls, true);
    assert!(errors.is_empty(), "{errors:?}");
    let t = t.expect("tables");
    let ty = |name: &str| kestrel_check::Ty::Struct(t.struct_by_name[name]);
    assert_eq!(reload_fault(&t, ty("Good"), "s"), None);
    let fault = reload_fault(&t, ty("WithFn"), "s").expect("a function value");
    assert_eq!((fault.path.as_str(), fault.kind), ("s.on_hit", FaultKind::Fn));
    assert_eq!(reload_fault(&t, ty("WithArena"), "s").map(|f| f.kind), Some(FaultKind::Arena));
    assert_eq!(reload_fault(&t, ty("WithShape"), "s").map(|f| f.kind), Some(FaultKind::Iface));
}

#[test]
fn a_state_holding_what_a_reload_cant_keep_is_refused() {
    let with_fn = "struct Game {
    n: int
    on_hit: (int) -> int
}
pure twice(n: int) -> int { n * 2 }
pub setup() -> Game {
    Game(n: 0, on_hit: twice)
}
pub frame(var s: Game) -> bool {
    false
}
";
    let (_, errors) = run(with_fn, false);
    assert_eq!(errors, ["`setup`'s state can't hold a function value; keep functions out of the state"]);
    let with_arena = "struct Game {
    mem: Arena
}
pub setup() -> Game {
    Game(mem: Arena())
}
pub frame(var s: Game) -> bool {
    false
}
";
    let (_, errors) = run(with_arena, false);
    assert_eq!(errors, ["`setup`'s state can't hold an `Arena`; keep arenas out of the state"]);
}
