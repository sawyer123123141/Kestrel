# Frontier roadmap proposal (2026-11-01)

The user's direction today: "I want this to be a frontier language." This proposal says what that can mean for Kestrel in claims that can be measured, lists every known gap with its number and cost, says what the performance gaps need (a better KIR, or a second backend), and picks an order for the next 4-8 weeks. It builds on `docs/superpowers/plans/2026-10-29-roadmap-proposal.md` (steps 1-4 done: the I/O round, hot reload, the rollback library; steps 5-8 open: GPU round 1, Linux, the errors-as-values and packages proposals, small tooling). Names and syntax are the user's, as always; where a pick is written out below, the user can say yes or rename it.

Costs are days of agent work at this project's pace (the I/O, hot reload and rollback rounds each took about a day to a day and a half): **S** is a day or less, **M** 2-3 days, **L** 4 days or more. Ratios are Kestrel's time over Rust's, `--release` against Rust release, from the file named.

## 1. What frontier means here

Speed near Rust is the floor, not the claim: Zig, Odin and Jai are all near C. The frontier is what no other game language promises. Five claims Kestrel could make and measure:

1. **"Your game cannot desync: the same bits on every machine, and the compiler proves which code is deterministic."** Today: `@prove(deterministic)`, Kestrel's own float math (at most 0.51 ulp, the same bits in release, debug and `target-cpu=native` builds; `kestrel/bench/determinism/results.md`, "Fast float math"), one SSE4.1 ISA for the JIT and exes (`docs/superpowers/specs/2026-10-27-determinism-design.md`, decision 4). GGPO and its Rust twin GGRS leave determinism to the game; Unity, Godot, Rust, Zig and Odin promise nothing about float bits across machines. **Needs:** evidence from a second CPU and a second OS (no such run exists: one user, one PC), so CI and Linux; later ARM. **Measure:** a Pong replay recorded on the user's PC checks bit for bit on a CI runner's different CPU, on Windows and on Linux.
2. **"Rollback netcode is five calls a frame, and the compiler checks the step."** Today: `use rollback`, a Pong session frame 38 ns, 132 ns rolling back 8 frames every frame, memory flat after 200 ring laps (`kestrel/bench/determinism/results.md`, "The rollback library"). **Needs:** real UDP (determinism round 3), acknowledgements and resending (follow-up 85), N players (82), a second machine. **Measure:** two machines play an hour over a real network with no desync, and a planted desync is reported with the field that differs.
3. **"Edit the game while it runs, state kept, and replay any session bit for bit."** Today: a save reloads Pong and the voxel engine in 0.28 s with the release `kestrel`; `--record` costs Pong 0.17 µs a frame (`kestrel/bench/determinism/results.md`, "The hot reload round", "The I/O round"). Godot reloads scripts and Rust has third-party reloaders, but none pairs a state-keeping reload with a bit-exact replay. **Needs:** recording across reloads (81), a reload during a rollback (76), the time-travel debugger (roadmap step 8). **Measure:** step a recorded bug back frame by frame, fix the code, and replay the same input against the new code.
4. **"The frame cannot stutter or crash, and the compiler says which line would make it."** Today: `@prove(no_alloc, no_panic, no_checks, deterministic, inline)`; checks cost 0-1% on the noise kernels and the tuned voxel world (`kestrel/bench/kernels/results.md`, "Quiet reruns"; `kestrel/bench/voxels3d/results.md`, "Quiet rerun"). Unity's Burst restricts a job to a subset; it does not prove a whole frame. **Needs:** natives marked safe (so a frame that draws can prove `no_alloc`; syntax spec "Not built yet"), contracts per generic instance (33, 84), facts across calls. **Measure:** Pong's and the voxel engine's `frame` prove `no_alloc` and `no_panic`, drawing included.
5. **"One `pure` function runs on every core or on the GPU, and races are ruled out by the checker."** Today: `par for` builds the voxel world in 78 ms on 16 threads against 491-505 ms on one, where C#'s `Parallel.For` takes 357-391 ms (`kestrel/bench/kernels/results.md`, "`bench/2a`, `heightmap`, `voxels3d`, `maps`"). **Needs:** GPU rounds 1-3 (`docs/superpowers/plans/2026-10-27-gpu-proposal.md`), work that spans frames and nested `par for` (engine entries 4 and 8). **Measure:** the particle demo at 1M on the GPU with the same `step` the CPU runs (the proposal expects 2-4 ms of frame work against 41 ms measured; not measured yet). GPU floats will not be bit-identical to the CPU's, so claim 1 stays CPU-only and must say so.

