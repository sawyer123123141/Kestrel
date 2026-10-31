//! A reloadable program's module (hot reload design, decisions 1, 3 and 4): the generated `main`,
//! `ReloadInfo` (the state's description and layout, the hash of `setup`) and `reload_drop`.

use kestrel_driver::{compile_unoptimized, DiagStyle};
use kestrel_kir::*;

/// The module as lowering left it (the inliner would fold `setup` and `frame` into `main`).
fn module(src: &str) -> Module {
    let (c, text) = compile_unoptimized("game.kes", src, DiagStyle::Short, true);
    c.unwrap_or_else(|| panic!("{text}")).module
}

fn func<'m>(m: &'m Module, name: &str) -> &'m Func {
    m.funcs.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function `{name}`"))
}

const GAME: &str = "use { Texture from draw }

struct Game {
    tex: Texture?
    n: int
    v: vec4
}

pub setup() -> Game {
    Game(tex: none, n: 0, v: vec4(0.0, 0.0, 0.0, 0.0))
}

pub frame(var s: Game) -> bool {
    s.n += 1
    s.n < 3
}
";

#[test]
fn reload_info_describes_the_state() {
    let m = module(GAME);
    let info = m.reload.as_ref().expect("a reloadable program has `ReloadInfo`");
    assert_eq!(info.state_description, "struct Game{tex:struct Texture{id:gpu,width:int,height:int}?,n:int,v:vec4}");
    // The `vec4` sets the alignment.
    assert_eq!((info.state_size, info.state_align), (48, 16));
    // A `main` program has none.
    let main = GAME.replace("pub setup", "setup").replace("pub frame", "frame") + "main() {\n    var s = setup()\n    while frame(var s) {}\n}\n";
    assert!(module(&main).reload.is_none());
}

#[test]
fn the_setup_hash_follows_setup_only() {
    let base = module(GAME).reload.unwrap().setup_hash;
    // `frame` changed: the same hash.
    let frame_changed = GAME.replace("s.n < 3", "s.n < 30");
    assert_eq!(module(&frame_changed).reload.unwrap().setup_hash, base);
    // A function and a struct added before `setup` move its lines and every index: the same hash.
    let moved = GAME.replace("struct Game {", "struct Extra {\n    a: list[int]\n}\n\npure helper(x: int) -> int { x + 1 }\n\nstruct Game {");
    assert_eq!(module(&moved).reload.unwrap().setup_hash, base);
    // `setup` changed: another hash.
    let setup_changed = GAME.replace("n: 0, v:", "n: 5, v:");
    assert_ne!(module(&setup_changed).reload.unwrap().setup_hash, base);
}

#[test]
fn a_main_is_generated_that_runs_setup_then_frame_until_false() {
    let m = module(GAME);
    let main = &m.funcs[m.main.expect("a generated `main`").0 as usize];
    assert_eq!(main.name, "main");
    assert_eq!(main.lines, (0, 0), "generated code has no lines");
    assert!(!main.exported);
    let (setup, frame) = (m.funcs.iter().position(|f| f.name == "setup").unwrap(), m.funcs.iter().position(|f| f.name == "frame").unwrap());
    let text = format!("{:?}", main.body);
    assert!(text.contains(&format!("Call(FuncIdx({setup}), [])")), "{text}");
    assert!(text.contains("While {"), "{text}");
    assert!(text.contains(&format!("Call(FuncIdx({frame}), [Ref(")), "{text}");
    // `setup` and `frame` are exported, so the host can call them.
    assert!(func(&m, "setup").exported && func(&m, "frame").exported);
}

#[test]
fn reload_drop_drops_the_states_gpu_objects() {
    let m = module(GAME);
    let drop = func(&m, "__kestrel_reload_drop");
    assert!(drop.exported);
    assert_eq!(drop.params.len(), 1);
    let s = &drop.locals[drop.params[0].0 as usize];
    assert_eq!(s.kind, LocalKind::RefParam);
    assert!(matches!(s.ty, KTy::Struct(_)));
    let text = format!("{:?}", drop.body);
    assert!(text.contains("DropGpu"), "{text}");
}

fn hash(src: &str) -> u64 {
    module(src).reload.expect("a reloadable program").setup_hash
}

#[test]
fn the_setup_hash_reads_the_code_not_its_text() {
    // String literals that look like the debug text of an index are only strings: no hang, no
    // panic, and the hash still sees them.
    let tricky = GAME.replace("pub setup() -> Game {\n", "pub setup() -> Game {\n    print(\"ElemIdx(\")\n    print(\"StructIdx(999)\")\n");
    let base = hash(&tricky);
    assert_ne!(hash(&tricky.replace("StructIdx(999)", "StructIdx(998)")), base);
    // Code moved down by blank lines and a comment: the same hash.
    let moved = tricky.replace("pub setup", "// the state\n\n\npub setup");
    assert_eq!(hash(&moved), base);
}

const TWO_STRUCTS: &str = "struct Pos {
    x: int
    y: int
}

struct Game {
    at: Pos
    n: int
}

pub setup() -> Game {
    Game(at: Pos(x: 1, y: 2), n: 0)
}

pub frame(var s: Game) -> bool {
    s.n += 1
    s.n < 3
}
";

#[test]
fn the_setup_hash_ignores_the_order_of_declarations() {
    let base = hash(TWO_STRUCTS);
    let pos = "struct Pos {\n    x: int\n    y: int\n}\n\n";
    let swapped = TWO_STRUCTS.replace(pos, "").replace("pub setup", &format!("{pos}pub setup"));
    assert_ne!(swapped, TWO_STRUCTS);
    assert_eq!(hash(&swapped), base);
    // A constant inside `setup` changed: another hash.
    assert_ne!(hash(&TWO_STRUCTS.replace("x: 1, y: 2", "x: 1, y: 3")), base);
}

const WITH_CONSTANTS: &str = "let LEVELS = [10, 20, 30]
let OTHER = [4, 5]

struct Game {
    n: int
}

pub setup() -> Game {
    Game(n: LEVELS[1])
}

pub frame(var s: Game) -> bool {
    s.n += OTHER[0]
    s.n < 100
}
";

#[test]
fn the_setup_hash_names_the_heap_constants_setup_reads() {
    let base = hash(WITH_CONSTANTS);
    // Reformatted, with a function before `setup` that reads the other constant first: `LEVELS`
    // gets another global, and the hash stays.
    let reformatted = WITH_CONSTANTS
        .replace("struct Game {", "pure first() -> int {\n    OTHER[1]\n}\n\n\nstruct Game {")
        .replace("pub frame", "\n// one frame\npub frame");
    assert_eq!(hash(&reformatted), base);
    // Another constant read: another hash.
    assert_ne!(hash(&WITH_CONSTANTS.replace("LEVELS[1]", "OTHER[1]")), base);
}
