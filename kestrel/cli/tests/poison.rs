//! `KESTREL_POISON=1`: memory an arena frees is filled with 0xDB and never reused, so a read
//! of freed memory shows up as garbage instead of stale-but-plausible bytes.
//!
//! The runtime is driven directly (through `start_with`, which takes the setting the way
//! `KESTREL_POISON` is read at start), and a whole program runs through `kestrel run`.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use kestrel_runtime::arena::{kestrel_rt_alloc, kestrel_rt_arena_free, kestrel_rt_arena_new, kestrel_rt_arena_pop, kestrel_rt_arena_push};

mod common;

/// `start_with` sets a process-wide setting, so the tests that use it take turns.
static TURN: Mutex<()> = Mutex::new(());
static FIRST: AtomicUsize = AtomicUsize::new(0);
static SECOND: AtomicUsize = AtomicUsize::new(0);
static FIRST_BYTE: AtomicUsize = AtomicUsize::new(0);

/// Makes an arena, writes 1s into 100 bytes of it, frees it, then makes another and allocates.
extern "C" fn churn() {
    let a = kestrel_rt_arena_new();
    kestrel_rt_arena_push(a);
    let p = kestrel_rt_alloc(100, 1, 1, 0, std::ptr::null());
    // SAFETY: `p` holds 100 bytes.
    unsafe { std::ptr::write_bytes(p, 1, 100) };
    kestrel_rt_arena_pop();
    // SAFETY: `a` came from `kestrel_rt_arena_new` and is no longer pushed.
    unsafe { kestrel_rt_arena_free(a) };
    // SAFETY: the freed block is still mapped either way (poisoned, it stays allocated; plain,
    // it goes to the spare cache).
    FIRST_BYTE.store(unsafe { *p } as usize, Ordering::SeqCst);
    let b = kestrel_rt_arena_new();
    kestrel_rt_arena_push(b);
    let q = kestrel_rt_alloc(100, 1, 1, 0, std::ptr::null());
    kestrel_rt_arena_pop();
    FIRST.store(p as usize, Ordering::SeqCst);
    SECOND.store(q as usize, Ordering::SeqCst);
    // SAFETY: `b` came from `kestrel_rt_arena_new` and is no longer pushed.
    unsafe { kestrel_rt_arena_free(b) };
}

#[test]
fn a_freed_block_is_poisoned_and_not_handed_to_the_next_arena() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(kestrel_runtime::start::start_with(churn, true), 0);
    assert_ne!(FIRST.load(Ordering::SeqCst), SECOND.load(Ordering::SeqCst), "the freed block was reused");
    assert_eq!(FIRST_BYTE.load(Ordering::SeqCst), 0xDB, "the freed block was not poisoned");
}

#[test]
fn without_poisoning_the_next_arena_reuses_the_block() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(kestrel_runtime::start::start_with(churn, false), 0);
    assert_eq!(FIRST.load(Ordering::SeqCst), SECOND.load(Ordering::SeqCst), "the spare cache should hand the block back");
    assert_eq!(FIRST_BYTE.load(Ordering::SeqCst), 1);
}

/// The arena report line of `arenas.kes` with `KESTREL_POISON` as given.
fn arena_report(poison: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/programs/arenas.kes");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    let out = common::hermetic(cmd.arg("run").arg(path)).env("KESTREL_POISON", poison).env("KESTREL_ARENA_REPORT", "1").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.starts_with("kestrel: program arena ")).collect()
}

#[test]
fn poisoning_does_not_change_the_program_arena_report() {
    let (off, on) = (arena_report("0"), arena_report("1"));
    assert!(off.contains("program arena"), "no report: {off:?}");
    assert_eq!(on, off);
}

#[test]
fn the_harness_turns_poisoning_on_and_a_later_env_can_turn_it_off() {
    fn poison(cmd: &Command) -> Option<&OsStr> {
        cmd.get_envs().find(|(key, _)| *key == "KESTREL_POISON").and_then(|(_, value)| value)
    }
    let mut cmd = Command::new("kestrel");
    common::hermetic(&mut cmd);
    assert_eq!(poison(&cmd), Some(OsStr::new("1")));
    cmd.env("KESTREL_POISON", "0");
    assert_eq!(poison(&cmd), Some(OsStr::new("0")));
}

/// Two rollback sessions over a `FakeNet` that loses 20% of the packets: each packet is made by
/// `outgoing` inside an `arena { }`, copied into the network's slots, freed with the arena, and
/// given to `remote` frames later from a list made in another arena.
const ROLLBACK_PACKETS: &str = "use rollback
use { Input, Rollback, Session, new_session, new_fake_net from rollback }

struct Tally: Rollback {
    one: int
    two: int

    pure step(var self, one: Input, two: Input) {
        self.one += one.buttons
        self.two += two.buttons * 3
    }
}

in1(f: int) -> Input {
    Input(buttons: f % 3)
}

in2(f: int) -> Input {
    Input(buttons: (f / 10) % 2)
}

confirmed(s: Session[Tally]) -> Tally {
    let c = s.confirmed()
    if c == s.now { s.game } else { s.ring[c % rollback.RING] }
}

main() {
    var p1 = new_session(Tally(one: 0, two: 0), me: 1, delay: 0)
    var p2 = new_session(Tally(one: 0, two: 0), me: 2, delay: 0)
    var net = new_fake_net(9, latency: 3, jitter: 3, loss: 0.2)
    for tick from 0 to 300 {
        arena {
            for p in net.deliver(tick) {
                if p.sender == 1 {
                    p2.remote(p)
                } else {
                    p1.remote(p)
                }
            }
        }
        p1.local(in1(tick))
        p1.advance()
        p2.local(in2(tick))
        p2.advance()
        arena {
            net.send(tick, p1.outgoing())
            net.send(tick, p2.outgoing())
        }
    }
    var straight = Tally(one: 0, two: 0)
    let c = p1.confirmed()
    for f from 0 to c {
        straight.step(in1(f), in2(f))
    }
    let a = confirmed(p1)
    print(c > 280, p2.confirmed() > 280, a.one == straight.one, a.two == straight.two, net.lost > 0)
    print(p1.compared, p2.compared, p1.desync_at, p2.desync_at)
}
";

#[test]
fn packets_made_in_an_arena_go_through_a_fake_net_to_a_session_under_poisoning() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("poison");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollback_packets.kes");
    std::fs::write(&path, ROLLBACK_PACKETS).unwrap();
    let run = |poison: &str| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
        let out = common::hermetic(cmd.arg("run").arg(&path)).env("KESTREL_POISON", poison).output().unwrap();
        assert!(out.status.success(), "KESTREL_POISON={poison}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let (on, off) = (run("1"), run("0"));
    let lines: Vec<&str> = on.lines().collect();
    assert_eq!(lines.first(), Some(&"true true true true true"), "{on}");
    assert!(lines.get(1).is_some_and(|l| l.ends_with(" -1 -1")), "{on}");
    assert_eq!(on, off);
}
