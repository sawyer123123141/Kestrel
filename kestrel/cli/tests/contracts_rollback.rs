//! The rollback library's contracts at the command line: a game proves its frame allocation-free
//! with a one-line wrapper around `Session.advance`, which fails for a game whose state holds a
//! list, and a session whose game is an interface value is refused inside the library.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fresh directory holding `files` (name, text).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("contracts_rollback_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        fs::write(dir.join(file), text).unwrap();
    }
    dir
}

fn kestrel(args: &[&str], file: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kestrel")).args(args).arg(file).output().expect("run kestrel")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n")
}

/// A game with `state` as its fields and `body` as its `step`, and the wrapper that proves a frame.
fn game(state: &str, body: &str, start: &str) -> String {
    format!(
        "use {{ Input, Rollback, Session, new_session from rollback }}
use {{ Rng from random }}
struct Game: Rollback {{
{state}
    pure step(var self, one: Input, two: Input) {{
{body}
    }}
}}
@prove(deterministic, no_alloc)
pure advance_peer(var p: Session[Game]) {{
    p.advance()
}}
main() {{
    var s = new_session({start}, me: 1, delay: 0)
    s.local(Input(buttons: 1))
    advance_peer(var s)
    print(s.now)
}}
"
    )
}

/// The line of `session.kes` (the library's part) that stores the present state in the ring.
fn ring_line() -> usize {
    let src = include_str!("../../driver/std/rollback/session.kes");
    1 + src.lines().position(|l| l.trim() == "self.ring[self.now % RING] = self.game").expect("the ring store")
}

#[test]
fn a_plain_games_frame_is_proven_allocation_free() {
    let src = game(
        "    pos: vec2\n    score: int\n    rng: Rng",
        "        self.pos += vec2(one.x, two.x)\n        self.score += one.buttons + self.rng.randi_range(0, 3)",
        "Game(pos: vec2(0, 0), score: 0, rng: Rng(seed: 1))",
    );
    let dir = project("plain", &[("main.kes", &src)]);
    for args in [&["check"][..], &["check", "--release"]] {
        let out = kestrel(args, &dir.join("main.kes"));
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{args:?}");
    }
    let out = kestrel(&["run"], &dir.join("main.kes"));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"), "1\n");
}

#[test]
fn a_game_holding_a_list_fails_no_alloc_at_the_ring_store() {
    let src = game("    xs: list[int]", "        self.xs.add(one.buttons)", "Game(xs: [])");
    let dir = project("list", &[("main.kes", &src)]);
    let out = kestrel(&["check"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let expected = format!(
        "error: `advance_peer` can't be proven `no_alloc`: it allocates on rollback/session.kes:{}: the assignment copies `p.game.xs` to a new list when the list there has too little room",
        ring_line()
    );
    assert!(stderr(&out).contains(&expected), "{}", stderr(&out));
}

#[test]
fn a_session_of_an_interface_value_is_refused_in_the_library() {
    // An interface meets its own rule, so `G` may be one; the library's `checksum` refuses it.
    let src = "use { Input, Rollback, new_session from rollback }
struct Game: Rollback {
    x: int
    pure step(var self, one: Input, two: Input) {
        self.x += one.buttons
    }
}
main() {
    let g: Rollback = Game(x: 3)
    var s = new_session[Rollback](g, me: 1, delay: 0)
    s.advance()
}
";
    let dir = project("interface", &[("main.kes", src)]);
    let out = kestrel(&["check"], &dir.join("main.kes"));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("error: `checksum` can't take an interface value"), "{err}");
    assert!(err.contains("--> <builtin>/rollback/session.kes:"), "{err}");
    assert!(err.contains("help: in the copy `rollback.Session[rollback.Rollback].confirm`"), "{err}");
    assert!(err.ends_with("1 error\n"), "{err}");
}
