use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering};

pub const PANIC_OVERFLOW: i32 = 1;
pub const PANIC_DIV_ZERO: i32 = 2;
/// A narrowing conversion (`int(x)`, `u8(x)`) got a value the target can't hold.
pub const PANIC_NARROW_INT: i32 = 3;
pub const PANIC_NARROW_U8: i32 = 4;
/// `x << n` / `x >> n` with `n` negative or at least the type's width.
pub const PANIC_SHIFT: i32 = 5;
/// A loop over a map found its entry gone: the body replaced the map.
pub const PANIC_MAP_CHANGED: i32 = 6;
/// A bodiless interface method was called (the compiler never emits such a call).
pub const PANIC_ABSTRACT: i32 = 7;
/// An index outside its list or array. Only `kestrel_rt_proof_failed` takes this kind (a bounds
/// failure reports itself with `kestrel_rt_panic_index`).
pub const PANIC_BOUNDS: i32 = 8;

static FAILURE_HOOK: std::sync::OnceLock<fn(&str) -> !> = std::sync::OnceLock::new();

/// Makes runtime failures (overflow, bad index, ...) call `hook` instead of
/// ending the process: for hosts that embed Kestrel, like the Godot add-on.
/// `hook` must not return. The first hook set stays.
pub fn set_failure_hook(hook: fn(&str) -> !) {
    let _ = FAILURE_HOOK.set(hook);
}

/// Flushes output, then hands `msg` to the failure hook, or reports it and
/// exits with code 101.
pub fn fail(msg: &str) -> ! {
    // Inside a `par for`: a pool thread hands the message to the caller and parks; the caller waits for
    // the other threads and reports the failure with the lowest index, which may not be this one.
    let msg = crate::par::fail_in_job(msg).unwrap_or_else(|| msg.to_string());
    let msg = msg.as_str();
    crate::print::flush();
    // A recording saves its file, with this failure last; a replay compares it and reports.
    if let Some(code) = crate::replay::on_failure(msg) {
        std::process::exit(code);
    }
    if let Some(hook) = FAILURE_HOOK.get() {
        hook(msg);
    }
    eprintln!("kestrel: {msg}");
    std::process::exit(101);
}

/// The line of the latest call from a program's own code to a built-in function
/// that isn't `pure` (0 before the first). Compiled code stores it just before such
/// a call, so a native's error can name the line. Only the window's thread runs
/// those calls.
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static kestrel_rt_call_line: AtomicI32 = AtomicI32::new(0);

/// A source file's name as compiled code passes it to a runtime error: static data holding a
/// pointer to the name's bytes and its length. Compiled code passes null when it has no name.
#[repr(C)]
pub struct FileName {
    ptr: *const u8,
    len: i64,
}

/// " of geom.kes" for an error in that file, or nothing for null.
pub(crate) fn of_file(file: *const FileName) -> String {
    if file.is_null() {
        return String::new();
    }
    // SAFETY: compiled code passes a pointer to its static `FileName`, whose bytes are static too.
    let name = unsafe { std::slice::from_raw_parts((*file).ptr, (*file).len as usize) };
    format!(" of {}", String::from_utf8_lossy(name))
}

/// The file of the call whose line is in `kestrel_rt_call_line`, stored with it.
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static kestrel_rt_call_file: AtomicPtr<FileName> = AtomicPtr::new(std::ptr::null_mut());

/// Forgets the latest call's line and file. The reloading host calls this when it swaps in a new
/// module, so a failure before the new code's first call names no line rather than one of the old
/// code's, and the file pointer, which points into the old module, is never read.
pub fn reset_call_site() {
    kestrel_rt_call_line.store(0, Ordering::Relaxed);
    kestrel_rt_call_file.store(std::ptr::null_mut(), Ordering::Relaxed);
}

/// Fails with `msg` and the line of the call into the built-in function that is
/// failing: "draw.circle was called before window.open on line 12".
///
/// A native that a `pure` built-in function can reach must use plain `fail` instead: calls to a
/// `pure` function never store the line, so the one here would be stale (the line of some earlier
/// call), and the `pure` function may be running on a `par for` thread.
pub fn fail_at_call(msg: &str) -> ! {
    match kestrel_rt_call_line.load(Ordering::Relaxed) {
        0 => fail(msg),
        line => fail(&format!("{msg} on line {line}{}", of_file(kestrel_rt_call_file.load(Ordering::Relaxed)))),
    }
}

#[no_mangle]
pub extern "C" fn kestrel_rt_panic_index(index: i64, len: i64, line: i32, file: *const FileName) -> ! {
    fail(&format!("index {index} out of bounds for length {len} on line {line}{}", of_file(file)));
}

#[no_mangle]
pub extern "C" fn kestrel_rt_panic(kind: i32, line: i32, file: *const FileName) -> ! {
    let what = match kind {
        PANIC_OVERFLOW => "integer overflow",
        PANIC_DIV_ZERO => "division by zero",
        PANIC_NARROW_INT => "value doesn't fit in int",
        PANIC_NARROW_U8 => "value doesn't fit in u8",
        PANIC_SHIFT => "shift amount out of range",
        PANIC_MAP_CHANGED => "a map was replaced while looping over it",
        PANIC_ABSTRACT => "an interface method with no body was called",
        _ => "runtime error",
    };
    fail(&format!("{what} on line {line}{}", of_file(file)));
}

