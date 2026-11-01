# Kestrel

A compiled language for games (Rust + Cranelift). The compiler is the Cargo workspace in `kestrel/`. The old `kestrelc/` and `kestrel.js` implementations were removed in October 2026; they are in git history only.

## Start here

**Read `docs/superpowers/plans/2026-10-10-handoff.md` first.** Its top "Start here" section has the current status and the next task (determinism round 1 is done, 2026-10-28, on `main`: Kestrel's own math, the float mode pinned, `@prove(deterministic)`, `Rng` and `checksum`, storage-reusing assignment for snapshots, and a rollback Pong demo in `kestrel/demos/pong/`, with results in `kestrel/bench/determinism/results.md`; generics are done, 2026-10-25; the voxel engine version 1 is built, reviewed and measured, 2026-10-26, in `kestrel/voxel/`, with its spec `docs/superpowers/specs/2026-10-25-voxel-engine-design.md` and plan `docs/superpowers/plans/2026-10-25-voxel-engine.md`; the KIR inliner works bottom-up, 2026-10-26; the fast float math round is done, 2026-10-29 (`float` math is Kestrel's own wide functions, at most 0.51 ulp and the same bits everywhere, inlined branch-free except `pow`, with loop-invariant math computed once before the loop; spec `docs/superpowers/specs/2026-10-28-fast-math-design.md`, numbers in `kestrel/bench/determinism/results.md`); the user played `voxel/main.kes` ("feels good"); the I/O round is built and reviewed, 2026-10-30 (`to_bytes`/`from_bytes[T]`/`first_difference`, the `file`, `time`, `args` and `env` modules, `kestrel run --record` and `kestrel replay`, Pong's `pong.replay`, the voxel engine's `world.save`; spec `docs/superpowers/specs/2026-10-29-io-round-design.md`); hot reload that keeps state is done, 2026-10-31 (`pub setup() -> State` and `pub frame(var s: State) -> bool` instead of `main`; `kestrel run` recompiles a saved file on the side and swaps it in between frames, keeping the state in place when its shape is unchanged; Pong and the voxel engine are written that way; the call exclusivity rule now compares places by field; spec `docs/superpowers/specs/2026-10-30-hot-reload-design.md`); the rollback library and modules in folders are done, 2026-11-01 (the built-in `rollback` module, four files merged into one: a game implements `Rollback` with `pure step(var self, one: Input, two: Input)` and drives a `Session[Game]` with `remote`, `local`, `advance`, `outgoing` and `desync` each frame, with `FakeNet`, `Replay` and `check_replay`; Pong is written on it, 498 lines in two files, with byte-identical output; `use net.packet` loads `net/packet.kes` and full names work in types; spec `docs/superpowers/specs/2026-10-31-rollback-library-design.md`); the performance round is done in code, 2026-11-01, through a2aff35 (small `if`s as selects, list headers in registers across loops, `parts[i]` and plain enum locals in registers, loop-invariant checks moved before the loop, xor chains regrouped, number sorts keyed once with a radix sort, accumulator recursion a loop in `--release`; no bit changed; 1,911 tests; spec `docs/superpowers/specs/2026-11-01-perf-round-design.md`); Kestrel builds and tests on Linux x64 without the window, and CI runs on Ubuntu and Windows (`.github/workflows/ci.yml`); the frontier plan is `docs/superpowers/plans/2026-11-01-frontier-roadmap-proposal.md`, and the five language proposals are ruled in `docs/superpowers/plans/2026-11-01-language-proposals.md`; next are the round's numbers (Task 10, when the PC is free), then the language round (`exit`/`fail`, `Result` + `try`, interface placeholders, `pure` inferred, packages), the showcase netplay demo and tutorial, and GPU round 1); the rest has the user's decisions and preferences and environment gotchas.

## Where things are

- Language design: `docs/superpowers/specs/2026-10-09-kestrel-syntax-design.md`
- Compiler design: `docs/superpowers/specs/2026-10-09-kestrel-core-rebuild-design.md`
- Plans and open follow-ups: `docs/superpowers/plans/` (`2026-10-09-core-2a-followups.md` lists known gaps)
- The pipeline is `syntax → check → kir → codegen`. The runtime is linked as a staticlib (built by `cli/build.rs`).

## Commands (run in `kestrel/`)

```bash
cargo test --workspace          # unit, program (JIT + exe) and error tests
cargo clippy --workspace --all-targets -- -D warnings
cargo run -q -p kestrel-cli -- run file.kes
```

Program tests are `kestrel/tests/programs/*.kes`, with `// expect:`, `// expect-exit:` and `// expect-stderr:` lines. Error tests are `kestrel/tests/errors/*.kes`, with `// error <line>: <message>` lines.

## Conventions

- Keep source files under about 400 lines; split by responsibility.
- Error messages are plain sentences that say what to do.
- Never call a bare `link.exe`: Git's coreutils `link.exe` shadows the MSVC linker on PATH (`find-msvc-tools` locates the real one).
