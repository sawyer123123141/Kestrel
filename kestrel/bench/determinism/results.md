# The rollback library (2026-11-01)

Measured 2026-11-01 on the user's PC (Ryzen 7 5800X, 8 cores, 16 threads, Windows 11), the PC
free: no build, no Godot and no Roblox running (checked with `Get-Process` before and after). The
compiler is `kestrel` at 39ab7f9 built `--release` (`target/release/kestrel.exe`), every program
run with `kestrel run --release` (the JIT), at normal priority, one measurement at a time.
**Load** is the whole machine's CPU use (`\Processor(_Total)\% Processor Time`, 2 s): 6.5%
before the first block and 14.8% after the last, the Claude app, OneDrive and Defender. Spec:
`docs/superpowers/specs/2026-10-31-rollback-library-design.md` ("Measurements").

**Short version:** a session frame (`remote`, `local`, `advance`) costs Pong **38 ns** on a
perfect wire, against 9.7 ns for its `step` alone, and **132 ns with an 8-frame rollback every
frame** (3.5 times: nine steps and nine snapshots, but the fixed costs are most of a frame with a
step this small). The crowd (1,000 bodies) costs **2.2 µs** a frame (`step` 1.3-1.4 µs, the rest
mostly the 16 KB snapshot) and **18.1 µs** with the rollback every frame (8.2 times). **Pong's
whole network frame is 6-9% slower than with `peer.kes`** (2.30 against 2.12 µs at 600,000
frames; 7.5% slower with recording off on both), **so the 5% bound is missed**: about a third
of it is the arena the outgoing packets are made in each frame, most of the rest the wider
`Input`, and the replay keeps 24 bytes a frame against 4. Both Pongs print the same lines at
every length. The sessions' memory **stays flat**: the program arena is the same after 2 and
200 ring laps (64 KB for Pong, 576 KB for the crowd), with every frame rolling back.

## How it was measured

- **`bench/rollback/`** holds the programs:
  - `crowd.kes`: the heavier `Rollback` game. 1,000 bodies whose positions and velocities are
    two `list[vec2]`, and an `Rng`. `step` moves every body by its velocity scaled by both
    players' buttons, wraps it around a 1,000-unit field, and gives one random body a new
    velocity. It is `@prove(deterministic)`; it can't be `no_alloc` through the library, since
    its state holds lists.
  - `game.kes`: a copy of Pong's `demos/pong/game.kes` at 39ab7f9 (a bench can't `use` a file
    above its folder).
  - `drive.kes`: player 1's inputs and player 2's packets on two wires. On the **perfect wire**
    player 2's input for frame t arrives at tick t, before `advance`, so no frame runs on a
    guess. On the **late wire** player 2's packets arrive 8 frames late and its input flips
    between up and down every frame. A frame not yet known is run again on each rollback, on the
    newest input known (the frame before it), so its last guess is always wrong: from tick 8 on,
    every `advance` rolls back 8 frames and runs 9.
  - `advance.kes` (`kestrel run --release bench/rollback/advance.kes [long]`): for each game,
    player 1's session runs 2,000 frames (`long`: 20,000), five times on each wire, each run in
    an `arena { }` of its own from a new session. A frame is the session's three calls: `remote`
    with that tick's packet (all made before the timer starts), `local` and `advance`, timed
    with `time.now()` around the loop. The game's `step` alone on the same inputs is timed too,
    as the floor. It checks that the late wire made 1,992 rollbacks 8 deep and that its newest
    confirmed state has the checksum of the `step`-alone run at that frame. The program was run
    three times, and once with `long`.
  - `memory.kes` (`KESTREL_ARENA_REPORT=1 kestrel run --release bench/rollback/memory.kes
    pong|crowd [long]`): player 1's session on the late wire for 2 laps of the ring (32 frames;
    `long`: 200 laps, 3,200 frames), each packet made and taken in inside an arena of its own,
    the session in the program's memory, as Pong keeps its sessions. The runtime's report is the
    program arena's size when `main` returns.
  - `network.py` (`python bench/rollback/network.py <kestrel.exe> <old> <new>`): Pong's whole
    run before and after the rewrite. The old Pong is `git show 1b5e284:kestrel/demos/pong/<f>`
    for its five files, the new one today's two, each copied to `C:/kt/rb5/` (outside the synced
    folder, since a run writes `pong.replay` beside it). Both run under the same `kestrel` with
    `KESTREL_HEADLESS=<frames> KESTREL_HEADLESS_QUIET=1 KESTREL_POISON=0`, for 1, 3,000, 60,000
    and 600,000 frames, five rounds, old and new in turn within each round, the whole process
    timed by `time.perf_counter`. One frame's cost is (T(N) - T(1)) / (N - 1) from the medians.
    It includes the headless window's frame and the end-of-run replay check (which runs the
    confirmed frames again) and write, as the poll-cost measurement of the hot reload round does.
  - For the verdict's why, the same ran with recording off on both (`RECORD = false` in the new
    `pong.kes`; the old one's four-line `while` loop that records removed), and once more at 1
    and 600,000 frames with the old Pong's two `send` calls wrapped in an `arena { }` (a scratch
    script, not kept).

Medians of five, with the range (min to max) in brackets.

## `advance` per frame

ns per frame, from the first of three runs of the program (the other two in the notes):

| game | `step` alone | perfect wire | 8-frame rollback every frame | rollback / perfect |
|---|---:|---:|---:|---:|
| Pong | 9.7 (8.3-13.4) | **38.0** (37.1-47.1) | **131.8** (124.5-165.0) | 3.47 |
| crowd | 1,388.8 (1,302.7-1,512.8) | **2,206.8** (2,185.4-2,221.7) | **18,138.1** (18,087.3-18,138.8) | 8.22 |
| Pong, 20,000 frames | 8.3 (8.1-8.6) | 37.3 (37.2-37.5) | 126.9 (125.6-128.1) | 3.40 |
| crowd, 20,000 frames | 1,372.3 (1,328.9-1,377.4) | 2,216.1 (2,212.6-2,274.9) | 18,232.9 (18,204.9-18,514.2) | 8.23 |

- **The other two runs agree:** Pong 37.5 and 37.6 ns on the perfect wire, 133.6 and 133.1 ns
  rolling back (3.57 and 3.54 times); the crowd 2,202 and 2,263 ns, 18,149 and 18,263 ns (8.24
  and 8.07 times).
- **Pong's session costs about 28 ns a frame over its `step`:** the snapshot into the ring,
  taking in an 8-input packet, the input histories, and the confirm and compare checks. A
  rollback of 8 frames runs nine steps and writes nine snapshots, about 95 ns more: 3.5 times
  the perfect wire, not 9, because the fixed costs are most of Pong's frame.
- **The crowd's session costs about 0.85 µs over its 1.3-1.4 µs `step`**, mostly the
  snapshot's 16 KB of list items (copied into the slot's own storage, without allocating).
  Rolling back 8 frames is nine steps and nine snapshots, 18.1 µs, 8.2 times the perfect wire.
- In every run, the late wire's confirmed state equals the `step`-alone run's at the same frame
  for both games, and it made 1,992 rollbacks 8 deep in 2,000 frames.

## Pong's network frame, before and after

As shipped (recording on; the new replay keeps 24 bytes a frame, the old 4). Load as above.

| frames | `peer.kes`, ms | library, ms | `peer.kes`, µs a frame | library, µs a frame | library / `peer.kes` |
|---|---:|---:|---:|---:|---:|
| 1 | 105.3 (103.4-118.9) | 136.6 (135.0-138.2) | | | |
| 3,000 | 114.1 (109.8-115.8) | 141.7 (141.5-144.7) | (2.941) | (1.719) | (not resolved) |
| 60,000 | 233.5 (228.7-233.9) | 272.8 (269.0-274.4) | 2.138 | 2.270 | 1.062 |
| **600,000** | 1,376.0 (1,371.8-1,381.3) | 1,518.9 (1,510.0-1,548.8) | **2.118** | **2.304** | **1.088** |

With recording off on both:

| frames | `peer.kes`, ms | library, ms | `peer.kes`, µs a frame | library, µs a frame | library / `peer.kes` |
|---|---:|---:|---:|---:|---:|
| 1 | 106.4 (102.7-107.6) | 135.4 (132.0-137.2) | | | |
| 3,000 | 111.3 (110.0-112.7) | 140.5 (138.7-144.1) | (1.641) | (1.706) | (not resolved) |
| 60,000 | 229.2 (228.4-229.5) | 271.3 (267.9-272.3) | 2.047 | 2.265 | 1.107 |
| **600,000** | 1,360.8 (1,354.7-1,374.6) | 1,484.1 (1,470.5-1,509.8) | **2.091** | **2.248** | **1.075** |

- **The 5% bound is missed: the library's network frame is 6-9% slower** (+0.13 to +0.19 µs on
  about 2.1 µs), and 7.5% (+0.157 µs) with recording off. Both print the same lines at every
  length, so it is the same game, the same network and the same rollbacks.
- **Why:**
  - **The packet's arena, about a third.** Old Pong's `send` copied the 8 inputs straight out
    of the peer's history; the library's `outgoing()` makes a `Packet` whose inputs are
    allocated, so Pong calls it inside an `arena { }`, one more each frame. Wrapping old Pong's
    two `send` calls in an arena alone took it from 2.093 to 2.148 µs a frame (600,000 frames,
    recording off): 0.055 µs of the 0.155 µs gap.
  - **The wider input, most of the rest.** Old Pong's input was two `bool`s; an `Input` is an
    `int` and two `float`s, copied into the histories, the guesses, every packet's 8 inputs and
    `FakeNet`'s slots, and compared field by field in `remote`. `advance` also clears the next
    input slot and adds the input delay, which old Pong did not do. This part was not split
    further.
  - **Recording, the remainder.** The replay keeps both `Input`s, 24 bytes a frame against 4:
    recording costs the library about 0.056 µs a frame and old Pong about 0.027 µs.
- **3,000 frames can't measure a frame.** The run is 110-140 ms, nearly all of it the compile,
  and its frames take 5-7 ms in all, so the per-frame numbers there swing with the 1-frame run's
  noise (1.6 and 2.9 µs for the same old Pong).
- **The library's compile is about 30 ms longer** (1-frame runs, 105-106 against 135-137 ms): the
  module's four files and `Session[Game]`, `Replay[Game]` and their methods made for Pong, on
  every start. That is not a per-frame cost.

