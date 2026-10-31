use std::sync::OnceLock;
use std::time::Instant;

/// Stack for the Kestrel program, so deep recursion fits.
const STACK_SIZE: usize = 256 * 1024 * 1024;

/// When the program started: `time.now()` counts from here.
static STARTED: OnceLock<Instant> = OnceLock::new();

/// The program's path and arguments, when the host set them (`kestrel run`).
static ARGS: OnceLock<(String, Vec<String>)> = OnceLock::new();

/// When the program started: the first `kestrel_rt_start`, or the first call of this when a host
/// runs Kestrel code without it. `time.now()` counts from here.
pub fn started() -> Instant {
    *STARTED.get_or_init(Instant::now)
}

/// Gives the program its path and arguments, for `args.program()` and `args.all()`. `kestrel run`
/// calls this before `run_main`, with the `.kes` path and the words meant for the program. A built
/// executable never calls it, so the natives read the process's own command line instead. Only the
/// first call counts.
pub fn set_args(program: String, args: Vec<String>) {
    let _ = ARGS.set((program, args));
}

/// What `set_args` gave, if it was called.
pub fn args() -> Option<&'static (String, Vec<String>)> {
    ARGS.get()
}

/// Runs `entry` and flushes on the calling thread, since output is buffered per thread.
fn run_entry(entry: extern "C" fn()) {
    run_program(|| entry());
}

/// Runs `body` as a program on the calling thread: the float mode pinned and the thread marked
/// first, then the output flushed and the arena reports printed (when asked) once it returns.
fn run_program<R>(body: impl FnOnce() -> R) -> R {
    // Round to nearest and nothing flushed, whatever mode the thread started with; `par for`
    // then pins its caller (this thread) too.
    crate::fpu::pin();
    crate::fpu::mark_started();
    let result = body();
    crate::print::flush();
    // The program arena belongs to this thread, so it is measured here (and only when asked).
    if let Some(report) = arena_report(std::env::var("KESTREL_ARENA_REPORT").ok().as_deref(), crate::arena::program_bytes) {
        eprintln!("{report}");
    }
    if let Some(report) = arena_count_report(std::env::var("KESTREL_ARENA_COUNT").ok().as_deref(), crate::arena::arenas_alive) {
        eprintln!("{report}");
    }
    result
}

/// The line `KESTREL_ARENA_COUNT=1` asks for once `main` has returned: how many arenas made by
/// `Arena()` or `arena { }` are still alive. A program that drops everything it made says 0, so
/// tests use it to tell that an owning value was dropped exactly once. `alive` counts them, and
/// runs only when the report is on.
fn arena_count_report(setting: Option<&str>, alive: impl FnOnce() -> i64) -> Option<String> {
    (setting == Some("1")).then(|| format!("arenas alive at exit: {}", alive()))
}

/// The line `KESTREL_ARENA_REPORT=1` asks for once `main` has returned. Tests use it to
/// tell a program that frees its temporary memory from one that doesn't. `bytes` measures the
/// arena, and runs only when the report is on.
fn arena_report(setting: Option<&str>, bytes: impl FnOnce() -> usize) -> Option<String> {
    (setting == Some("1")).then(|| format!("kestrel: program arena {} bytes", bytes()))
}

/// Whether `KESTREL_POISON` asks for poisoning: only "1" does. Freed arena blocks are then filled
/// with 0xDB and never reused, so the test suite shows a use of freed memory as garbage.
fn poison_setting(setting: Option<&str>) -> bool {
    setting == Some("1")
}

/// Runs a compiled Kestrel `main` on a large stack, then flushes output.
/// Returns the exit code. `KESTREL_POISON` is read here, once, before `entry` runs.
#[no_mangle]
pub extern "C" fn kestrel_rt_start(entry: extern "C" fn()) -> i32 {
    started();
    if !crate::fpu::has_sse41() {
        eprintln!("kestrel: {}", crate::fpu::NO_SSE41);
        return 101;
    }
    start_with(entry, poison_setting(std::env::var("KESTREL_POISON").ok().as_deref()))
}

/// `kestrel_rt_start` with the poison setting given, not read from the environment (the process
/// environment is shared by every thread of a test process; this isn't). The setting is
/// process-wide: arenas made from now on follow it.
pub fn start_with(entry: extern "C" fn(), poison: bool) -> i32 {
    crate::blocks::set_poison(poison);
    on_big_stack(move || {
        run_entry(entry);
        0
    })
}

/// Runs `host` as the program, the way `kestrel_rt_start` runs a compiled `main`: on a 256 MiB
/// thread with the float mode pinned, the output flushed and the arena reports printed after it.
/// `host` returns the exit code. The reloading host runs its whole session through this, so the
/// window, the GPU objects and the program arena (all kept per thread) live as long as it does.
/// A Kestrel failure inside ends the process with 101 as usual; a panic in the runtime itself
/// gives 101.
pub fn start_host(host: impl FnOnce() -> i32 + Send + 'static) -> i32 {
    started();
    if !crate::fpu::has_sse41() {
        eprintln!("kestrel: {}", crate::fpu::NO_SSE41);
        return 101;
    }
    crate::blocks::set_poison(poison_setting(std::env::var("KESTREL_POISON").ok().as_deref()));
    on_big_stack(move || run_program(host))
}