/// The message for a check range analysis removed (`KESTREL_CHECK_PROOFS=1` puts it back) that fails.
/// `kind` is a `PANIC_*` kind; no `file` (empty) leaves out " of {file}".
pub fn proof_failed_message(kind: i32, line: i32, file: &str) -> String {
    let what = match kind {
        PANIC_OVERFLOW => "overflow",
        PANIC_SHIFT => "shift",
        PANIC_DIV_ZERO => "division",
        PANIC_NARROW_INT | PANIC_NARROW_U8 => "narrowing",
        PANIC_BOUNDS => "bounds",
        _ => "runtime",
    };
    let of = if file.is_empty() { String::new() } else { format!(" of {file}") };
    format!("range analysis removed the {what} check on line {line}{of}, but it fails; this is a compiler bug")
}

/// A check range analysis proved can't fail, and kept for `KESTREL_CHECK_PROOFS=1`, failed.
#[no_mangle]
pub extern "C" fn kestrel_rt_proof_failed(kind: i32, line: i32, file: *const FileName) -> ! {
    let name = of_file(file);
    fail(&proof_failed_message(kind, line, name.strip_prefix(" of ").unwrap_or("")));
}

/// Fails with the message in the `len` bytes at `msg`: a compiled error message with no line (the
/// copy function of an owning type's method table).
///
/// # Safety
/// `msg` points at `len` bytes of UTF-8 that stay valid (compiled code passes static data).
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_fail(msg: *const u8, len: i64) -> ! {
    // SAFETY: the caller passes `len` readable bytes at `msg`.
    let bytes = unsafe { std::slice::from_raw_parts(msg, len as usize) };
    fail(&String::from_utf8_lossy(bytes));
}

/// The message for an `@assume` condition that is false: "assumption `n <= xs.len` is false on
/// line 9 of f.kes". No `file` (empty) leaves out " of {file}".
pub fn assume_failed_message(text: &str, line: i32, file: &str) -> String {
    let of = if file.is_empty() { String::new() } else { format!(" of {file}") };
    format!("assumption `{text}` is false on line {line}{of}")
}

/// An `@assume` condition, whose text is the `len` bytes at `text`, is false.
///
/// # Safety
/// `text` points at `len` bytes of UTF-8 that stay valid (compiled code passes static data), and
/// `file` is null or points at a static `FileName`.
#[no_mangle]
pub unsafe extern "C" fn kestrel_rt_assume_failed(text: *const u8, len: i64, line: i32, file: *const FileName) -> ! {
    // SAFETY: the caller passes `len` readable bytes at `text`.
    let bytes = unsafe { std::slice::from_raw_parts(text, len as usize) };
    let name = of_file(file);
    fail(&assume_failed_message(&String::from_utf8_lossy(bytes), line, name.strip_prefix(" of ").unwrap_or("")));
}

#[no_mangle]
pub extern "C" fn kestrel_rt_fmod_f32(a: f32, b: f32) -> f32 {
    // Kestrel's own remainder, like its other math (see `math.rs`): Rust's `%` calls the C runtime.
    // The wide remainder is exact, so it equals `libm::fmodf`'s.
    crate::math::wide::fmodf(a, b)
}

#[no_mangle]
pub extern "C" fn kestrel_rt_fmod_f64(a: f64, b: f64) -> f64 {
    libm::fmod(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_proof_names_its_check_line_and_file() {
        assert_eq!(
            proof_failed_message(PANIC_BOUNDS, 12, "world.kes"),
            "range analysis removed the bounds check on line 12 of world.kes, but it fails; this is a compiler bug"
        );
        assert_eq!(
            proof_failed_message(PANIC_NARROW_U8, 3, ""),
            "range analysis removed the narrowing check on line 3, but it fails; this is a compiler bug"
        );
        let kinds = [(PANIC_OVERFLOW, "overflow"), (PANIC_SHIFT, "shift"), (PANIC_DIV_ZERO, "division"), (PANIC_NARROW_INT, "narrowing")];
        for (kind, word) in kinds {
            assert!(proof_failed_message(kind, 1, "a.kes").contains(&format!("removed the {word} check")));
        }
    }

    #[test]
    fn a_false_assumption_names_its_text_line_and_file() {
        assert_eq!(assume_failed_message("n <= xs.len", 9, "f.kes"), "assumption `n <= xs.len` is false on line 9 of f.kes");
        assert_eq!(assume_failed_message("i >= 0", 3, ""), "assumption `i >= 0` is false on line 3");
    }

    #[test]
    fn resetting_the_call_site_forgets_the_line_and_the_file() {
        let mut name = FileName { ptr: b"old.kes".as_ptr(), len: 7 };
        kestrel_rt_call_line.store(12, Ordering::Relaxed);
        kestrel_rt_call_file.store(&mut name, Ordering::Relaxed);
        reset_call_site();
        assert_eq!(kestrel_rt_call_line.load(Ordering::Relaxed), 0);
        assert!(kestrel_rt_call_file.load(Ordering::Relaxed).is_null());
    }

    #[test]
    fn fmod_matches_rust_remainder() {
        assert_eq!(kestrel_rt_fmod_f32(7.5, 2.0), 1.5);
        assert_eq!(kestrel_rt_fmod_f64(-7.5, 2.0), -1.5);
    }
}
