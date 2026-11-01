//! The voxel engine's play program, `kestrel/voxel/main.kes`, under hot reload: it runs from a
//! scratch folder holding copies of every `kestrel/voxel/*.kes`, and a test edits a copy while it
//! runs. The engine's own tests are `voxel_engine.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

mod common;

use common::hermetic;
use common::session::Session;

fn voxel_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../voxel")
}

/// The `.kes` files directly in `dir`, sorted.
fn kes_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "kes"))
        .collect();
    files.sort();
    files
}

/// The play program's trace line (`TRACE` on), `trace pos <x> <y> <z> spawned <bool> columns <n>`:
/// the position as text, whether the player spawned, and the columns in use.
fn trace(s: &mut Session) -> (String, bool, i64) {
    let line = loop {
        let line = s.next_out();
        if line.starts_with("trace ") {
            break line;
        }
    };
    let w: Vec<&str> = line.split(' ').collect();
    assert!(w.len() == 9 && w[1] == "pos" && w[5] == "spawned" && w[7] == "columns", "not a trace line: {line:?}");
    let columns = w[8].parse().unwrap_or_else(|_| panic!("not a trace line: {line:?}"));
    (w[2..5].join(" "), w[6] == "true", columns)
}

/// `main.kes` under hot reload: an edit of the walking speed in `player.kes` reloads the program,
/// and the player stays where it stood in the same world (a reset would make a new world, with no
/// columns, and put the player back in the sky at `START`).
#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn an_edit_reloads_the_play_program_and_keeps_the_world() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("voxel_main_reload");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for module in kes_files(&voxel_dir()) {
        fs::copy(&module, dir.join(module.file_name().unwrap())).unwrap();
    }
    // `TRACE` on, each frame paced to 2 ms more so the run lasts while the test edits it.
    let main = dir.join("main.kes");
    let src = fs::read_to_string(&main).unwrap();
    let print = "        print(\"trace pos";
    assert!(src.contains("let TRACE = false") && src.contains(print), "main.kes has no trace line");
    let paced = src.replacen("use window", "use time\nuse window", 1).replace("let TRACE = false", "let TRACE = true");
    fs::write(&main, paced.replace(print, &format!("        time.sleep(0.002)\n{print}"))).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    hermetic(cmd.arg("run").arg(&main)).env("KESTREL_HEADLESS", "100000").env("KESTREL_HEADLESS_QUIET", "1");
    let player = dir.join("player.kes");
    let mut s = Session::start(cmd, &player);
    // Until the player has spawned and stood still for 10 frames.
    let (mut pos, mut still) = (String::new(), 0);
    while still < 10 {
        let (now, spawned, _) = trace(&mut s);
        still = if spawned && now == pos { still + 1 } else { 0 };
        pos = now;
    }
    let (_, _, columns) = trace(&mut s);
    let src = fs::read_to_string(&player).unwrap();
    assert!(src.contains("let WALK = 4.3"), "player.kes has no `let WALK = 4.3`");
    s.edit(&src.replace("let WALK = 4.3", "let WALK = 6.0"));
    let line = s.wait_err("reloaded main.kes in ");
    assert!(line.ends_with(" s"), "{line}");
    // The lines printed before the swap are read at once; a second more of them came after it.
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end {
        let (now, spawned, n) = trace(&mut s);
        assert!(spawned && now == pos && n >= columns, "at {pos} with {columns} columns, then {now} with {n}:\n{}", s.transcript());
    }
    assert!(!s.errs.iter().any(|l| l.contains("state reset") || l.contains("setup()")), "{}", s.transcript());
}
