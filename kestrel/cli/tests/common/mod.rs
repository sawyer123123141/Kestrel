//! Helpers shared by the CLI test binaries.
#![allow(dead_code)]

pub mod counter;
pub mod directives;
pub mod session;
pub mod smoke;

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Only Windows builds of `kestrel` have the window (raylib) for now. Elsewhere a test that needs
/// one is ignored with `#[cfg_attr(not(windows), ignore = "no window on this platform")]`, and a
/// test that runs many programs skips the window ones (`no_window`).
pub const HAS_WINDOW: bool = cfg!(windows);

/// The start of what `kestrel` says when it refuses a window program because it has no window.
pub const NO_WINDOW: &str = "this build of kestrel has no window";

/// Whether `out` is `kestrel` refusing a program because this build has no window: the test skips
/// that program. Never on Windows, whose build has the window.
pub fn no_window(out: &Output) -> bool {
    !HAS_WINDOW && out.status.code() == Some(2) && String::from_utf8_lossy(&out.stderr).contains(NO_WINDOW)
}

/// Prints which programs a test skipped because this build has no window.
pub fn report_skipped(test: &str, skipped: &[String]) {
    if !skipped.is_empty() {
        println!("{test}: skipped {} window programs, no window on this platform: {}", skipped.len(), skipped.join(", "));
    }
}

/// Takes every `KESTREL_*` variable out of the child's environment, so nothing left in the
/// shell that runs `cargo test` (`KESTREL_HEADLESS`, `KESTREL_HIDDEN`, `KESTREL_THREADS`,
/// `KESTREL_ARENA_REPORT`, ...) changes what a test sees. Then it sets `KESTREL_POISON=1`, so
/// every program a test runs fills freed arena memory with 0xDB and never reuses it (a use of
/// freed memory then fails the test instead of reading stale bytes). Set the other ones a test
/// wants after this: a later `env` on the same variable wins, so a program turns poisoning off
/// with `// env: KESTREL_POISON=0` (and says why in a comment).
pub fn hermetic(cmd: &mut Command) -> &mut Command {
    scrub(cmd, std::env::vars_os().map(|(key, _)| key)).env("KESTREL_POISON", "1")
}

/// `hermetic`, for these variable names (the ones the current process has).
pub fn scrub(cmd: &mut Command, names: impl Iterator<Item = OsString>) -> &mut Command {
    for name in names.filter(|n| is_kestrel_variable(n)) {
        cmd.env_remove(name);
    }
    cmd
}

/// Names are case-insensitive on Windows, so `kestrel_hidden` is the same variable.
fn is_kestrel_variable(name: &OsStr) -> bool {
    name.to_string_lossy().to_ascii_uppercase().starts_with("KESTREL_")
}

/// Runs the command with no input, killing it after `limit`. `None` when it timed out.
pub fn output_within(cmd: &mut Command, limit: Duration) -> Option<Output> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("run the command");
    // Read both pipes on threads, so a program that prints a lot can't block on a full pipe.
    fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    }
    let (stdout, stderr) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for the command") {
            break Some(status);
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
    status.map(|status| Output { status, stdout, stderr })
}
