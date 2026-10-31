//! Tests of `start_host`, the entry the reloading host runs its game loop through.

use super::start_host;

/// Set in the child process `a_failing_closure_exits_with_101` starts: the child calls `fail`.
const FAIL_CHILD: &str = "KESTREL_START_HOST_FAIL_CHILD";

#[test]
fn the_host_runs_with_the_float_mode_pinned_and_returns_its_code() {
    let code = start_host(|| {
        assert_eq!(crate::fpu::read(), crate::fpu::PINNED);
        assert!(crate::fpu::started(), "the host's thread is marked as running a program");
        7
    });
    assert_eq!(code, 7);
    assert!(!crate::fpu::started(), "only the host's own thread is marked");
}

#[test]
fn the_host_runs_on_a_large_stack() {
    fn recurse(depth: u32) -> u8 {
        let block = [depth as u8; 1 << 20];
        let below = if depth == 0 { 0 } else { recurse(depth - 1) };
        std::hint::black_box(&block);
        below.wrapping_add(block[1 << 10])
    }
    // 64 MiB: far past a default thread stack, well inside ours.
    assert_eq!(
        start_host(|| {
            std::hint::black_box(recurse(64));
            0
        }),
        0
    );
}

#[test]
fn a_runtime_panic_in_the_host_gives_101() {
    assert_eq!(start_host(|| panic!("a bug in the runtime")), 101);
}

/// `fail` ends the process with 101 (as a Kestrel program's failure does), so the test runs it in
/// a child: this same test binary, asked to run only this test, with `FAIL_CHILD` set.
#[test]
fn a_failing_closure_exits_with_101() {
    if std::env::var_os(FAIL_CHILD).is_some() {
        start_host(|| crate::panic::fail("the host failed on purpose"));
        unreachable!("fail returned");
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "start::start_test::a_failing_closure_exits_with_101", "--nocapture", "--test-threads=1"])
        .env(FAIL_CHILD, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(101), "stderr: {stderr}");
    assert!(stderr.contains("kestrel: the host failed on purpose"), "stderr: {stderr}");
}
