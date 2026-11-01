//! Hot reload under `kestrel run` when a save can't be swapped in at once: a compile error, which
//! keeps the old code running until a fix; a save the editor is still writing; and a module that
//! only the edit uses, broken or not written yet. The programs are the counter of `reload.rs`
//! (`common/counter.rs`).

use std::fs;

mod common;

use common::counter::{begin, counter, follow, start};

/// The counter with constant `k` and a type error holding `text` on line 23.
fn broken(k: i64, text: &str) -> String {
    let bad = format!("    time.sleep(0.005)\n    let oops: int = \"{text}\"\n");
    counter(k).replace("    time.sleep(0.005)\n", &bad)
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_compile_error_keeps_the_old_code_then_a_fix_reloads() {
    let mut s = start("error", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&broken(2, "broken"));
    s.wait_err("still running the previous code; fix the error and save again");
    assert!(s.errs.iter().any(|l| l.contains("error")), "the diagnostics are printed:\n{}", s.transcript());
    let last = follow(&mut s, last, 2, 2, 10);
    s.edit(&counter(5));
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 5, 5);
    // The error was reported once, not on every poll.
    assert_eq!(s.errs.iter().filter(|l| l.contains("still running")).count(), 1, "{}", s.transcript());
}

#[cfg(windows)]
#[test]
fn a_save_still_being_written_is_waited_for() {
    // An editor that holds the file shut while it writes: the host waits for it, and reports no
    // error that the finished file doesn't have.
    let mut s = start("held", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit_held(&counter(3), std::time::Duration::from_millis(600));
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 3, 3);
    assert!(!s.errs.iter().any(|l| l.contains("still running") || l.contains("can't read")), "{}", s.transcript());
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn an_error_after_a_reload_reports_the_new_file_and_keeps_the_latest_good_code() {
    // Review Focus 2.
    let mut s = start("error_after_reload", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&counter(3));
    s.wait_err("reloaded prog.kes in ");
    let last = follow(&mut s, last, 2, 3, 3);
    s.edit(&broken(3, "after the reload"));
    s.wait_err("still running the previous code; fix the error and save again");
    let errs = s.errs.join("\n");
    assert!(errs.contains("after the reload") && errs.contains(":23:"), "the error is in the new file:\n{errs}");
    // The latest good code (K = 3) keeps running, not the first (K = 2).
    follow(&mut s, last, 3, 3, 10);
}

/// The counter with its value times `helper.k()`, from the module `helper.kes` beside it.
fn with_helper() -> String {
    counter(2)
        .replace("use time\n", "use time\nuse helper\n")
        .replace("    print(\"frame {s.n} value {s.n * K}\")\n", "    let v = s.n * K * helper.k()\n    print(\"frame {s.n} value {v}\")\n")
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_module_only_a_broken_edit_uses_is_watched() {
    let mut s = start("helper_broken", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    let helper = s.path.with_file_name("helper.kes");
    fs::write(&helper, "pub k() -> int {\n    \"three\"\n}\n").unwrap();
    s.edit(&with_helper());
    s.wait_err("still running the previous code; fix the error and save again");
    assert!(s.errs.iter().any(|l| l.contains("helper.kes")), "the error is in the module:\n{}", s.transcript());
    let last = follow(&mut s, last, 2, 2, 5);
    // Only the module is fixed: the program file is not saved again.
    s.edit_file(&helper, "pub k() -> int {\n    3\n}\n");
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 6, 5);
    assert_eq!(s.errs.iter().filter(|l| l.contains("still running")).count(), 1, "{}", s.transcript());
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn a_module_written_after_the_edit_that_uses_it_reloads() {
    let mut s = start("helper_missing", &counter(2), 100_000, &[]);
    let last = begin(&mut s, 2, 3);
    s.edit(&with_helper());
    s.wait_err("still running the previous code; fix the error and save again");
    let last = follow(&mut s, last, 2, 2, 5);
    let helper = s.path.with_file_name("helper.kes");
    s.edit_file(&helper, "pub k() -> int {\n    3\n}\n");
    s.wait_err("reloaded prog.kes in ");
    follow(&mut s, last, 2, 6, 5);
}