## Session memory

The program arena's size in bytes from `KESTREL_ARENA_REPORT=1`, `memory.kes` on the late wire
(every frame rolls back 8 frames).

| game | 2 laps (32 frames) | 200 laps (3,200 frames) |
|---|---:|---:|
| Pong | 65,536 | **65,536** |
| crowd | 589,824 | **589,824** |

- **Both stay flat after the first lap.** Pong's session fits in the program arena's first
  64 KB block, as the demo's two do (Task 4: 65,536 bytes at 600 and 3,000 frames with
  recording off).
- The crowd's 576 KB holds the session's 17 crowds (the ring's 16 and the present, 16 KB of
  items each), the start crowd, and the lists' growth while `new_crowd` built it. Every ring
  write after the first lap reuses its slot's storage.
- The report is a total of blocks, so a growth smaller than a block would not show at 2 against
  200 laps unless it crossed one. `cli/tests/snapshot_memory.rs` makes the same check for a
  list-holding session over 2 and 200 laps of 816 frames.

# The hot reload round (2026-10-31)

Measured 2026-10-31 on the user's PC (Ryzen 7 5800X, 8 cores, 16 threads, Windows 11) while the
user was away: no build and no Godot running, and no keyboard or mouse input for the 27 minutes
before the runs began. Roblox (`RobloxPlayerBeta`) was open but idle in the background, using
about 0.7 of a thread (measured over 5 s); the I/O round's runs had no Roblox at all. The
compiler is `kestrel` at 466627e, built twice: `--release` (`target/release/kestrel.exe`) and
debug, for the record. Every program ran headless and quiet (`KESTREL_HEADLESS_QUIET=1`), at
normal priority, one measurement at a time. **Load** is the whole machine's CPU use
(`\Processor(_Total)\% Processor Time`, 2 s) before and after each block: 7-19%, the Claude app,
OneDrive, Defender and the idle Roblox. Spec: `docs/superpowers/specs/2026-10-30-hot-reload-design.md`.

**Short version:** with the release `kestrel`, a save reloads **Pong in 0.28 s** (median of
five; at most 0.36 s in twenty saves) and **the voxel engine in 0.28 s** (at most 0.45 s), so
the spec's "within a second for Pong" holds, with room. **The counter** takes 0.21 s. About
0.11 s of Pong's time and 0.19 s of the voxel engine's is the compile; the rest is the
quarter-second poll, up to 0.25 s. The poll costs **nothing measurable a frame**: about 0.01 µs
on Pong's 2.1 µs frame, inside the noise. A same-shape reload keeps about 0.9 MB. A **shape
change keeps about 58-64 MB** of the voxel engine: the old state's world is not given back, as
decision 4 and the anti-goals accept. The debug `kestrel` takes 1.2 s for Pong and 2.3 s for the voxel engine,
which is Task 5's 1.3 and 2.3 s.

## How it was measured

- **The driver** is `cli/tests/reload_bench.rs`: three `#[ignore]`d tests that print their
  numbers. It uses the reload tests' harness, `cli/tests/common/session.rs`, which edits a file
  by writing a new one and renaming it over the old. One command runs all three:

  ```
  cargo test --release -p kestrel-cli --test reload_bench -- --ignored --nocapture --test-threads=1
  ```

  Without `--release`, the same command measures the debug `kestrel`. Each test can also be run
  alone by adding its name and `--exact` (`latency`, `poll_cost`, `memory`).
- **Where the programs run:** every program runs from a copy in the system's temporary folder,
  outside the synced checkout. The copy runs with `KESTREL_POISON=0`, as a normal run does. The
  tests' `hermetic` turns poisoning on, and poisoning never reuses freed arena memory: in a
  first try with it on, Pong ran out of memory at 600,000 frames, and the voxel engine reached
  863 MB before any reload.
- **Latency** is the wall time from just before the rename to the `reloaded <file> in <t> s`
  line on stderr.
  - The saves are 0.4 s apart plus 37 ms more each time, so they land at every point of the
    poll. Twenty saves per program alternate between two versions of a constant:
    - the counter: `K`, 2 and 3;
    - Pong: `MAX_STEPS`, 4 and 5;
    - the voxel engine: `WALK` in `player.kes`, 4.3 and 6.0.
  - The headline is the **median of the first five**. The **fastest of the twenty** is a save
    that landed just before a poll, so it is about the compile alone.
  - The host's `<t>` is not the compile. As decision 6 says, it runs from the save (the file's
    modification time) to the swap, so it holds the poll wait, the compile and the wait for the
    end of the frame. It matches the wall time to within 0.01 s. The brief expected it to be the
    compile, which is wrong.
  - Each program runs with `TRACE` on and its frames slowed, as the demos' reload tests do:
    5 ms a frame for the counter and Pong, 2 ms more for the voxel engine.
  - The first edit comes after 20 frames of the counter, 30 of Pong, and once the voxel player
    has spawned and the columns have stopped changing for 2 s.
  - Each program is measured with and without `kestrel run --release`.
  - For comparison, a **one-frame `--no-reload` run** of the same files is timed whole, five
    times: the compile plus the process's start and one `setup` and `frame`.
- **Poll cost:** Pong from an unedited copy (`TRACE` off, no sleep), run whole at 1, 2,000,
  60,000 and 600,000 frames, with reloading on and with `--no-reload`, five rounds in turn. The
  cost of one frame is (T(N) - T(1)) / (N - 1). That includes Pong's end-of-run replay check,
  which runs the confirmed frames again. Both modes run through the reloading host's loop; the
  only difference is the watcher.
- **Memory:** the voxel engine as for latency, with `WorkingSet64` and `PrivateMemorySize64` of
  the `kestrel.exe` the test started (found by its parent process id) read at each point:
  - once the world has streamed in (the player has spawned and stood still for 10 frames, and
    the columns in use have not changed for 2 s);
  - after ten `WALK` edits, and the world settled again;
  - after each of ten shape changes, with the world streamed in again each time. The shape
    changes alternately add `extra: int` to `VoxelState` (and `extra: 0` to `setup`'s
    constructor) and remove it. Each one printed `state reset: VoxelState changed shape`.
- **Window:** no visible-window check was run. The user does not want windows opened, and
  every run here is headless.

## Reload latency, save to `reloaded`

Load 8.0% before and 15.2% after the release block, and 8.1% and 18.8% for the debug block.
Seconds, as median (min-max):

| program | `kestrel` | median of 5 | 20 saves: median | fastest (about the compile) | one-frame run |
|---|---|---:|---:|---:|---:|
| counter | release | **0.21** (0.03-0.26) | 0.16 (0.02-0.26) | 0.02 | 0.045 |
| Pong | release | **0.28** (0.19-0.32) | 0.26 (0.14-0.36) | 0.14 | 0.125 |
| Pong, `run --release` | release | 0.27 (0.21-0.32) | 0.24 (0.11-0.34) | 0.11 | 0.125 |
| voxel engine | release | **0.28** (0.22-0.37) | 0.31 (0.19-0.42) | 0.19 | 0.268 |
| voxel engine, `run --release` | release | 0.30 (0.21-0.36) | 0.31 (0.19-0.45) | 0.19 | 0.247 |
| counter | debug | 0.25 (0.08-0.27) | 0.17 (0.01-0.27) | 0.01 | 0.044 |
| Pong | debug | 1.16 (1.06-1.31) | 1.25 (1.04-1.62) | 1.04 | 1.000 |
| Pong, `run --release` | debug | 1.37 (1.13-1.47) | 1.37 (1.13-1.51) | 1.13 | 0.978 |
| voxel engine | debug | 2.28 (2.11-2.38) | 2.27 (2.00-2.38) | 2.00 | 1.750 |
| voxel engine, `run --release` | debug | 2.20 (1.97-2.37) | 2.22 (1.97-2.37) | 1.97 | 1.630 |

- **"Within a second for Pong" holds in release.** The median is 0.28 s and the slowest of 40
  Pong saves 0.36 s. The voxel engine, at 0.28 s median and 0.45 s slowest, is under a second
  too. With the debug `kestrel` it does not hold: Pong takes 1.2 s and the voxel engine 2.3 s,
  as Task 5 found.
- **The poll is most of a release reload.**
  - The compile, the fastest save's time, is 0.02 s for the counter, 0.11-0.14 s for Pong and
    0.19 s for the voxel engine.
  - The rest, up to 0.25 s and about 0.13 s on average, is the wait for the next poll.
  - The 37 ms steps between saves show this in the raw times. Each save takes about 37 ms less
    than the one before, until it misses a poll and takes 0.25 s more (Pong: 0.30, 0.31, 0.27,
    0.25, 0.23, 0.20, 0.11, then 0.35).
  - A shorter poll interval would cut the median by up to 0.1 s. That is not done here.