What the others offer, as far as is certain (no numbers here, since none were measured against Kestrel beyond Rust and C#):

| Language or tool | Strength Kestrel is measured against | What it leaves to the programmer |
|---|---|---|
| Rust, with Bevy | LLVM speed; Cargo and crates; Bevy runs systems in parallel from their declared data access | float determinism; rollback through GGRS; code hot reload is third-party |
| Zig | LLVM speed; errors as values (`!T`, `try`); a package manager in its build system; `comptime` | determinism, rollback, reload |
| Odin | LLVM speed; `or_return`; vendor packages (raylib, GGPO bindings) | determinism; reload is a hand-made DLL swap |
| Jai | fast builds, compile-time execution | not publicly released |
| C# in Unity | Burst compiles a checked subset of C# for jobs; the job system catches some data races | float bits across machines; shaders are a second language |
| GDScript in Godot | script reload in the editor, the editor itself | speed; static checks are optional; shaders are a second language |
| GGPO (and GGRS) | proven rollback netcode for fighting games | determinism, save and load: the game's callbacks |

## 2. The gap table

Follow-up numbers are in `docs/superpowers/plans/2026-10-09-core-2a-followups.md` (engine entries 1-34 under "Found while building the voxel engine", then 35-96). Done since they were filed, and so not listed: 19 (bottom-up inliner), 47 for `float`, 51, 74, 79, the bounds-check half of "bounds checks and list-header reloads", loop-invariant code motion, flow-sensitive moves, types by full module name (072ff9f).

| Group | Gap | Number or id | Fix | Cost | Risk | Unblocks |
|---|---|---|---|---|---|---|
| Performance | Recursion and calls | fib 2.3-2.4x (`kestrel/bench/2a/results.md`) | inline a recursive function into itself one level; tail calls to loops; frameless leaf functions | M | code growth | the speed floor |
| Performance | Signed `%`/`/` by a power of two | collatz 2.4x (`bench/2a/results.md`, after 59dd0f2) | `x % 2^k == 0` as a mask; exact shift after a passed even test | S | low | integer code |
| Performance | Division by other constants | `loop` 1.2x (`% 1000000007`, `bench/2a/results.md`) | magic-number multiply ("Performance") | S | signed edge cases | hashes, noise |
| Performance | List headers reloaded, `add` through memory | `copy.kes` 0.68 against Rust 0.24-0.26 ns, 2.7x (`bench/kernels/results.md`, "Quiet reruns"); race 1.39x and 1.84x `block-mesh`, `add` 2.5x `Vec::push` (engine 27-30, 38, 39) | alias regions for headers, hoist the header of a list the loop can't change, `{ptr, len, cap}` in registers for `add` loops, an inlined `add` fast path, `clear` without zeroing (29) | M-L | a header can be another list's element: a careless split reuses a stale header | race round 2, the mesher |
| Performance | Sort | `sort_by` 2.05x (44.76 against 21.80 ns, `bench/kernels/results.md`) | a sort generated per item type with the compare inlined | M | low | |
| Performance | Enum through a stack slot | heightmap about 2x (`bench/heightmap/results.md`) | scalar replacement for enums | M | medium | |
| Performance | No auto-vectorising | `map` 1.49 against 0.84 ns, 1.8x (`bench/kernels/results.md`) | an SLP pass, or LLVM (section 3); `vec4` by hand today | L | high | |
| Performance | Xor chains not reassociated | frozen voxel world 1.13x, hand-tuned copy 0.92x (`bench/voxels3d/results.md`) | reassociate loop-invariant operands | S-M | low | noise code |
| Performance | Bytes one item at a time | `to_bytes` 7.6x a copy (70, 62) | one memcpy for plain items | S | low | saves, replays |
| Performance | `pow`, float `%`, `asin`/`acos`, `f64` math | 52, 57, 56, 49 | table `pow`; constants loaded once; wide `f64` | S each, `f64` M | license check (49) | math-heavy games |
| Performance | Compile time of range analysis | +30-43% against a 10% target (`bench/voxels3d/results.md`, "Range analysis") | cache writes per statement, sparse `Facts` | M | low | reload latency |
| Performance | Spare-block cache holds memory | 240-295 MB beside the arena (engine 10), 71 | cap or trim (the user's trade-off) | S | reuse speed | memory |
| Language | Errors as values | none built; `tests/programs/generic_result.kes` | section 4 | M | syntax | libraries, `file` |
| Language | `exit`/`fail` | 58, 83 | section 4 | S | syntax | libraries can refuse |
| Language | Placeholders on interfaces and methods | syntax spec "Not built" (Generics) | section 4 | M-L | syntax | `Rollback[I]`, 82, 96 |
| Language | `pure` written everywhere | open since 2026-10-28 | section 4 | M | syntax | friendliness |
| Language | Private methods, read-only fields | 90 (and 88) | `pub` on methods; the user's spelling | S-M | syntax | safe libraries |
| Language | Bit scans and counts | engine 32 | `trailing_zeros`, `leading_zeros`, `count_ones` | S | syntax (names) | greedy meshing |
| Language | Owning types as `T`, struct map keys, sized `list[T](n, fill)` | engine 1-3, 6, 11 | per-instance rules, moves into fields | M each | medium | the engine |
| Language | Contracts on generics, natives marked safe, facts across calls | 33, 84; syntax spec "Not built yet"; range "Facts across calls" | per-instance checking; a table of safe natives | M, M, L | low, low, high | claim 4 |
| Language | Work that spans frames, nested `par for`, work stealing | engine 4, 8 | a job library first (`await` is syntax) | M-L | medium | claim 5 |
| Library | Real networking | determinism round 3 (`2026-10-27-determinism-proposal.md`) | UDP natives, acks and resend (85), N players (82) | M | needs a second machine | claim 2 |
| Library | Thin standard library | `reduce`, `reverse`, `min_by`, comparator sort, `copy_from` (engine 12), JSON, audio, gamepads, fonts | by need, from real programs | S each | low | outside users |
| Platform and tooling | No CI, one PC | `.github/workflows/` is empty | GitHub Actions, Windows first; `.gitattributes` (87) | S | runner minutes | claim 1 |
| Platform and tooling | Windows only | roadmap step 6; raylib fails elsewhere | Linux x64 compiler and exes, window gated; macOS later (main-thread window) | M (Linux), L (macOS) | needs CI | claims 1-2, users |
| Platform and tooling | No profiler or time-travel debugger | roadmap step 8 | per `par for` and arena timing; step a replay back | M each | low | claim 3 |
| Platform and tooling | Follow-up backlog | 96 numbered plus older entries | a sweep of the cheap ones (87, 91, 92, 94, 95, engine 21, 22) | S-M | low | polish |
| Adoption | No packages | roadmap step 7 | section 4 | M | syntax | sharing code |
| Adoption | One user, no docs for outsiders, no tutorial | README and specs are design records | section 5 | M | low | everything else |

## 3. Performance in depth

**fib, 2.3-2.4x.** `bench/2a/results.md`: "Cranelift doesn't inline or transform recursion; GCC turns one of fib's two recursive calls into a loop". Checks are not the cost (checked 251 against `--release` 242 ms). A KIR pass that inlines a self-recursive function into itself one level halves the calls, and a frameless leaf trims each call. Honest expectation: about 1.4x (1.2-1.6); no experiment in the files backs a number. Reaching 1.0x likely needs LLVM's recursion transforms.

**collatz, 2.4x.** The divides are by 2 and already shifts (59dd0f2 took it from 5.2x). The bench file names the likely rest "the bias chain on signed `% 2` and the checks around `3 * n + 1`", without a dump; `--release` has no checks, so the bias chain is the suspect. Fix: `n % 2 == 0` as `n & 1 == 0` (true for negative `n` too) and `n / 2` as a plain shift where `n` is known even. Expected 1.2-1.5x; dump first (half a day). The non-power-of-two case is `loop` (`% 1000000007`, 1.2x); a magic-number multiply should bring it to about 1.0-1.1x.

**List copy and `add` loops, 2.5-2.7x.** `bench/kernels/results.md` ("The copy loop got slower") gives three causes: the header is reloaded "because Cranelift can't prove the store to the list's elements didn't change it", a check per store (since proven away in counted loops), and 8 bytes moved as two 4-byte moves where Rust uses wider moves. Engine entries 38-39 add the header reload after every `add` and pointers spilled in a six-list loop. Cranelift's memory flags carry alias regions, so headers and elements can be told apart at KIR level (the spike's `KSPIKE_HOIST` and `KSPIKE_ADDREG` switches, `.superpowers/sdd/list-spike-partial.patch`). Expected: the race's kernels from 1.39x and 1.84x to about 1.2x and 1.3x; `copy.kes` to about 1.5-2x, because Rust vectorises that loop; 1.3x on it needs vectorising or a bulk copy op.

**`sort_by`, 2.05x.** `bench/kernels/results.md`: the runtime sorts `(key, position)` pairs and then moves the items; "Rust sorts the 16-byte items directly". With generics built, the sort can be Kestrel code generated per item type with the key compare inlined. Expected 1.2-1.5x; Rust's standard sort is a tuned modern algorithm, and matching it exactly is a larger job.

**Expected ratios, in one place** (estimates from the causes above, not measurements; the LLVM column assumes KIR hands LLVM the same facts it hands Cranelift):

| Case (file) | Today | After the KIR round | With an LLVM release backend |
|---|---:|---:|---:|
| fib (`bench/2a`) | 2.3-2.4x | about 1.4x | about 1.0x |
| collatz (`bench/2a`) | 2.4x | 1.2-1.5x | about 1.0x |
| `loop`, `% 1000000007` (`bench/2a`) | 1.2x | 1.0-1.1x | about 1.0x |
| `copy.kes` (`bench/kernels`) | 2.7x | 1.5-2x | 1.0-1.3x (vectorised) |
| race faces / arrays (`bench/engine`) | 1.39x / 1.84x | about 1.2x / 1.3x | about 1.1x |
| `sort_by` (`bench/kernels`) | 2.05x | 1.2-1.5x | the same: the algorithm decides |
| `map` (`bench/kernels`) | 1.8x | 1.8x | about 1.0x (vectorised) |
| heightmap (`bench/heightmap`) | about 2x | not estimated | not estimated |

**A second backend.** The compiler design (`docs/superpowers/specs/2026-10-09-kestrel-core-rebuild-design.md`) keeps KIR free of Cranelift types so "a second backend (C output or LLVM, sub-project 5) reads the same KIR"; the syntax design lists "5 release speed (LLVM backend, precompiled runtime)"; the handoff's later queue (2026-10-11) judged "with the step at 1.15x Rust, the case for it is weaker than it was". The split is common: Rust has a Cranelift backend for fast debug builds, Jai its own x64 backend beside LLVM, and Zig is building its own backends beside LLVM. Cost of an LLVM release backend through inkwell:
- **Dependency:** an installed LLVM of one pinned major version at build time; this machine has no LLVM or libclang (handoff, 2026-10-11). Easier on Linux CI than here.
- **Build and compile time:** LLVM is a large C++ toolchain, and optimised LLVM builds are far slower than Cranelift's 0.11-0.19 s compiles (`kestrel/bench/determinism/results.md`), so `kestrel run`, hot reload and the Godot add-on stay on Cranelift; LLVM is for `kestrel build --release` only.
- **ABI and runtime:** a second lowering of everything in `kestrel/codegen` (5,389 lines), including struct returns, `par for` thunks, natives, exports, the `kestrel_rt_*` calls and the inline wide math (`codegen/src/func/wide*.rs`), which must mirror the Rust twins op for op.
- **Determinism:** target x86-64 with SSE4.1 only (as `codegen/src/isa.rs`), no fast-math flags, no contraction into FMA. The determinism spec's guarantee "the same instructions from the same KIR" in JIT and exe (decision 4) becomes "the same bits from two code generators", which needs a cross-backend test over the bit table, the 0.51-ulp sweep and Pong's replay. The GPU emitter is already a second backend with this cost.
- **Size:** L, about 6-10 days, then a second answer for every future feature.

**Verdict: not now.** Each 2-3x case has a named cause that KIR and Cranelift's own alias regions can address; after that round, fib (about 1.4x) and vectorisable loops (1.5-2x) are what LLVM would still win. Re-measure after the performance round; if those matter to a real program, spike `--backend llvm` on Linux CI (S-M) before committing.

## 4. Language gaps that need the user's syntax

**Errors as values.** Pick: a built-in `enum Result[T, E] { Ok(value: T), Err(error: E) }` (what `tests/programs/generic_result.kes` writes by hand), and a prefix `try`: `let bytes = try file.read(path)` gives the `Ok` value or returns the `Err` from the enclosing function, whose return type must be a `Result` with the same `E`; on a `T?` in a function returning an optional it returns `none`. Handling stays `match` and `let ... else`. `file` and `from_bytes` keep their optional forms; new library code returns `Result[T, string]` with a sentence. Zig uses the same word.

**`exit` and `fail`.** Pick: `exit(code: int)` flushes output and ends the program with that code (follow-up 58; Pong's replay check could then exit 1); `fail(message: string)` stops with "stopped on line N of x.kes: " and the sentence (83: `new_session` could say "`me` is 1 or 2"). Both count as never returning, like `while true` with no `break`; `fail` is a way to fail for `no_panic`.

**Placeholders on interfaces and methods.** Pick: the same square brackets as structs: `interface Rollback[I] { step(var self, one: I, two: I) }` and `xs.map_to[U](...)`. The rollback library would take the game's own input type, which removes the 12-byte `Input` cost (96) and opens N players (82).

**`pure` inferred.** Pick: the compiler infers purity for every function; a written `pure` stays as a checked promise, required only on `pub` functions another package relies on (so a library update can't silently break a user's `par for`). The error for a `par for` calling an impure function names the chain to the line, as contracts do.

**Packages.** Pick: a `kestrel.toml` beside the entry file listing dependencies by git URL and tag or by path, a `kestrel.lock` pinning each to a commit (a replay then names exact code), fetched into a cache by `kestrel fetch`; `use name.module` finds a package after the program's own folders. No registry until outside users exist.

## 5. Adoption gaps

In order, with cost: (1) **CI on GitHub Actions, Windows** (S): headless workspace tests, the determinism bit table and Pong's replay from the user's PC; the runner's CPU is the second machine claim 1 needs, at no cost to the user's PC, though it has no GPU. (2) **Linux x64** (M): compiler, exes and tests, window and Godot crates gated out first; then a Linux runner and a cross-OS replay check. (3) **Docs for outsiders** (S-M): a short language tour; today the 1,100-line syntax spec and the README are design records. (4) **"Write your first game"** (S-M): Pong from an empty file with `setup`/`frame`, reload, `--record` and `rollback`, each step a program test so it can't rot. (5) **Outside users** (the user's call): two or three people, measured by time to a running game and the errors they hit; the friends who gave feedback on 2026-10-28 are the obvious first. (6) **The Godot add-on on Linux** (M, after 2); macOS needs a Mac and the main-thread window move (L).

## 6. The pick: a new order

The user's play test of reloading and of Pong on the library runs beside all of this. Rounds 1-3 need no user syntax, so they run while the user is away; the section 4 proposal is written beside round 1 and waits for answers.

1. **CI and cross-machine determinism** (S, week 1): the cheapest real evidence for claim 1, and it guards every later round.
2. **The performance round** (M-L, week 1-2): section 3's KIR fixes, with a `bench` table of every kernel's ratio to Rust; target at most 1.3x except fib and vectorisable loops, so "near Rust" is true before anyone outside measures it.
3. **Linux x64** (M, week 2-3): the second OS for claim 1 and the first thing an outside user asks for.
4. **GPU round 1** (L, week 3-4), as proposed, with the proposal's recommended names unless the user renames: kept because the user chose it on 2026-10-27, placed where the user can say the PC is free, since a hung GPU loop can crash a running game.
5. **The language round** (M-L, week 4-5): whatever section 4 the user answered (`exit`/`fail` and `try` first, then interface placeholders, `pure` inferred).
6. **Real networking** (M, week 5-6): UDP, acknowledgements, N players on `Rollback[I]`; two machines for claim 2.
7. **Contracts that reach the frame** (M, week 6-7): safe natives, per-instance generic contracts: claim 4 on Pong and the voxel engine.
8. **Packages, the tour and the tutorial** (M, week 7-8): ready for the first outside users; then the LLVM checkpoint, the time-travel debugger and GPU round 2 by what the users and numbers show.

## 7. Open questions for the user

1. **May the controller add GitHub Actions CI to the repo** (free for public repos, a monthly quota for private ones)? It is the second machine claim 1 needs.
2. Section 4's picks: yes, or rename (`Result`/`try`, `exit`/`fail`, `Rollback[I]`, inferred `pure`, `kestrel.toml`).
3. GPU round 1: keep the proposal's recommended names, and when is the PC free for GPU work?
4. LLVM: agree to decide after the performance round, not before?
5. Outside users: who, and when may they see the repo?
