//! `print` output is buffered until the program ends, but `window.frame()` flushes it. A
//! frame loop that runs for minutes must show its lines while it runs, which is how the
//! particles demo reports its times.
//!
//! The program runs a headless window for far more frames than the test will wait for, and
//! the test kills it after reading its first line.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

mod common;

const SOURCE: &str = "use window\n\nmain() {\n    window.open(64, 64, \"flush\")\n    print(\"running\")\n    while window.frame() {\n    }\n}\n";

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn print_output_shows_up_while_the_frame_loop_runs() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("window_print_flush");
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("flush.kes");
    fs::write(&source, SOURCE).unwrap();

    let mut child = common::hermetic(Command::new(env!("CARGO_BIN_EXE_kestrel")).arg("run").arg(&source))
        .env("KESTREL_HEADLESS", "4000000000")
        .env("KESTREL_HEADLESS_QUIET", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let first = rx.recv_timeout(Duration::from_secs(30));
    child.kill().unwrap();
    child.wait().unwrap();
    match first {
        Ok(line) => assert_eq!(line.trim_end(), "running"),
        Err(_) => panic!("the program printed nothing in 30 seconds: `print` output isn't flushed by window.frame()"),
    }
}
