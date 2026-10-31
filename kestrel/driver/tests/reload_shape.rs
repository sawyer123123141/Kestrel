//! What the reloading host gets for a reloadable program: `compile_as_host` leaves the generated
//! `main` out, and the JIT exports `setup`, `frame` and `reload_drop` as thunks that pass the
//! state by pointer (`setup` writes it into the buffer `frame` then takes, with no copy).

use kestrel_codegen::{ExportTy, JitProgram, Options};
use kestrel_driver::{compile, host::compile_as_host, DiagStyle};

const GAME: &str = "struct Game {
    n: int
    total: i64
    v: vec4
}

pub setup() -> Game {
    Game(n: 0, total: 100, v: vec4(1.0, 2.0, 3.0, 4.0))
}

pub frame(var s: Game) -> bool {
    s.n += 1
    s.total += s.n
    s.n < 3
}
";

/// A 16-byte aligned buffer of `size` bytes (the state may hold a `vec4`).
#[repr(C, align(16))]
struct Block([u8; 16]);

fn buffer(size: u32) -> Vec<Block> {
    (0..size.div_ceil(16)).map(|_| Block([0; 16])).collect()
}

#[test]
fn the_host_compile_has_no_main() {
    let (c, text) = compile_as_host("game.kes", GAME, DiagStyle::Short);
    let c = c.unwrap_or_else(|| panic!("{text}"));
    assert!(c.module.main.is_none());
    assert!(!c.module.funcs.iter().any(|f| f.name == "main"));
    assert!(c.module.reload.is_some());
    // Everything else gets the generated `main`.
    let (c, text) = compile("game.kes", GAME, DiagStyle::Short);
    assert!(c.unwrap_or_else(|| panic!("{text}")).module.main.is_some());
}

#[test]
fn setup_frame_and_reload_drop_are_exported_by_those_names() {
    let (c, text) = compile_as_host("game.kes", GAME, DiagStyle::Short);
    let c = c.unwrap_or_else(|| panic!("{text}"));
    let info = c.module.reload.clone().expect("reload info");
    let jit = JitProgram::compile(&c.module, Options::default()).expect("compiles");
    let exports = jit.exports();
    let names: Vec<&str> = exports.iter().map(|(e, _)| e.name.as_str()).collect();
    assert_eq!(names, ["setup", "frame", "reload_drop"]);
    let game = ExportTy::Other("Game".into());
    let (setup, frame, drop) = (&exports[0], &exports[1], &exports[2]);
    assert_eq!((setup.0.params.clone(), setup.0.ret.clone(), setup.0.ret_size), (vec![], Some(game.clone()), info.state_size));
    assert_eq!((frame.0.params.clone(), frame.0.ret.clone(), frame.0.arg_offsets.clone()), (vec![game.clone()], Some(ExportTy::Bool), vec![0]));
    assert_eq!((drop.0.params.clone(), drop.0.ret.clone()), (vec![game], None));
    assert_eq!((info.state_size, info.state_align), (32, 16));

    // `setup` writes the state into the buffer; `frame` runs on that same buffer in place.
    jit.run_init();
    let mut state = buffer(info.state_size);
    let at = state.as_mut_ptr() as *mut u8;
    // SAFETY: the buffer has the state's size and alignment; the thunks take what `ExportInfo` says.
    unsafe { (setup.1)(std::ptr::null(), at) };
    let mut more = Vec::new();
    for _ in 0..3 {
        let mut go = 0u8;
        unsafe { (frame.1)(at, &mut go) };
        more.push(go);
    }
    assert_eq!(more, [1, 1, 0]);
    // `n` at 0, `total` at 8, `v` at 16.
    let (n, total) = unsafe { (*(at as *const i32), *(at.add(8) as *const i64)) };
    assert_eq!((n, total), (3, 106));
    let lanes = unsafe { std::slice::from_raw_parts(at.add(16) as *const f32, 4) };
    assert_eq!(lanes, [1.0, 2.0, 3.0, 4.0]);
    // Nothing in this state needs dropping; the call must still be safe.
    unsafe { (drop.1)(at, std::ptr::null_mut()) };
}