- **The one-frame run backs up the compile times.** It is the compile plus about 0.02-0.04 s of
  process start (the counter's whole run is 0.024-0.045 s). Its voxel time also holds `setup`'s
  world and one frame, so it is above the fastest save.
- **`kestrel run --release` makes no difference with the release `kestrel`.** With the debug
  one, Pong's optimizing compile takes 0.2 s more.
- **Earlier runs agree.** Two earlier release runs had poisoning on, before the driver turned
  it off. Five saves each gave medians of 0.17, 0.24 and 0.29 s for the counter, Pong and the
  voxel engine. Twenty saves each gave first-five medians of 0.21, 0.27 and 0.31 s.

## The poll's cost per frame

Pong, headless and quiet, the whole process in ms, five rounds. Load 7.3% before and 9.2% after.

| frames | reload on | `--no-reload` | per frame, on | per frame, off | difference |
|---|---:|---:|---:|---:|---:|
| 1 | 125.2 (124.8-225.7) | 124.6 (124.0-167.5) | | | |
| **2,000** | 132.9 (124.4-145.0) | 124.7 (123.9-126.3) | (3.85 µs) | (0.03 µs) | (not resolved) |
| 60,000 | 246.9 (246.2-269.9) | 246.1 (245.2-250.1) | 2.028 µs | 2.026 µs | +0.002 µs |
| **600,000** | 1,407.0 (1,399.4-1,425.1) | 1,400.3 (1,363.9-1,425.7) | 2.136 µs | 2.126 µs | **+0.010 µs** |

- **The poll costs nothing measurable.** Over 600,000 frames (1.4 s, so about six polls that
  read the files' times), reloading on is 7 ms slower than off. That is 0.01 µs, or 0.5%, of a
  2.1 µs frame, and well inside the 62 ms spread of the `--no-reload` runs. At 60,000 frames it
  is 0.002 µs.
- Between polls, a frame's whole cost is one `Instant::elapsed` against the quarter second.
- **2,000 frames can't measure it.** The run is 125 ms, nearly all of it the compile, and its
  frames take a few ms at most. Even the `--no-reload` runs are within 0.1 ms of the one-frame
  ones.
- Two of the five reload-on 2,000-frame runs took 144-145 ms. The watcher's first poll comes
  250 ms after the start, after those runs had already ended, so it isn't a per-frame cost, and
  the gap is gone at 60,000 and 600,000 frames. I did not find its cause.

## Memory after ten reloads

The voxel engine, release `kestrel`, MB. Load 10.7% before and 15.2% after.

| point | working set | private bytes | columns |
|---|---:|---:|---:|
| world streamed in, after `setup` | 314.3 | 327.9 | 269 |
| after 10 same-shape reloads | 325.1 | 336.4 | 269 |
| after shape change 1 | 391.2 | 401.4 | 269 |
| after shape change 2 | 483.0 | 498.3 | 269 |
| after shape change 3 | 553.7 | 569.5 | 269 |
| after shape change 5 | 674.8 | 689.7 | 269 |
| after shape change 10 | **966.4** | **979.9** | 269 |

- **A shape change keeps about 58 MB, and 64 MB on average over the ten.** From the fourth on,
  each one adds 58.0-59.0 MB of working set. The first three add 66, 92 and 71 MB.
  - That is the voxel engine's world, which the I/O round sized at about 58 MB. The new `setup`
    makes a new world, and the old state's arena memory is not given back.
  - The spec accepts this. Decision 4 says the old state's arena memory is not reclaimed, and
    the anti-goals say a shape change leaks the old state once.
  - For the voxel engine, ten shape changes cost about 0.6 GB, and a restart gets it back.
  - Pong's state is small (its arena stays at 64 KB), so for Pong a shape change keeps almost
    nothing.
- **A same-shape reload keeps about 0.9 MB**: 10.9 MB of working set and 8.5 MB of private
  bytes over ten. That fits decision 2's leaked code: the JIT modules are never unmapped, and the
  host keeps the old programs (`Live.retired`). This measurement does not prove it. The world itself stays: still 269
  columns, and no new `setup`.
- An earlier run with poisoning off stopped at the shape changes because of CRLF line ends in
  the checkout's `main.kes`, which the driver now handles. Until then it gave the same numbers:
  313.7 and 324.5 MB of working set, against 314.3 and 325.1.

# The I/O round (2026-10-30)

Measured 2026-10-30 on the user's PC (Ryzen 7 5800X, 8 cores, 16 threads, Windows 11), the PC
free: no build, no Godot, no Roblox running (checked with `tasklist` before each block). The
compiler is `target/release/kestrel.exe` built `--release` at 45dcfe8, every program run with
`kestrel run --release` (the JIT), at normal priority. **Load** is the whole machine's CPU use
(`\Processor(_Total)\% Processor Time`, 2 s) before and after each block; the background was the
Claude app, OneDrive's sync and Defender, about one to two threads of 16 (2-15%). Spec:
`docs/superpowers/specs/2026-10-29-io-round-design.md` ("Measurements").

**Short version:** a `Game` snapshot copy costs about 1 ns, `to_bytes` of it about 70 ns over an
empty arena (most of it the arena's first block, not the encoding), `from_bytes` about 6 ns.
For a 100,000-item `list[int]`, `to_bytes` is **7.6 times slower than a copy** (49 against 6.4
µs) and `from_bytes` 10 times (65 µs): both go one item at a time where the copy is one memcpy.
`--record` costs Pong **0.17 µs a frame** (8% of its 2.1 µs frame) and the voxel engine nothing
measurable. `file.write` and `file.read` run at the OS's speed (about 320 and 1,370 MB/s, the
same as Rust's `std::fs` within noise for writes; reads are about 10% slower, one extra copy). A
replay check runs at the game's own speed.

## How it was measured

- **`bench/io/bytes.kes`**: Pong's `Game` (copied from `demos/pong/game.kes`, with its `Rng`),
  10,000 iterations of each operation, and a `list[int]` of 100,000 items, 100 iterations;
  `long` as the program's argument runs 100 times as many (1,000,000 and 10,000), for totals
  well above the clock's noise. Every iteration runs in an `arena { }` of its own and changes the
  state a little first (`nudge`, or one item, or one byte of the encoding for `from_bytes`), so
  nothing folds away. The copy is a save into a ring of 16 (`ring[i & 15] = g`, as a rollback
  keeps them) for `Game`, and `var snap = xs` with one item of `snap` changed for the list. A
  control loop with the same change and the same arena but no copy and no encoding is timed too
  and taken out ("net"); the raw time is given as well. Timed with `time.now()` around each loop.
- **`bench/io/files.kes`** (not `file.kes`: `file` is a built-in module, so a program can't have
  that name): writes a 1 MB (2^20 bytes) `list[u8]` with `file.write` 20 times, then reads it
  with `file.read` 20 times, each in its own arena, to `C:/kt/file-bench.bin` (outside OneDrive).
  The same loop in Rust (`std::fs::write`, `std::fs::read`, `rustc -O`, a scratch program not
  kept) ran in turn with it as the reference.
- **Pong**: `KESTREL_HEADLESS=<frames> KESTREL_HEADLESS_QUIET=1 kestrel run --release pong.kes`,
  with and without `--record C:/kt/pong-bench.replay`, and `kestrel replay pong.kes
  C:/kt/pong-bench.replay --release`, the three in turn in each round, the whole process timed by
  PowerShell's `Measure-Command`. Run from a copy of `demos/pong/` in `C:/kt/pongb/`, so the
  `pong.replay` Pong writes at the end doesn't land in the synced folder. The whole process is
  mostly compiling (a 1-frame run takes 107 ms), so 600 frames can't show a per-frame cost; 6,000
  and 60,000 frames were run as well.
- **Voxel engine**: `voxel/bench.kes` in a hidden window (`KESTREL_HIDDEN=1`), as `measure.py`
  runs it, but under `kestrel run --release` (`--record` is a `run` flag, so not the built exe)
  and from a copy in `C:/kt/voxb/` with the circle shortened to 30 s (`measure.py` shortens it to
  100 s the same way); 4,386 frames a run. Three rounds of plain then recorded. The same in a
  headless window (`KESTREL_HEADLESS=6000`, whose fake clock makes the bench's own times 0) timed
  whole by `Measure-Command`, six rounds: the CPU work alone, with no GPU and no display.

Medians of 3 runs unless said, with the range (min to max) in brackets.

## `to_bytes` and `from_bytes` against a snapshot copy

Load 10.8% before, 9.8% after (`long`: 9.8% before, 14.1% after).

| ns per operation | 10,000 / 100 iterations | `long`: 1,000,000 / 10,000 |
|---|---:|---:|
| empty loop, no arena (`Game`) | 0.65 (0.64-0.65) | 0.74 (0.63-0.80) |
| control: arena and change (`Game`) | 35.2 (35.1-35.2) | 36.2 (36.0-36.5) |
| `Game` copy, net | 1.1 (1.0-4.1) | **1.1** (1.1-1.2) |
| `to_bytes(g)`, net | 72.3 (71.4-74.8) | **69.3** (69.1-73.6) |
| `to_bytes(g)`, raw | 107.4 (106.6-110.0) | 105.5 (105.1-110.1) |
| `from_bytes[Game](b)`, net | 7.3 (2.3-7.3) | **6.2** (4.1-7.2) |
| `from_bytes[Game](b)`, raw | 43.1 (43.0-52.2) | 44.3 (43.5-44.4) |
| list copy (100,000 `int`s), net | 8,390 (7,505-**121,339**) | **6,443** (6,405-6,964) |
| list `to_bytes`, net | 48,874 (48,804-50,162) | **49,131** (48,142-53,242) |
| list `from_bytes`, net | 64,507 (64,436-64,852) | **65,270** (65,067-65,428) |

Byte sizes: `Game` **53 bytes** (the 13-byte header and 40 of fields); the list **400,017 bytes**
(the header, a 4-byte length and 4 bytes an item).

- **`Game`: `to_bytes` is about 65 times a copy, but the encoding is not where the time goes.**
  An empty `arena { }` costs 36 ns, and an arena that allocates once (a scratch probe:
  `array[u8](53)` in an arena, 1,000,000 times) costs 89 ns: its first allocation takes a
  64 KB block from the thread's spares and gives it back at the end, about 53 ns. `to_bytes`
  raw (105 ns) is that plus about 18 ns of sizing and writing 53 bytes. `from_bytes` makes
  nothing in the arena (a `Game?` is a plain value) and costs about 6 ns. So a rollback that
  serializes one small state per frame in an arena of its own pays about 0.1 µs; the arena's
  first block is the cost to cut (a smaller first block, or one block kept warm per thread
  without the spares' search), not the byte format.
- **The list: `to_bytes` is 7.6 times slower than a copy, `from_bytes` 10 times.** The copy is a
  memcpy of 400 KB (about 60 GB/s, in the L3 cache); `to_bytes` writes the 100,000 items one at a
  time (0.49 ns, about 2 cycles, an item: 8 GB/s), and `from_bytes` reads them one at a time
  with its checks (0.65 ns an item). For a list of plain numbers the format is the memory's own
  layout (little-endian, no padding), so both could be one memcpy (and the read one bounds
  check); that would bring them to about copy speed. Not done in this round.
- **In some runs every list copy was 15-20 times slower.** In 5 of 18 runs of the list copy (here
  the first 10,000/100 run: 121 µs; earlier runs of the same loop: 90, 91, 95 and 128 µs), every
  copy in that run took 90-128 µs instead of 6.4-8.4. That is about what faulting in 400 KB of
  fresh pages (about 100 of them) on every copy would cost, as if the copy's block weren't
  recycled warm in those runs; a scratch probe that times each copy alone (2,000 copies, 8 runs)
  never showed it. I did not find the cause; it is worth a look in the runtime's block recycling
  (`runtime/src/blocks.rs`). `to_bytes` and `from_bytes`, which also make a 400 KB block each
  iteration, never showed it.

## The per-frame cost of `--record`

Pong, headless and quiet, the whole process in ms:

| frames (rounds) | plain | `--record` | difference | `kestrel replay` |
|---|---:|---:|---:|---:|
| 1 (5) | 107.1 (106.7-113.9) | 108.5 (106.7-127.5) | +1.4 | 108.0 (106.6-114.9) |
| **600 (3)** | 111.6 (106.5-117.1) | 109.8 (108.4-126.5) | -1.8 | 110.5 (110.4-112.2) |
| 600 (11) | 111.7 (109.7-117.2) | 112.8 (108.9-128.5) | +1.1 | 113.2 (108.1-122.8) |
| 6,000 (5) | 121.5 (119.0-176.8) | 124.0 (119.3-138.3) | +2.5 | 122.1 (118.6-134.8) |
| **60,000 (5)** | 235.6 (229.2-243.4) | 245.9 (241.7-249.4) | **+10.3** | 242.9 (239.3-248.8) |

Load 9.0% before and 2.7% after the 3 rounds of 600; 5-8% for the others. The replay file is
12.0 KB for 600 frames and 1.1 MB for 60,000 (about 20 bytes a frame: Pong's `window.frame`,
`input.held` and `time` results).

- **`--record` costs Pong about 0.17 µs a frame** (10.3 ms over 60,000 frames), against a frame
  of about 2.1 µs (235.6 - 107.1 ms over 60,000 frames): 8% of Pong's work, nothing against a
  16.7 ms frame. At 600 frames the difference (-1.8 to +1.1 ms) is below the noise of a process
  that spends 107 ms compiling; 600 frames can't measure it, as the 1-frame row shows.

The voxel engine, hidden window, the bench's own medians over each phase's frames (ms):

| work time per frame | plain | `--record` |
|---|---:|---:|
| fill (135 frames), median | 7.61 (7.53-7.92) | 7.83 (7.67-7.85) |
| fly20 (1,200), median | 0.818 (0.797-0.821) | 0.818 (0.816-0.820) |
| fly100 (1,200), median | 6.68 (6.63-6.75) | 6.57 (6.48-6.83) |
| edits (51), median | 2.28 (2.06-2.43) | 2.16 (2.03-3.06) |
| circle (1,800), median | 0.869 (0.868-0.971) | 0.861 (0.782-0.971) |
| circle, 99th percentile | 9.00 (8.51-10.90) | 9.42 (8.55-11.25) |
| fill, whole (`fill_ms`) | 1,139 (1,099-1,165) | 1,145 (1,142-1,165) |
| whole process | 14,224 (14,071-14,519) | 14,213 (13,890-14,937) |
| headless, whole process (6 rounds) | 8,319 (8,260-8,459) | 8,410 (8,284-14,813) |

Load 3.3% before, 11.4% after (headless: 8.3% and 2.6% before, 4.8% and 11.2% after). The
replay file is 314 KB for 4,386 frames (about 72 bytes a frame).

- **`--record` costs the voxel engine nothing measurable**: every phase's median is within the
  run-to-run range (fly20 is 0.818 ms both ways), and so are the whole runs, hidden and headless.
  Its frames log more than Pong's (about 72 bytes a frame), still a fraction of a microsecond
  against frames of 0.8-8 ms. One headless recorded run took 14.8 s instead of 8.3 (1 of 6); the
  hidden runs never did, and I did not find why.
- Two exploratory hidden runs before the timed ones (one plain, one recorded) were paced by the
  display: every phase's work median was 16.6 ms and a run took 75 s, where the timed runs took
  14 s. The window asks for vsync (`FLAG_VSYNC_HINT`), and a driver may wait for it in the
  frame's first GL call, inside the work time; both modes were alike, so it doesn't change the
  comparison, but a work time of 16.6 ms in this bench means it waited for the display, not that
  the frame took that long.

## 1 MB file read and write

Load 2.4% before, 6.4% after. 20 files each; MB/s, and ms per 1 MB file:

| | Kestrel `file.*` | Rust `std::fs` (reference) |
|---|---:|---:|
| write | **315 MB/s** (264-360), 3.17 ms | 323 MB/s (245-340), 3.09 ms |
| read | **1,367 MB/s** (1,063-1,409), 0.73 ms | 1,544 MB/s (1,445-1,589), 0.65 ms |

- **Writes run at the OS's speed** (`file.write` is `std::fs::write`), which on this PC is about
  3 ms for a 1 MB file, most of it opening, truncating and closing the file with Defender
  watching. **Reads are about 10% slower than Rust's**: `file.read` reads the file into a Rust
  buffer and then copies it once into the arena's `list[u8]` (about 0.07 ms a MB); reading
  straight into the list would remove it.

## Replay check speed

`kestrel replay pong.kes C:/kt/pong-bench.replay --release` of a 600-frame recording, the whole
process: **110.5 ms** (110.4-112.2), against 111.6 for playing the same 600 frames (table
above). A replay runs at the speed of the game itself, with no window: of 60,000 frames, 242.9 ms
against 235.6 played (+3%), so for a short recording it is almost all compiling.

# Fast float math (2026-10-28)

Measured 2026-10-29 on the user's PC (Ryzen 7 5800X, 8 cores, 16 threads, Windows 11), the PC
free. `float` transcendental math (`sin cos tan asin acos atan atan2 exp log pow` and float `%`)
is now Kestrel's own wide functions (computed in `f64`, rounded once to `float`), inlined
branch-free but for one special-input branch, except `pow`, which stays a call; pure math whose
operands a loop never changes is computed once before the loop. Spec:
`docs/superpowers/specs/2026-10-28-fast-math-design.md`; plan:
`docs/superpowers/plans/2026-10-28-fast-math.md`.

**Short version:** `sin`, `cos`, `tan`, `atan2` and `log` met their per-call targets, and the
`pow`-per-particle kernel went from 33.4 to 4.4 ns (`pow` hoisted). `exp`, `pow` and float `%`
got faster but **missed** their targets. `asin`, `acos` and `atan` got **26-28% slower**, which
the spec forbids. `trig_step.kes` **missed** its 22 ns target by 0.3 ns (22.3). Every program
without hot math stayed within noise. Accuracy is at most 0.501177 ulp, and the results hash is
the same in release, debug and `target-cpu=native` builds.

**Second measurement (Task 8, below):** with the picks on float compares kept in the float
registers (3986256) and the constants of the adds and multiplies read from memory (d691776),
`atan` is now 6% faster than before the round, `exp` meets its target (3.52 ns), `trig_step.kes`
meets its target (19.28 ns), and `sin`, `cos`, `tan`, `atan2` and `log` got faster again.
`asin` and `acos` are still 5% and 9% slower than before the round, and float `%` still misses
(4.53 against 3).

## Second measurement: picks in the float registers, constants from memory (Task 8)

Measured 2026-10-29, the same PC and method (`measure.py kernels 9 --only math_ trig`, here with
four compilers in turn within each round, `bench/kernels` built with each in a `git worktree`):

- **49ae9f0**, before the round; **1804848**, the first measurement's code (3a1a713's codegen);
- **3986256**: every pick on a float compare (`atan_core`'s swap and `π/4` picks, the `exp`
  clamps, `%`'s fix-up and zero picks) is a vector compare and a blend of lane 0 (`cmpltpd`,
  `pblendvb`; the swap's two picks and the clamps become `minpd`/`maxpd`), where it was a scalar
  `ucomisd` and an integer `cmov` with each value moved to a general register and back. The
  picks on integer conditions (`tiny`, the quadrant, the sign) stay integer `select`s: a mask
  moved to the float registers measured 0-20% slower at every one of those sites (`sin` +9% for
  `tiny`, +15% for the quadrant);
- **d691776**: the constants added or multiplied go second, so x64 reads them from memory
  (`mulsd (%rip)`) instead of building each in a general register and moving it over
  (`movabsq`, `movq`) at every use, inside the loops too. Same bits (IEEE `+` and `*` commute).

Load: `math_` block 6.2% before, 13.0% during, 6.8% after; trig block 6.3%, 12.6%, 1.1%. No
Godot before or after. ns per item, median (range); net is less `math_none` (0.62) or, for
`atan2`, `pow` and `%`, `math_none2` (0.63).

| function | 49ae9f0 | 1804848 | 3986256 | d691776 | d691776 / 49ae9f0 | net d691776 | target | met? |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `sin` | 15.02 (14.91-15.16) | 4.21 | 4.20 | **3.92** (3.91-3.94) | 0.261 | 3.30 | ≤ 6 | yes |
| `cos` | 15.09 (15.04-15.23) | 3.76 | 3.76 | **3.49** (3.48-3.52) | 0.231 | 2.87 | ≤ 5.5 | yes |
| `tan` | 15.82 (15.71-15.89) | 5.10 | 5.11 | **4.66** (4.65-4.70) | 0.295 | 4.04 | no slower | yes |
| `asin` | 8.53 (8.51-8.61) | 10.86 | 10.44 | 8.96 (8.95-9.00) | **1.050** | 8.34 | no slower | **no** |
| `acos` | 8.26 (8.24-8.33) | 10.61 | 10.12 | 8.98 (8.98-9.01) | **1.087** | 8.36 | no slower | **no** |
| `atan` | 7.08 (7.04-7.10) | 8.99 | 7.78 | **6.63** (6.62-6.68) | 0.936 | 6.01 | no slower | yes |
| `atan2` | 20.29 (20.17-20.43) | 10.27 | 9.40 | **8.12** (8.11-8.16) | 0.400 | 7.49 | ≤ 11 | yes |
| `exp` | 7.30 (7.28-7.35) | 5.58 | 4.79 | **3.52** (3.52-3.56) | 0.482 | 2.90 | ≤ 4 | yes |
| `log` | 6.08 (6.06-6.14) | 4.36 | 4.36 | **3.61** (3.61-3.67) | 0.594 | 2.99 | ≤ 4.5 | yes |
| `pow` (a call) | 37.65 (37.47-37.78) | 24.24 | 24.25 | 24.23 (24.22-24.30) | 0.644 | 23.60 | ≤ 12 | **no** |
| float `%` | 5.63 (5.60-5.72) | 4.98 | 4.53 | 4.53 (4.52-4.56) | 0.805 | 3.90 | ≤ 3 | **no** |
| `trig_step.kes` | 35.23 (35.02-36.99) | 22.35 | 21.12 | **19.28** (19.25-19.30) | 0.547 | | ≤ 22 | yes |

1804848's column repeats the first measurement within 1% (its ranges are in the raw output,
`.superpowers/sdd/2026-10-28-fast-math/task-8-raw/`, local only). The picks alone (3986256) take
`atan` from 8.99 to 7.78, `exp` from 5.58 to 4.79, `%` from 4.98 to 4.53 and `trig_step.kes` to
21.12, under its target; the constants (d691776) do the rest. The picks gained `asin` and `acos`
only 4-5% (10.86 to 10.44, 10.61 to 10.12), so most of their gap is elsewhere: the inline code already beats the Rust function of the same operations (table below)
and loses to `libm`'s branchy `asinf`/`acosf`, which the spike's cost model didn't foresee.

## How it was measured

**Two compilers**, each built `--release` (and `bench/kernels` with each) in a `git worktree`
(`C:/kt/fm-before`, `C:/kt/fm-after`, removed afterwards):

- **before**: 49ae9f0, the commit before the round (`float` math widened to `f64` `libm` calls);
- **after**: 3a1a713, the round's last code commit.

Both compiled the same files, from the main checkout (the `math_<op>.kes` kernels and
`trig_step.kes` are plain Kestrel, which the old compiler compiles too). The method is the
determinism round's (`measure.py`, run from this folder; raw outputs in
`.superpowers/sdd/2026-10-28-fast-math/task-6b-raw/`, local only): every compiler in turn within
each round, at normal priority, nothing else of mine running, medians of 9 rounds (11 for the
exes, 21 and 31 for compile times) with the range. **Load** is the whole machine's CPU use, 2 s
before and after each block and over the whole block ("during" includes the timed process: one
busy thread of 16 is 6.25%). Another agent had a Godot editor open (spinning one thread) when the
session started; no timing ran until it had closed, and `tasklist` showed no Godot before or after
any block below. The per-function and `trig_step` blocks were run twice; the second run is within
2% of the first everywhere.

## Per call: the `math_<op>.kes` kernels (G3)

Each kernel fills a list (two for `atan2`, `pow`, `%`) with 65,536 random inputs from a seeded
`Rng` in the spike's ranges, then sums the function over it 100 times, through the JIT
(`bench/kernels`); the operands change every item, so the inline code is what is timed. ns per
item, median (range). **`math_none.kes`** sums `x` itself (the loop and the add: 0.63 ns before
and after); **`math_none2.kes`** sums `x + ys[i]` over two lists (two loads, a bounds check and
the adds: 0.64 ns before and after). One-argument functions are given net of `math_none`,
`atan2`, `pow` and `%` net of `math_none2` (net of `math_none` they are 0.01 ns more). The
target is the spec's, for the time per call; both ways are shown, since the loop's add chain
overlaps the math.

Load 4.6% before, 12.7% during, 9.4% after (second run: 3.9%, 8.9%, 0.9%).

| function | before | after | after / before | net before | net after | target | met? |
|---|---:|---:|---:|---:|---:|---:|---|
| `sin` | 15.17 (15.10-15.28) | **4.25** (4.20-4.30) | 0.280 | 14.54 | 3.62 | ≤ 6 | yes |
| `cos` | 15.28 (15.25-15.36) | **3.81** (3.79-3.85) | 0.249 | 14.65 | 3.18 | ≤ 5.5 | yes |
| `tan` | 15.98 (15.94-16.10) | **5.13** (5.12-5.16) | 0.321 | 15.35 | 4.50 | no slower | yes |
| `asin` | 8.65 (8.60-8.69) | 10.86 (10.84-11.01) | **1.255** | 8.02 | 10.23 | no slower | **no** |
| `acos` | 8.38 (8.32-8.44) | 10.60 (10.59-10.77) | **1.265** | 7.75 | 9.97 | no slower | **no** |
| `atan` | 7.16 (7.11-7.20) | 8.99 (8.95-9.11) | **1.256** | 6.53 | 8.36 | no slower | **no** |
| `atan2` | 20.49 (20.44-20.71) | **10.26** (10.23-10.45) | 0.501 | 19.85 | 9.62 | ≤ 11 | yes |
| `exp` | 7.37 (7.36-7.45) | 5.58 (5.58-5.68) | 0.757 | 6.74 | 4.95 | ≤ 4 | **no** |
| `log` | 6.11 (6.10-6.26) | **4.38** (4.36-4.47) | 0.717 | 5.48 | 3.75 | ≤ 4.5 | yes |
| `pow` (a call) | 37.63 (37.52-38.26) | 24.31 (24.22-24.74) | 0.646 | 36.99 | 23.67 | ≤ 12 | **no** |
| float `%` | 5.71 (5.66-5.74) | 5.03 (5.02-5.08) | 0.881 | 5.07 | 4.39 | ≤ 3 | **no** |

The before column matches the spec's "today" (the spike's figures, 15.4 for `sin` to 36.1 for
`pow`) within 1-4%.

**Supplement: the Rust functions alone** (`cargo test -p kestrel-runtime --release wide_math_speed
-- --ignored --nocapture`, `runtime/src/math/wide_speed.rs`): 65,536 inputs times 153 passes, 9
interleaved rounds, each function called through an `extern "C"` pointer. "today" is 49ae9f0's
path (`libm` in `f64`, widened and narrowed; `libm::fmodf` for `%`), "wide" the `kestrel_rt_<op>f`
function, which is the inline code's slow path and `pow` itself. ns per call, median (range); the
harness alone (an identity function) is 1.04 ns with one argument and 1.86 with two. Random
inputs: load 8.0% before, 8.7% during, 2.8% after; in order (`ORDER=seq`): 2.9%, 8.6%, 2.5%.

| function | today, random | wide, random | today, in order | wide, in order |
|---|---:|---:|---:|---:|
| `sin` | 13.30 (13.29-13.67) | 12.03 (12.02-12.21) | 6.20 (6.17-7.94) | 9.30 (9.30-9.32) |
| `cos` | 13.93 (13.74-14.13) | 6.51 (6.51-6.70) | 6.73 (6.72-6.75) | 3.02 (3.00-3.03) |
| `tan` | 14.90 (14.89-14.92) | 11.07 (11.07-11.08) | 9.27 (9.23-9.31) | 11.07 (11.07-11.08) |
| `asin` | 10.72 (10.64-10.74) | 12.13 (12.10-12.26) | 7.99 (7.98-8.14) | 6.67 (6.67-6.89) |
| `acos` | 9.58 (9.51-9.63) | 13.08 (12.84-13.11) | 7.05 (7.05-7.09) | 6.27 (6.27-6.32) |
| `atan` | 6.86 (6.40-6.86) | 4.25 (4.23-4.39) | 4.84 (4.78-4.88) | 4.11 (4.09-4.14) |
| `atan2` | 19.83 (19.79-20.26) | 13.20 (13.19-13.40) | 8.71 (8.70-8.81) | 8.39 (8.38-8.48) |
| `exp` | 6.83 (6.82-6.85) | 3.25 (3.24-3.25) | 6.76 (6.75-6.82) | 3.25 (3.25-3.29) |
| `log` | 5.48 (5.47-5.56) | 3.46 (3.46-3.48) | 5.47 (5.47-5.49) | 3.46 (3.46-3.49) |
| `pow` | 35.62 (35.58-35.98) | 24.23 (24.22-24.44) | 33.10 (33.05-33.22) | 24.24 (24.23-24.25) |
| `%` | 5.11 (5.10-5.17) | 5.21 (4.61-5.46) | 4.46 (4.45-4.47) | 5.21 (5.20-5.23) |

What the two tables say together:

- **`pow`** is a call to the wide Rust function in both tables, 24.2-24.3 ns: the inline route
  was a non-goal, and the target of 12 needed a faster `powf` (followups entry 52).
- **Random inputs are hard on `libm`** (its branches mispredict): `sin` today is 13.3 ns random
  and 6.2 in order. The kernels use random inputs, as the spec says. In order, the inline `sin`
  and `cos` (branch-free, so the same speed either way) would still be ahead of today's.
- **For `atan`, `exp` and `log` the Rust function, the same operations built by LLVM and called
  through a pointer, costs less than the inline Cranelift copy**: `atan` 4.25 ns with the call
  against 8.99 inline, `exp` 3.25 against 5.58, `log` 3.46 against 4.38 (the two timings use
  different loops, so this is a pointer, not a measurement of the fix). Why wasn't looked into
  here. The inline code is branch-free, so it pays for every range's reduction on every item,
  where today's `libm` branches; for `asin`, `acos` and `atan` that cost more than it saved.
  Calling the Rust function for `atan` and `exp` instead of inlining them may be the fix; it is
  not measured here.

## Kernels (G4)

`bench/kernels` through the JIT, 1 thread, 9 rounds; ns per item (per particle or mover).

| Kernel | before | after | after / before | target | met? |
|---|---:|---:|---:|---:|---|
| `step_pow.kes` (`pow(KEEP, dt)` per particle) | 33.44 (33.39-33.77) | **4.38** (4.34-4.41) | **0.131** | ≤ 5 | yes |
| `step.kes` (`pow` hoisted by hand) | 4.35 (4.33-4.37) | 4.36 (4.33-4.39) | 1.002 | | |
| `step_index.kes` | 5.65 (5.62-5.74) | 5.66 (5.62-5.68) | 1.002 | | |
| `step_scalar.kes` | 4.35 (4.32-4.38) | 4.36 (4.34-4.38) | 1.002 | | |
| `trig_step.kes` (`atan2`, `cos`, `sin` per mover) | 35.52 (35.12-35.73) | 22.34 (22.30-22.48) | 0.629 | ≤ 22 | **no** |
| `noise_scalar.kes` checked | 6.71 (6.66-6.73) | 6.71 (6.70-6.71) | 1.000 | ±2% | yes |
| `noise_scalar.kes --release` | 6.62 (6.62-6.63) | 6.63 (6.62-6.63) | 1.002 | ±2% | yes |
| `noise_corners.kes` checked | 5.45 (5.44-5.45) | 5.45 (5.44-5.45) | 1.000 | ±2% | yes |
| `noise_corners.kes --release` | 5.41 (5.41-5.44) | 5.42 (5.41-5.44) | 1.002 | ±2% | yes |
| `noise_wide.kes` checked | 2.70 (2.70-2.70) | 2.70 (2.70-2.70) | 1.000 | ±2% | yes |
| `noise_wide.kes --release` | 2.71 (2.70-2.71) | 2.71 (2.70-2.71) | 1.000 | ±2% | yes |
| `iface_step.kes` | 4.33 (4.30-4.36) | 4.32 (4.30-4.36) | 0.998 | | |

Load: step 4.6% before, 12.4% during, 4.8% after; trig 3.8%, 11.4%, 7.4%; noise 6.2%, 11.3%,
7.4%. `trig_step.kes` was 22.34, 22.33 and 22.31 ns in three blocks (the step block, the trig
block, and the second run with the per-function block), so it misses by 0.3 ns (1.5%). The spike
estimated 19.8; the inline `atan2` (9.6 ns net) is most of it. **`step_pow.kes` now matches
`step.kes`, which hoists `pow` by hand** (4.38 against 4.36): `pow(KEEP, dt)` runs once before the
loop. Rust references in the same rounds: `step` 3.75 ns, `noise_scalar` 6.10, `noise_wide` 2.40.

## Programs (G4)

**Exes** (the whole run minus the empty program's, about 6 ms; ms, median (range of the whole
runs)):

| | before | after | after / before |
|---|---:|---:|---:|
| frozen world `bench/voxels3d/main.kes`, checked | 419.1 (423.2-560.3) | 418.4 (422.5-538.5) | 0.998 |
| frozen world, `--release` | 375.8 (379.6-490.8) | 376.3 (379.3-429.7) | 1.001 |
| generation, 64 columns through `load_now`, `--release` | 302.0 (306.6-348.2) | 302.3 (306.9-347.2) | 1.001 |

Load: frozen world 7.0% before, 13.6% during, 4.3% after; generation 5.0%, 11.9%, 6.7%. Both
print the same results (463,749 faces; 18,500).

**Engine** (`kestrel/bench/engine`, `voxel/bench.kes --release` in a hidden window, 3 runs per
compiler in turn; work p99 per phase, ms, median (range)):

| Phase | before | after | after / before |
|---|---:|---:|---:|
| fill | 10.33 (9.80-11.63) | 9.48 (9.24-12.21) | 0.918 |
| fly20 | 8.31 (8.24-8.33) | 8.13 (8.04-8.14) | 0.978 |
| fly100 | 8.46 (8.45-8.56) | 8.33 (8.24-8.42) | 0.985 |
| edits (p99 = max) | 5.12 (4.67-5.34) | 5.02 (4.84-5.09) | 0.980 |
| circle | 8.51 (8.42-8.64) | 8.52 (8.37-8.53) | 1.001 |
| generation, ms per column (circle `gen_ms`) | 2.44 (2.44-2.46) | 2.45 (2.45-2.46) | 1.004 |

Load per run: 0.6-6.4% before, 3.5-7.2% others during. The engine calls a few `sin`, `cos`,
`tan` and `pow` per frame (camera and frustum), too few to show. No phase got slower; fill's p99
is 8% lower, but its two ranges overlap almost entirely (fill's p99 is its noisiest number, as in
the determinism round's table below).

## Compile time

`measure.py compile` (new in this round): `kestrel check --release` (the front end, the checks and
KIR with its passes, no machine code), `kestrel run --release` (that plus Cranelift into memory and
the program's `main`; not for `voxel/`, which opens a window) and `kestrel build --release` (that
plus the object file and the link), every compiler in turn. `voxel/main.kes`, 21 rounds (load
7.1% before, 13.2% during, 9.1% after):

| `voxel/main.kes` | before | after | after / before |
|---|---:|---:|---:|
| `check --release` | 215.1 (211.3-220.8) | 222.4 (217.4-246.0) | **1.034** |
| `build --release` | 320.4 (313.7-340.2) | 332.4 (323.8-366.1) | **1.037** |

**Compiling the voxel engine takes about 3.5% longer, 7 ms of it before code generation** (two
9-round blocks agree: `check` 1.033 and 1.017, `build` 1.024 and 1.002). `check` runs the same
front end at both commits, so the extra time is in KIR; loop hoisting (`kir/src/hoist.rs`) is the
round's new KIR pass. Machine code (`build` minus `check`) grew by about 5 ms, 105 to 110. The
empty program's build (the link) ranges 62-111 ms, so the builds are not shown net of it.

**The small kernels, 31 rounds** (load 2.4%, 9.8%, 3.6%): `kestrel run --release` minus `run` of
the empty program is 3-6 ms per kernel with either compiler. Summed over the 13 `math_*.kes`
kernels, using each one's fastest run, it is 42.0 ms before and 47.8 after (+0.45 ms a kernel,
+14%); `trig_step.kes` 3.9 against 6.1 ms. That is the inline math's machine code. Process times
here fall into two groups (about 14 and 19 ms) from run to run, so a single kernel's median moves
by ±3 ms; the totals are the readable number. `check` didn't move (10.2 against 9.9 ms in total).
The kernels' builds are lost in the link's noise.

## Accuracy and same bits

**Same bits everywhere:** the runtime's results hash (`wide_results_hash_is_pinned`, FNV-1a of
every wide function's results over its sweep) is `0xF8962BD9040E8F6C` in a release build, a debug
build and a `RUSTFLAGS=-C target-cpu=native` release build (own target folder, so AVX2 and FMA
were available to LLVM); all three passed. `kestrel check --report` and `--report --release` print
the same bytes with both compilers for the frozen world, and for `voxel/main.kes` and every
`bench/kernels/*.kes` (68 pairs, 0 differ): hoisting doesn't change the report.

**Accuracy** (`print_wide_report`, the 1M-input sweep per function, ulps against `libm`'s `f64`
result; "same as today" counts bit-identical results with 49ae9f0's):

| function | inputs | max ulp, sweep | max ulp, every `f32` (input) | mean ulp | same as today, sweep | differ from today, every `f32` |
|---|---:|---:|---:|---:|---:|---:|
| `sin` | 1,000,101 | 0.5009 | 0.500900 (5.5782076e7) | 0.2317 | 99.9773% | 146,766 |
| `cos` | 1,000,101 | 0.5009 | 0.500900 (1.5705271e4) | 0.2320 | 99.9721% | 170,626 |
| `tan` | 1,000,101 | 0.5010 | 0.501177 (3.4509312e8) | 0.2314 | 99.9641% | 221,788 |
| `asin` | 1,000,101 | 0.5000 | 0.500012 (1.8791507e-3) | 0.1928 | 100.0000% | 1,498 |
| `acos` | 1,000,101 | 0.5000 | 0.500007 (9.779774e-1) | 0.2402 | 100.0000% | 55 |
| `atan` | 1,000,101 | 0.5000 | 0.500012 (7.618335e-3) | 0.2132 | 99.9999% | 1,502 |
| `exp` | 1,000,101 | 0.5000 | 0.500013 (-5.5412523e-2) | 0.1763 | 99.9997% | 1,431 |
| `log` | 1,000,101 | 0.5000 | 0.500000 (1.5856676) | 0.2244 | 100.0000% | 9 |
| `atan2` | 1,010,201 | 0.5000 | (two arguments) | 0.2650 | 99.9980% | |
| `pow` | 1,010,201 | 0.5000 | (two arguments) | 0.1267 | 99.9836% | |
| `fmod` (float `%`) | 1,010,201 | 0 (exact) | (two arguments) | 0 | 100.0000% | |

The "every `f32`" column is `exhaustive_one_argument_functions` (all 2^32 inputs, 359 s on 8
threads), run again at 3a1a713: the same maxima and counts as at Task 1 (e25d79a), as expected,
since later commits changed only `pow` and the sweep's inputs. **Every function is within the
0.51 ulp gate**; out of 4.3 billion inputs at most 221,788 (0.005%, `tan`) differ from today's
bits, each by one ulp. `pow`'s "same as today" is lower than at Task 1 (99.9836% against
99.9995%) because its sweep changed at b45d8c7: 10% of its random inputs are now integer and
dyadic `x` with small integer and half exponents, whose results are often exact or exact
midpoints, where the wide path can round differently from `libm` (below). Its maximum is still
0.5000 ulp. (Not traced input by input.)

**`pow`:** `pow(x, y)` for `y` of exactly 1, -1, 2 and 0.5, with `x > 0`, returns `x`, `1/x`,
`x*x` and `sqrt(x)`, each one `f64` operation rounded once to `float`. It tests `y`'s bits, so its
value, whether it came from a literal, a constant or a computation. That is what `libm::pow`
returns early for those exponents, so they keep the old bits, and it equals the same expression
written in `float` (`pow(x, 2.0) == x*x`). Zeros, infinities, NaN and negative `x` take the `libm`
path, with today's results (`pow(-0, 0.5)` is `+0` and `pow(-inf, 0.5)` is `+inf`, as in C, while
`sqrt` of those gives `-0` and NaN). Every other exponent, `-2` included, takes the wide path,
within 0.5 ulp: an exact `1/(x*x)` would be neither the old bits nor provably correctly rounded.
The wide path can round an exact midpoint away from even where `libm` rounded to even:
`pow(2, -150)` is `2^-149` (the smallest subnormal), where the old code gave 0.

