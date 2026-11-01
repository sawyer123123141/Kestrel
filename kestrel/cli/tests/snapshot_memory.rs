//! A ring of snapshots stops allocating once each slot has held the longest state: assigning a
//! snapshot over an older one reuses the older one's buffers (a list's items, a map's entries and
//! index) when they have room.
//!
//! Memory is measured with the runtime's own report (`KESTREL_ARENA_REPORT=1`: the size of the
//! program arena's blocks when `main` returns), which is exact. Each program runs for 2 periods
//! and for 200; a period (816 frames: 16 slots times 51 lengths) is enough for every slot to have
//! held the longest list (the map program's is 336). The control leaves out the save and restore lines, to show the program's
//! own work allocates nothing after its first period.
//!
//! The rollback library's session does the same: two sessions of a game whose state holds such a
//! list, over a `FakeNet` that loses some packets (so they roll back), stay flat after the first lap.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

use common::hermetic;

/// The program, with `PERIODS` periods, saving and restoring snapshots when `ring` is set.
fn program(periods: u32, ring: bool) -> String {
    let (save, restore) = if ring {
        ("        ring[f % 16] = g\n", "        if f % 7 == 0 {\n            g = ring[(f + 11) % 16]\n        }\n")
    } else {
        ("", "")
    };
    format!(
        "let PERIODS = {periods}

struct State {{
    xs: list[int]
    name: string
    hp: int
}}

main() {{
    var g = State(xs: [], name: \"snapshot\", hp: 0)
    var ring: list[State] = []
    for i from 0 to 16 {{
        ring.add(State(xs: [], name: \"snapshot\", hp: 0))
    }}
    for f from 0 to PERIODS * 816 {{
        g.xs.clear()
        for k from 0 to f % 51 {{
            g.xs.add(k)
        }}
        g.hp = f
{save}{restore}    }}
    print(g.hp, g.xs.len, ring.len)
}}
"
    )
}

/// The same with a `map[int, int]` in the state whose key count cycles from 0 to 20. A period is
/// 336 frames (16 slots times 21 counts). Each frame removes every key and adds the frame's: the
/// map's removed entries are compacted in place, so its own updates stop allocating.
fn map_program(periods: u32, ring: bool) -> String {
    let (save, restore) = if ring {
        ("        ring[f % 16] = g\n", "        if f % 7 == 0 {\n            g = ring[(f + 11) % 16]\n        }\n")
    } else {
        ("", "")
    };
    format!(
        "let PERIODS = {periods}

struct State {{
    m: map[int, int]
    hp: int
}}

main() {{
    var g = State(m: {{}}, hp: 0)
    var ring: list[State] = []
    for i from 0 to 16 {{
        ring.add(State(m: {{}}, hp: 0))
    }}
    for f from 0 to PERIODS * 336 {{
        for k from 0 to 21 {{
            g.m.remove(k)
        }}
        for k from 0 to f % 21 {{
            g.m[k] = f
        }}
        g.hp = f
{save}{restore}    }}
    print(g.hp, g.m.len, ring.len)
}}
"
    )
}

/// Writes `src` as `name.kes` and gives its path.
fn write(name: &str, src: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("snapshot_memory");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.kes"));
    fs::write(&path, src).unwrap();
    path
}

/// Runs the program at `path` and gives the program arena's size it reported.
fn arena_bytes(path: &Path) -> u64 {
    let out = hermetic(Command::new(env!("CARGO_BIN_EXE_kestrel")).arg("run").arg(path)).env("KESTREL_ARENA_REPORT", "1").output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{}: exit {:?}\n{stderr}", path.display(), out.status.code());
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("kestrel: program arena "))
        .unwrap_or_else(|| panic!("no arena report in stderr:\n{stderr}"));
    line.strip_suffix(" bytes").and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("can't read the arena report {line:?}"))
}

#[test]
fn a_snapshot_ring_stops_allocating_after_its_first_lap() {
    let bytes = |name: &str, periods: u32, ring: bool| arena_bytes(&write(name, &program(periods, ring)));
    let (control_few, control_many) = (bytes("control_2", 2, false), bytes("control_200", 200, false));
    assert_eq!(control_few, control_many, "the control's own work allocates after its first period");
    let (ring_few, ring_many) = (bytes("ring_2", 2, true), bytes("ring_200", 200, true));
    assert_eq!(ring_few, ring_many, "the snapshot ring grew from {ring_few} bytes (2 periods) to {ring_many} bytes (200 periods)");
}

#[test]
fn a_map_in_a_snapshot_ring_stops_allocating() {
    let bytes = |name: &str, periods: u32, ring: bool| arena_bytes(&write(name, &map_program(periods, ring)));
    let (control_few, control_many) = (bytes("map_control_2", 2, false), bytes("map_control_200", 200, false));
    assert_eq!(control_few, control_many, "the control's own work allocates after its first period");
    let (ring_few, ring_many) = (bytes("map_ring_2", 2, true), bytes("map_ring_200", 200, true));
    assert_eq!(ring_few, ring_many, "the snapshot ring grew from {ring_few} bytes (2 periods) to {ring_many} bytes (200 periods)");
}

/// Two rollback sessions of a game whose list cycles from 0 to 50 long, over a `FakeNet` with 5%
/// loss, for `laps` laps of 816 frames. The arriving packets and the outgoing ones are made in
/// arenas; the sessions' rings and the network's slots are made once.
fn session_program(laps: u32) -> String {
    format!(
        "use rollback
use {{ Input, Rollback, new_session, new_fake_net from rollback }}

let LAPS = {laps}

struct Cycle: Rollback {{
    xs: list[int]
    hp: int

    pure step(var self, one: Input, two: Input) {{
        self.xs.clear()
        for k from 0 to self.hp % 51 {{
            self.xs.add(k + one.buttons * 100 + two.buttons)
        }}
        self.hp += 1
    }}
}}

main() {{
    var p1 = new_session(Cycle(xs: [], hp: 0), me: 1, delay: 0)
    var p2 = new_session(Cycle(xs: [], hp: 0), me: 2, delay: 0)
    var net = new_fake_net(3, latency: 4, jitter: 2, loss: 0.05)
    for tick from 0 to LAPS * 816 {{
        arena {{
            for p in net.deliver(tick) {{
                if p.sender == 1 {{
                    p2.remote(p)
                }} else {{
                    p1.remote(p)
                }}
            }}
        }}
        p1.local(Input(buttons: tick % 3))
        p1.advance()
        p2.local(Input(buttons: (tick / 10) % 2))
        p2.advance()
        arena {{
            net.send(tick, p1.outgoing())
            net.send(tick, p2.outgoing())
        }}
    }}
    print(p1.confirmed() > LAPS * 816 - 20, p2.confirmed() > LAPS * 816 - 20, p1.rollbacks > LAPS, p2.rollbacks > LAPS, p1.desync_at, p2.desync_at)
}}
"
    )
}

#[test]
fn a_rollback_session_of_a_list_holding_game_stops_allocating_after_its_first_lap() {
    let run = |name: &str, laps: u32| {
        let path = write(name, &session_program(laps));
        let out = hermetic(Command::new(env!("CARGO_BIN_EXE_kestrel")).arg("run").arg(&path)).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true true true true -1 -1", "{name}");
        arena_bytes(&path)
    };
    let (few, many) = (run("session_2", 2), run("session_200", 200));
    assert_eq!(few, many, "the sessions grew from {few} bytes (2 laps) to {many} bytes (200 laps)");
}