/// Runs `body` on a thread with a 256 MiB stack and returns its exit code, or 101 when it panics.
fn on_big_stack(body: impl FnOnce() -> i32 + Send + 'static) -> i32 {
    // Shared with the thread, so `body` can still run here when no thread can be made.
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(body)));
    let theirs = slot.clone();
    let spawned = std::thread::Builder::new().stack_size(STACK_SIZE).spawn(move || {
        let body = theirs.lock().unwrap_or_else(|e| e.into_inner()).take();
        body.map_or(0, |body| body())
    });
    match spawned {
        // A panic in the runtime itself (not a Kestrel failure, which exits directly).
        Ok(thread) => thread.join().unwrap_or(101),
        Err(_) => {
            // No room for a big stack: run on this thread rather than not at all.
            let body = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            body.map_or(0, |body| body())
        }
    }
}

#[cfg(test)]
#[path = "start_test.rs"]
mod start_test;

#[cfg(test)]
mod tests {
    use super::{arena_count_report, arena_report, kestrel_rt_start, poison_setting, run_entry};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    #[test]
    fn the_arena_report_is_on_only_for_one() {
        assert_eq!(arena_report(Some("1"), || 65536).as_deref(), Some("kestrel: program arena 65536 bytes"));
        assert_eq!(arena_report(Some("0"), || 1), None);
        assert_eq!(arena_report(Some("yes"), || 1), None);
        assert_eq!(arena_report(None, || 1), None);
    }

    #[test]
    fn the_arena_is_not_measured_unless_the_report_is_on() {
        for setting in [None, Some("0"), Some("true")] {
            assert_eq!(arena_report(setting, || panic!("measured with {setting:?}")), None);
        }
    }

    #[test]
    fn the_arena_count_is_on_only_for_one() {
        assert_eq!(arena_count_report(Some("1"), || 0).as_deref(), Some("arenas alive at exit: 0"));
        assert_eq!(arena_count_report(Some("1"), || 3).as_deref(), Some("arenas alive at exit: 3"));
        for setting in [None, Some("0"), Some("true")] {
            assert_eq!(arena_count_report(setting, || panic!("counted with {setting:?}")), None);
        }
    }

    #[test]
    fn the_start_instant_is_fixed_once_and_never_after_now() {
        let first = super::started();
        assert_eq!(kestrel_rt_start(entry), 0);
        assert_eq!(super::started(), first);
        assert!(first <= std::time::Instant::now());
    }

    #[test]
    fn poisoning_is_on_only_for_one() {
        assert!(poison_setting(Some("1")));
        for setting in [None, Some("0"), Some("true"), Some("")] {
            assert!(!poison_setting(setting), "{setting:?}");
        }
    }

    static RAN: AtomicBool = AtomicBool::new(false);
    static DEEP: AtomicBool = AtomicBool::new(false);

    extern "C" fn entry() {
        RAN.store(true, Ordering::SeqCst);
    }

    #[test]
    fn start_runs_entry_and_returns_zero() {
        assert_eq!(kestrel_rt_start(entry), 0);
        assert!(RAN.load(Ordering::SeqCst));
    }

    /// Uses 1 MiB of stack per level.
    fn recurse(depth: u32) -> u8 {
        let block = [depth as u8; 1 << 20];
        let below = if depth == 0 { 0 } else { recurse(depth - 1) };
        std::hint::black_box(&block);
        below.wrapping_add(block[1 << 10])
    }

    extern "C" fn deep_entry() {
        // 64 MiB: far past a default thread stack, well inside ours.
        std::hint::black_box(recurse(64));
        DEEP.store(true, Ordering::SeqCst);
    }

    #[test]
    fn entry_runs_on_a_large_stack() {
        assert_eq!(kestrel_rt_start(deep_entry), 0);
        assert!(DEEP.load(Ordering::SeqCst));
    }

    static MODE: AtomicU32 = AtomicU32::new(0);

    extern "C" fn mode_entry() {
        MODE.store(crate::fpu::read(), Ordering::SeqCst);
    }

    /// A fresh thread already starts in the pinned mode, so this runs `run_entry` on a thread
    /// set to a wrong mode first (round toward zero, denormals are zero): only the pin fixes it.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn start_pins_the_float_mode_for_main() {
        let seen = std::thread::spawn(|| {
            crate::fpu::set(0x7FC0);
            assert_eq!(crate::fpu::read(), 0x7FC0);
            run_entry(mode_entry);
            (MODE.load(Ordering::SeqCst), crate::fpu::read())
        })
        .join()
        .unwrap();
        assert_eq!(seen, (0x1F80, 0x1F80));
        assert_eq!(kestrel_rt_start(mode_entry), 0);
        assert_eq!(MODE.load(Ordering::SeqCst), 0x1F80);
    }
}