**Program output that changed** (all checksums of simulations, which mix every float result into
one number; the raw values on the other lines didn't change):

- `tests/programs/det_par.kes` line 33: `-4727177381731691941` to `1766166045610435347`;
- `tests/programs/det_sim.kes` lines 87-90 (frames 250, 500, 750, 1000):
  `8833392080462543213` to `6629063034239713950`, `-7362058023118986372` to
  `-8923671571335071607`, `2187843396518032676` to `-3003307686104160227`,
  `-3165968733279055487` to `-2363078924110686833`;
- `tests/programs/det_math_float.kes`, the `pow` checksum: `323258267209170825` (Task 2's wide
  path) to `-2935346233227922869`, when the exact exponents went in: 2 of its 2,000
  exact-exponent inputs changed, and both are now the old bits.

The 36 `det_math` float rows (raw values) and the 163 pinned `FLOAT_TABLE` rows didn't change.
`hoist_math.kes` case 1 (a sequential `for` and a `par for` over the same `dt`) now computes
`pow(KEEP, dt)` once before both loops, since a `par for` passes a `float` capture as a copy;
`int` captures are still passed by pointer (followups entry 55).

## The goals, met or missed

**After Task 8 (d691776, the second measurement above):** G3: `sin`, `cos`, `tan`, `atan2`,
`log` **met**, now with `exp` (3.52, ≤ 4) and `atan` (6.63 against 7.08, no slower) **met** too;
`asin` (8.96 against 8.53, +5%) and `acos` (8.98 against 8.26, +9%) **missed**, though from +27%;
`pow` (24.23) and float `%` (4.53) **missed**. G4: `trig_step.kes` **met** (19.28, ≤ 22). The
other G4 rows were not measured again: the change touches only the inline math. G1 and G2:
the same bits (the inline-against-Rust sweep, the `det_math`, `math`, `vec` and `hoist` programs,
the determinism tests and `WIDE_HASH` pass unchanged).

The first measurement (1804848), kept as it was:

**G3, speed per call** (the `math_<op>.kes` kernels; met or missed is the same gross and net):

- `sin` ≤ 6: **met**, 4.25 ns (3.62 net), from 15.17.
- `cos` ≤ 5.5: **met**, 3.81 (3.18 net), from 15.28.
- `exp` ≤ 4: **missed**, 5.58 (4.95 net), from 7.37.
- `log` ≤ 4.5: **met**, 4.38 (3.75 net), from 6.11.
- `pow` ≤ 12: **missed**, 24.31 (23.67 net), from 37.63; `pow` is still a call.
- `atan2` ≤ 11: **met**, 10.26 (9.62 net), from 20.49.
- float `%` ≤ 3, inlined: **missed**, 5.03 (4.39 net), from 5.71; it is inlined.
- `tan` no slower: **met**, 5.13 from 15.98.
- `asin`, `acos`, `atan` no slower: **missed**: 25-27% slower (10.86 from 8.65, 10.60 from 8.38,
  8.99 from 7.16).

**G4, speed in kernels and programs:**

- `trig_step.kes` ≤ 22 ns: **missed**, 22.34 (three blocks: 22.31-22.34), from 35.52.
- `step_pow.kes` ≤ 5 ns: **met**, 4.38, from 33.44, level with `step.kes`'s hand-hoisted 4.36.
- Noise kernels within ±2%: **met**, at most +0.2%.
- The frozen world within ±2%: **met**, -0.2% checked, +0.1% `--release`; generation +0.1%.
- The engine's p99 within ±2%: **met**, in that no phase got slower than +0.1% (circle); fly20,
  fly100 and edits are 1.5-2.2% faster, and fill 8% faster with overlapping ranges.
- Not a goal, but asked for: compiling `voxel/main.kes` takes 3.5% longer (7 ms in KIR, 5 ms in
  machine code); each small math kernel's JIT compile about 0.5 ms more.

**Accuracy and determinism** (G1, G2): **met**. Every function is at most 0.501177 ulp from
`libm`'s `f64` result, the stand-in for the true value (the gate is 0.51); the runtime's results
are the same bits in release, debug and host-CPU builds. The JIT against exe half of G2 is the
test suite's (Task 2: the 163 pinned rows and 13 sweep checksums are the same in the JIT, a
checked exe and a `--release` exe), not measured here.

**Follow-ups this points at:** call the Rust function instead of inlining `atan` (and perhaps
`asin`, `acos` and `exp`), where LLVM's build of the same code is faster than Cranelift's inline
copy; a faster `powf` (followups entry 52); and, if the compile time matters, the hoisting pass's
cost (followups entry 54).

# Determinism round 1 (2026-10-27)

Measured 2026-10-28 on the user's PC (Ryzen 7 5800X, 8 cores, 16 threads, Windows 11), the user
away and the PC free. What the round costs and gains: Kestrel's own math (`libm`) against the
platform's, pinning the float mode (MXCSR), compiling for the SSE4.1 baseline instead of the host
CPU, and snapshot copies and `checksum`.

## How it was measured

**Three compilers**, each built `--release` in a `git worktree` with its own short target folder
(`C:/kt/det-<name>`, removed afterwards):

- **before**: c743e8a, the commit before the round (platform math, no pinning, host ISA);
- **after**: 45d39e7, the round's last commit (`libm`, pinning, the SSE4.1 baseline for exes and
  the JIT);
- **host**: 45d39e7 with `codegen/src/isa.rs` changed back to `cranelift_native::builder()` (the
  host's features: AVX2, FMA, BMI and so on), a scratch change that was never committed. **after
  against host is the baseline ISA's effect alone**; before against after is the whole round.

All three compiled the same sources (`voxel/`, `bench/voxels3d/` and `bench/kernels/` are the same
at both commits). Every exe prints the same result for all three (463,749 faces, generation 18,500,
the snapshot and checksum values), and the script stops if one doesn't.

**Method** (`measure.py`, run from this folder; raw outputs in
`.superpowers/sdd/2026-10-27-determinism/task-12-raw/`, local only): each block runs every
compiler's program once per round, in turn, at normal priority, nothing else of mine running.
Exes: the matching empty program's median (process start-up, about 6 ms; for the snapshots, the
state's set-up) is subtracted. Medians of 9 rounds (11 for the frozen world and generation), with
the range of the raw times. **Load** is the whole machine's CPU use, sampled for 2 s before and
after each block and over the whole block ("during" includes the timed process: one busy thread
of 16 is 6.25%). OneDrive used about one thread of its own (about 6%) for most of the session, so
"before" loads are mostly 6-8%; it affects all three compilers alike, since they run in turn.

## Own math against the platform

`cargo test -p kestrel-runtime --release own_math_against_the_platform -- --ignored --nocapture`
(`runtime/src/math/speed.rs`), 10,000,000 calls per function over a spread of inputs, 9 runs
(`measure.py math 9`). Load 0.4% before, 6.8% during, 0.6% after. ns per call, median (range):

| Function | `libm` (now) | platform (before) | ratio |
|---|---:|---:|---:|
| `sin` | 6.09 (6.07-6.13) | 5.22 (5.21-5.23) | 1.17 |
| `cos` | 6.22 (6.20-6.26) | 5.12 (5.11-5.17) | 1.21 |
| `tan` | 7.82 (7.81-7.85) | 6.45 (6.22-6.53) | 1.21 |
| `asin` | 8.33 (8.30-8.45) | 5.41 (5.33-5.44) | 1.54 |
| `acos` | 7.54 (7.50-7.57) | 5.04 (4.93-5.12) | 1.50 |
| `atan` | 5.37 (5.36-5.41) | 4.84 (4.75-4.89) | 1.11 |
| `atan2` | **8.89** (8.88-8.97) | 9.72 (9.69-10.26) | **0.91** |
| `exp` | 7.34 (7.33-8.03) | 3.91 (3.89-3.98) | 1.88 |
| `log` | 6.64 (6.63-6.73) | 4.61 (4.60-4.64) | 1.44 |
| `pow` | 32.81 (32.78-32.98) | 12.29 (12.26-12.52) | **2.67** |
| float `%` (`fmod`) | 7.51 (7.49-7.72) | 3.93 (3.91-3.94) | 1.91 |

**Kestrel's math is slower, by 1.1 to 2.7 times per call; only `atan2` got faster.** The platform's
functions (the MSVC runtime) pick FMA code on this CPU, which is exactly what made their results
differ between machines. In a program it shows where math is in the hot loop: `step_pow.kes` (one
`pow` per particle, through the JIT) went from **16.59 to 33.45 ns per particle, 2.02 times slower**
(kernels table below). Where the math is hoisted out of the loop (`step.kes`) or absent (the voxel
world, generation, the engine), nothing changed.

## Pinning the float mode

`pinning_the_float_mode` (same file), 100,000,000 iterations, 9 runs, same block as above:

| | ns per iteration |
|---|---:|
| empty loop | 0.21 (0.21-0.21) |
| `fpu::pin` inlined (one `ldmxcsr`) | 0.41 (0.41-0.42) |
| `kestrel_rt_fpu_pin` called through a pointer, as compiled code calls it | 4.14 (4.14-4.15) |

**Per frame:** a scratch counter in the runtime (`task-12-raw/pincount.patch`, not committed)
counted the pins compiled code makes after window-library natives: the voxel engine bench pins
5,467,240 times over 20,586 frames (**266 a frame, about 1.1 µs a frame**, 0.007% of a 16.7 ms
frame), rollback Pong (headless, 600 frames) 24,002 times (**40 a frame, about 0.17 µs**). Pinning
at `main` and when a thread joins a `par for` job happens once per program or job. Not measurable
in any program below.

## The baseline CPU, and the whole round, on programs

**Exes** (whole run minus the empty program's 6 ms; ms, median (range of the whole runs)):

| | before | after | host | after / before | after / host |
|---|---:|---:|---:|---:|---:|
| frozen world `bench/voxels3d/main.kes`, checked | 425.4 (429.5-472.5) | **418.5** (423.0-459.8) | 425.1 (429.9-476.4) | 0.984 | 0.984 |
| frozen world, `--release` | 369.9 (373.3-424.9) | 375.1 (378.7-417.4) | 370.1 (373.4-412.1) | 1.014 | 1.014 |
| generation, 64 columns through `load_now`, `--release` | 299.7 (303.8-357.1) | 301.7 (306.4-350.6) | 299.2 (303.9-343.8) | 1.007 | 1.008 |

Load: frozen world 6.8% before, 14.5% during, 7.7% after; generation 7.2%, 13.2%, 6.5%. The high
ends of the ranges are each block's first, cold round. **The baseline ISA moves the exes by -1.6%
to +1.4%**, in both directions: within what code layout alone moves these programs
(`bench/voxels3d/results.md`).

**JIT, whole programs** (`kestrel run --release`, compile included, minus `kestrel run` of the
empty program; 9 rounds; load 6.8% before, 13.2% during, 7.3% after):

| | before | after | host | after / host |
|---|---:|---:|---:|---:|
| frozen world | 370.4 (430.5-442.1) | 370.4 (431.3-439.9) | 371.0 (431.4-439.8) | 1.00 |
| generation | 352.4 (362.9-370.5) | 355.1 (365.3-369.0) | 353.6 (362.5-368.5) | 1.004 |

**JIT, kernels** (`bench/kernels`, which compiles each kernel the way `kestrel run` does and times
it through the JIT; 1 thread, 9 rounds, every compiler in turn within a round; ns per item, or per
chunk for the race; median of the rounds' medians (range)):

| Kernel | before | after | host | after / before | after / host |
|---|---:|---:|---:|---:|---:|
| `race_faces.kes` checked | 83,610 (82,846-84,294) | 84,089 (83,233-84,865) | 83,586 (82,728-84,387) | 1.006 | 1.006 |
| `race_faces.kes --release` | 66,515 (66,118-67,570) | 66,641 (66,232-67,517) | 66,659 (66,086-67,617) | 1.002 | 1.000 |
| `race_arrays.kes` checked | 128,085 (126,691-129,816) | **124,999** (123,731-126,781) | 128,260 (126,536-129,383) | 0.976 | **0.975** |
| `race_arrays.kes --release` | 103,795 (102,738-105,410) | **101,318** (100,074-102,540) | 104,026 (103,141-105,544) | 0.976 | **0.974** |
| `noise_scalar.kes` checked | 6.65 (6.65-6.70) | 6.71 (6.70-6.72) | 6.66 (6.65-6.67) | 1.009 | 1.008 |
| `noise_scalar.kes --release` | 6.55 (6.55-6.55) | 6.63 (6.62-6.63) | 6.55 (6.54-6.56) | 1.012 | 1.012 |
| `noise_corners.kes` checked | 5.29 (5.29-5.31) | **5.45** (5.45-5.45) | 5.29 (5.29-5.29) | 1.030 | **1.030** |
| `noise_corners.kes --release` | 5.26 (5.26-5.27) | **5.42** (5.41-5.42) | 5.26 (5.26-5.27) | 1.030 | **1.030** |
| `noise_wide.kes` checked | 2.60 (2.60-2.60) | **2.70** (2.70-2.70) | 2.60 (2.60-2.60) | 1.038 | **1.038** |
| `noise_wide.kes --release` | 2.58 (2.58-2.59) | **2.71** (2.70-2.71) | 2.58 (2.58-2.59) | 1.050 | **1.050** |
| `step.kes` | 4.29 (4.29-4.31) | 4.32 (4.32-4.34) | 4.29 (4.28-4.30) | 1.007 | 1.007 |
| `step_index.kes` | 5.51 (5.48-5.54) | 5.58 (5.56-5.60) | 5.51 (5.50-5.55) | 1.013 | 1.013 |
| `step_scalar.kes` | 4.29 (4.29-4.32) | 4.32 (4.32-4.34) | 4.29 (4.29-4.32) | 1.007 | 1.007 |
| `step_pow.kes` (`pow` per particle) | 16.59 (16.54-16.82) | **33.45** (33.30-34.18) | 33.26 (33.22-34.99) | **2.02** | 1.006 |
| `iface_step.kes` | 4.30 (4.29-4.57) | 4.30 (4.29-4.67) | 4.31 (4.30-4.58) | 1.00 | 1.00 |
| `iface_step_direct.kes`, `iface_step_generic.kes` | 0.42 | 0.42 | 0.42 | 1.00 | 1.00 |

Load: race 6.8% before, 11.5% during, 1.6% after; noise 1.4%, 7.3%, 9.2%; step 8.2%, 9.8%, 1.0%.
Rust references in the same rounds: `race_faces` 48,086, `race_arrays` 57,029, `noise_scalar`
6.11, `noise_wide` 2.40, `step` 3.75 ns.

**The JIT loses 3.0% on `noise_corners` and 3.8-5.0% on `noise_wide` to the baseline ISA, and
gains 2.5% on `race_arrays`.** The noise kernels' code shows why: with the host's AVX, Cranelift
uses three-operand VEX instructions and needs no register copies (`noise_wide`'s `bench` is 195
instructions with the host ISA against 223 with the baseline: 22 `movdqa` copies gone, splats as
one `vpbroadcastd`; `noise_corners` 135 against 146; `task-12-raw/dump-mnemonics.txt`). The race
gain isn't explained by an instruction count; it is the same size in both directions as other
layout effects seen here.

**Engine** (`kestrel/bench/engine`, `voxel/bench.kes --release` in a hidden window, 3 runs per
compiler in turn; work p99 per phase, ms, median (range)):

| Phase | before | after | host | after / host |
|---|---:|---:|---:|---:|
| fill | 10.31 (9.03-12.86) | 9.83 (9.38-10.63) | 10.16 (9.92-11.21) | 0.97 |
| fly20 | 8.04 (8.01-8.20) | 8.38 (8.22-8.42) | 8.20 (8.03-8.39) | 1.02 |
| fly100 | 8.41 (8.23-8.58) | 8.52 (8.34-8.66) | 8.43 (8.22-8.65) | 1.01 |
| edits (51 frames: p99 = max) | 5.38 (4.89-5.55) | 5.54 (4.90-5.61) | 5.16 (4.90-5.42) | 1.07 |
| circle | 8.42 (8.21-8.50) | 8.41 (8.33-8.59) | 8.30 (8.29-8.49) | 1.01 |
| generation, ms per column (circle `gen_ms`) | 2.46 (2.43-2.47) | 2.48 (2.45-2.49) | 2.46 (2.42-2.46) | 1.01 |

Load per run: 0.5-7.4% before, 0.9-7.3% others during (the first two rounds had OneDrive's thread,
the last had none). Every p99 is far inside E32's 16.7 ms. The ranges of after and host overlap in
every phase; fly20 p99 is 4% above before (8.38 against 8.04), and those two ranges don't overlap
(8.22-8.42 against 8.01-8.20). The
first engine block (`task-12-raw/engine.txt`) was thrown away: in its first two runs (before and
after) the display waited for vsync (work median 16.6 ms in every phase, runs of 344 and 236 s
instead of 56), so those p99s measure the display, not the code. Its other seven runs agree with
the table.

## Snapshots and `checksum`

`snapshot.kes` (`measure.py snap 9`): **Pong's `Game`** (a copy, with its `Rng` written out as a
struct of one `i64` so the old compiler compiles it) saved into a ring of 16 and restored, and a
**voxel-sized `Pool[Column]`** (441 columns of 32 x 128 x 32 `u8` blocks, 58 MB) saved into a
snapshot and restored. A pair is one save and one restore; the loop's other work and the set-up are
taken out by the control programs (`pong_control`, `pool_pairs(0)`). Load 6.1% before, 13.7%
during, 6.7% after.

| | before (copy into new memory) | after (reuse) | host |
|---|---:|---:|---:|
| `Game`, ns per pair (100,000,000 pairs) | 1.08 (whole 175.6-181.4 ms) | 1.09 (176.2-180.0) | 1.09 (175.3-179.5) |
| `Game`, ns per pair (1,000,000 pairs, as the plan asks) | 1.3 (7.8-11.0) | 1.6 (7.7-11.4) | 1.3 (7.7-11.3) |
| `Pool`, ms per pair, 10 pairs | **48.7** (whole 534-603 ms) | **19.4** (235-251) | 18.8 (234-249) |
| `Pool`, ms per pair, 100 pairs | - (would need 11.6 GB) | **19.1** (1,862-1,972) | 18.8 (1,871-1,966) |
| `Pool` program, peak private memory, 10 pairs | **1,225 MB** | **113 MB** | 113 MB |
| `Pool` program, peak private memory, 100 pairs | - | 113 MB | 113 MB |

- **Reuse makes a voxel-sized snapshot pair 2.5 times faster and stops the growth**: before, every
  save and every restore allocated a fresh 58 MB (1,225 MB after 10 pairs, so 100 pairs would need
  11.6 GB and weren't run); after, the program stays at its set-up's 113 MB (the pool and one
  snapshot) for 10 and 100 pairs. A pair is two 58 MB copies, about 6 GB/s.
- **Pong's `Game` is level** (about 1 ns a pair either way): it holds no list, so it was always a
  plain copy; the 1,000,000-pair row is below the timer's resolution (whole runs of 7.7-11.4 ms,
  of which the set-up takes 6.6; the measured net is 1.3 ms before and 1.6 ms after) and is given only because the plan asks for it.

`checksum` (today's compiler only; the old one has none), with its controls:

| | after | host |
|---|---:|---:|
| `checksum(g)` of a `Game`, ns per call (1,000,000 calls) | 6.52 (whole 12.4-21.2 ms) | 6.73 (12.6-17.3) |
| `checksum(p)` of the 58 MB `Pool`, ms per call (20 calls) | 72.1 (whole 1,466-1,485 ms) | 71.9 (1,464-1,468) |

A pool checksum is about 1.25 ns per block (0.8 GB/s): it folds one `u8` at a time, a multiply
chain. Fine for a desync check every 30 frames on a Pong-sized state; for a voxel world, checksum
the columns that changed.

## Where Kestrel got slower

- **Math calls: 1.1 to 2.7 times slower per call** (`pow` 2.67, `fmod` 1.91, `exp` 1.88); a kernel
  with `pow` per item is **2.02 times slower**. This is the price of the same bits on every CPU.
- **The JIT with the baseline ISA: 3.0% slower on `noise_corners`, 3.8-5.0% on `noise_wide`**, 0.7
  to 1.3% on `noise_scalar` and the `step` kernels; 2.5% faster on `race_arrays`.
- **Exes with the baseline ISA: the frozen world `--release` 1.4% slower** (checked 1.6% faster),
  generation 0.8% slower, engine p99 within 1-2% in most phases (edits, 51 frames: 7% against a host-ISA build and 3% against before; fly20 +4% against before, see the engine section).
- Pinning the float mode: nothing measurable.

## The JIT's ISA: a proposal for the controller

The JIT lost more than 2% (`noise_corners` 3.0%, `noise_wide` 3.8-5.0%), so by spec decision 4 the
host ISA can come back for the JIT without touching the promise (no FMA is ever emitted; AVX
encodings compute the same IEEE operations; `det_math` and the cross-build test would still pin
JIT against exe output). **Proposed: the JIT compiles for the host CPU again; exes keep the SSE4.1
baseline.** Against it: one ISA means the JIT and an exe compile the same instructions, so a
host-only lowering bug can't make them disagree; and the host ISA made `race_arrays` 2.5% slower.
The controller rules.
