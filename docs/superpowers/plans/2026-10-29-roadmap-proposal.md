# Roadmap proposal (2026-10-29): what would make Kestrel the best

The user asked what would make Kestrel the best language for general use, and separately for games. This document maps that answer onto what is already planned, says what is missing, and proposes a new order. Syntax and names are the user's call, as always; the pick below is about order and scope. A read-only Opus review of the first draft found factual errors and inconsistencies; this is the corrected version, re-reviewed.

The answer in one line each:

- **General use:** the compiler proves what other languages hope for (`pure`, `par for`, arena escape checks, and the five `@prove` contracts `no_alloc`, `no_panic`, `no_checks`, `inline` and `deterministic`). That is the brand. What is missing is the plain long tail: files, bytes, time, errors as values, packages, Linux and macOS.
- **Games:** nothing else combines bit-exact determinism, storage-reusing snapshots, and purity-checked parallelism. Finish that into rollback netcode, replays, hot reload that keeps state and a time-travel debugger, and the headline is "the game cannot desync, cannot stutter from allocation, runs on every core, and the compiler checked all three."

## Where each suggestion stands

| Suggestion | Status | Where |
|---|---|---|
| `to_bytes`/`from_bytes`, a `file` module, replay files, `kestrel replay`, `first_difference`, a bug-report bundle | Planned: determinism round 2 | `2026-10-27-determinism-proposal.md` ("Rounds") |
| A rollback library (snapshot ring, input delay, predict and confirm, desync report) that programs import, not demo code | Not planned as a library. Pong has it as demo files: `peer.kes` (the ring, the guess and confirm), `net.kes` (the simulated network) and `replay.kes`, used by `pong.kes` | New, below |
| `par gpu for` and Kestrel shaders | Planned: GPU rounds 1 to 4 | `2026-10-27-gpu-proposal.md` |
| Hot reload that keeps state, standalone | The Godot add-on reloads the whole program when a source file changes (`refresh`, `godot/src/lib.rs`; the driver tracks each file's modification time). Standalone reload is planned in three places, none built: item 4 of the 2026-10-12 order, the voxel proposal's "`await` and hot reload by what the engine shows it needs", and determinism round 3 (state restored by field name). GPU round 3 has a watcher that regenerates shader GLSL only | Pulled earlier, below |
| `await` | Planned by need, paired with hot reload in the voxel proposal | Stays by need: the engine's findings have not asked for it yet |
| Linux and macOS | A known gap since 2026-10-10, deferred by the window spec to "the cross-platform step" with no round. The build fails on Linux and macOS with raylib C errors; the window must move to the main thread for macOS; the add-on's Linux and macOS libraries were never built. The recorded plan for sharing the repo is a README line, "Windows x64 only for now" (handoff, 2026-10-14) | Linux below; macOS later |
| Packages (a git URL or path, a lock file) | Not planned. A module is one file next to the file that uses it | Proposal later; syntax is the user's |
| Errors as values with one propagation form | Not planned. Optionals with `let ... else` are the built-in form; generics let a program write its own `enum Result[T, E]` (`tests/programs/generic_result.kes`), but there is no built-in `Result` and no way to propagate | Proposal later; syntax is the user's |
| `time`, `args`, `env` natives | Not planned | Folded into the I/O round, below |
| JSON | Not planned | By need, below |
| An HTTP client | Not planned. Determinism round 3 has UDP natives | Later |
| A REPL | Not planned. The JIT exists, so it is cheap | Later |
| Compile-time system scheduling (an ECS: a system declares what it reads and writes; the compiler schedules systems in parallel) | Not planned | Proposal only, after the voxel engine's findings justify it |
| A frame profiler that knows `par for` and arenas; debug-build per-frame allocation counts | Not planned. `--report` lists every check and allocation left, with its line and why; the engine has hand-written phase timers | New, small, below |
| Time-travel debugging (step back through snapshots) | Not planned | After replays exist, below |
| `@prove` on whole modules; `@prove(bounded)` on loops | Not planned | Later |
| `pure` inferred, written `pure` an optional checked promise | Open question from the friends' feedback (2026-10-28) | The user's call |
| Audio, gamepads, fonts | In the window spec's "not in this spec" list, never planned | By need |
| WASM | Removed with `kestrel.js` (October 2026). The GPU proposal's "later" has the web via naga and WGSL | Later |

So of the games list (round 2's files and replays, the GPU, hot reload, the rollback library, the profiler, time travel), two are planned next, hot reload is planned but late and in pieces, and three are new. Of the general list, nothing is planned except the file module.

## The pick: a new order

Today's order (handoff, 2026-10-29): the Pong play test, then the GPU round, then determinism round 2. The pick moves the I/O round, hot reload and the rollback library in front of the GPU, because those are what every program needs every day (the voxel engine's save, Pong's replay files, every demo's edit-run loop), while the GPU is the showpiece. The I/O round and the rollback library are small. Hot reload is not: it needs a program-shape decision from the user (below), so it is a medium round. Each step is one round with its own spec, plan and ledger, as before.

1. **The Pong play test** (the user). Unchanged.
2. **The I/O round** (determinism round 2, widened). Everything round 2 lists: `to_bytes`/`from_bytes`, a `file` module (`read`, `write`, `exists`, `list`, in a built-in module with natives, like `window`), replay files and `kestrel replay`, `first_difference`, the bug-report bundle, and the voxel engine's save as seed, player and edits. Plus three small built-ins every program reaches for: `time` (a monotonic clock and the wall clock, as Godot's `Time`), `args` (the command line) and `env` (a variable's value). JSON stays out: `to_bytes` is the save and replay format; a JSON module comes when a demo needs to talk to something outside Kestrel. Done when the voxel engine saves and loads its world, Pong writes a replay file that `kestrel replay` checks and `first_difference` names a planted desync, and a program test covers each of `time`, `args`, `env`, `file.exists` and `file.list`.
3. **Hot reload that keeps state, standalone.** Build on what the Godot add-on does: `kestrel run` watches the program's files (the driver already records their modification times) and recompiles the whole program on a change, as today's JIT compiles one module and cannot swap a single function. What is new is keeping state across the recompile: snapshot it with `to_bytes` before the swap, restore it with `from_bytes` into the new code's types, matching fields by name and defaulting new ones. Two things the design must settle: (a) **the program shape:** today's demos hold their state in `main`'s locals inside `while window.frame()`, which nothing can snapshot, so a reloadable program needs an entry that owns the state (say an init that returns it and a frame function that takes it `var`); that is syntax, the user's; (b) **owning values:** `to_bytes` follows `checksum`'s rule (values without owning types or function values), so GPU meshes and window handles cannot cross the reload and are rebuilt after it, and the window itself must be owned by the runtime, not the program, to stay open. The followups file (line 220) warns that `kestrel_rt_call_file` can point into freed JIT memory after a reload once window natives are involved; the Godot add-on never reaches them, but a standalone reload with the window open does, so that is closed first. Needs step 2. The GPU round 3 shader watcher shares the file watcher, not the state restore. Done when a change to Pong's paddle speed shows after one recompile with the score kept, and a change to the voxel engine's walk speed shows with the world kept (a noise change regenerates the world, as it should).
4. **The rollback library.** Promote Pong's `peer.kes`, `net.kes` and `replay.kes` into a library any program imports: a snapshot ring with storage reuse, an input buffer with delay, predict and confirm, checksum comparison every N frames, and a desync report that names the first differing field (`first_difference`). A library, not a compiler built-in, because the 2026-10-12 ruling keeps the language general and the voxel proposal's order has "libraries grown out of the engine"; until packages exist it lives in the repo (say `kestrel/lib/rollback/`), and because a module today is one file found next to the file that uses it, using it from another directory needs an import by path, which is syntax (open question 3) and otherwise part of the packages proposal (step 7); the only built-ins stay the ones that need natives or are language-level (`random`). The game state and its step go through a generic bound or an interface, not a function value, so the ring keeps `@prove(no_alloc)` and `@prove(deterministic)`. Done when `pong.kes` imports it and the library's own tests cover a planted desync.
5. **The GPU round 1** as proposed (`par gpu for` over a `gpu_list`, the particle demo to one million). Unchanged in content; moved after 2 to 4.
6. **Linux.** The compiler crates, `kestrel build` exes and their tests on Linux x64, with the window and Godot crates gated out of the build there (the raylib staticlib in `cli/build.rs` is what fails today). The window and add-on come with the macOS main-thread move, later. No Linux machine or CI exists today; one is needed, the user's call. Done when the workspace tests pass on Linux with the window, GPU and demo tests skipped. This is a suggestion for before the repo is shared; the recorded plan is the README line "Windows x64 only for now", which stays the cheaper option.
   - **Built on the `ci` branch (2026-11-01, draft PR):** Linux builds the compiler and exes without the window (the window crate is empty off Windows, window programs are refused with a sentence), the window, GPU, demo, reload and replay tests are ignored there, and `.github/workflows/ci.yml` runs clippy and the tests on ubuntu-latest and windows-latest.
7. **Two proposals, no code until the user picks syntax:** errors as values with one propagation form (a built-in `Result` or the user's own shape, and a short form to pass an error up), and packages (a git URL or a path, pinned in a lock file). Both are general-use items that need the user's syntax.
8. **The small tooling**, by need: the frame profiler (per `par for` and per arena, in `--report` or at run time), a debug-build per-frame allocation count printed for the frame function (the shape step 3 settles) when it has no `@prove(no_alloc)` contract, and the time-travel debugger once replays exist (step a replay back, inspect state).

Not now, with the reason:

- **A REPL:** cheap, but nothing today needs it; add when the user wants to try expressions.
- **Compile-time system scheduling:** syntax-heavy, and the voxel engine's findings do not yet show that hand-written `par for` phases are the bottleneck.
- **`await`:** by need, as the voxel proposal says; nothing built so far has asked for it.
- **An HTTP client, audio, gamepads, fonts, WASM:** by need, when a demo asks. The user's rule stands: no evaluation games, so these come from real programs.
- **`@prove` on modules, `@prove(bounded)`:** after the contracts that exist have been used in anger by the rollback library and the engine's save.

## Rejected alternatives

- **The GPU round first** (today's order, the user's direction of 2026-10-27). It proves the plumbing and is the biggest demo. Rejected here because the I/O round and the rollback library are small and unblock daily use of everything already built, and hot reload's state restore reuses round 2's bytes walk. Honest cost of this pick: the GPU round starts about three rounds later, and hot reload is a medium round, not a small one.
- **A big standard library push now** (JSON, HTTP, audio together). Rejected: it is not driven by a program, and the user's rule is to build what real programs need.
- **Rollback as a compiler built-in** (like `random`). Rejected: a game-only built-in pulls against the general-language ruling; a library is what "libraries grown out of the engine" meant. Cost: it needs an import by path (or copied files) until packages exist, and it is less discoverable than a built-in.
- **Hot reload for shaders only** (GPU round 3 as written). Rejected: the state-keeping reload is what the edit-run loop needs; the shader watcher is one piece of it.
- **Per-function hot swap** (replace only the changed function in the running JIT). Rejected: the JIT builds one module and has no per-function replacement; whole-program recompile with state restore gets the same effect at today's compile times (the voxel program's `check --release` is about 0.22 s, before codegen).

## Open questions for the user

1. **Order.** Keep the GPU first, as you chose on 2026-10-27, or take the I/O round, hot reload and the rollback library first, as picked here? **Ruled 2026-10-29: the I/O round first ("sure").** Not started: another chat is working on math; the user said to build nothing yet.
2. **The reloadable program shape.** Hot reload needs the state to live somewhere the runtime can snapshot: an init that returns it and a frame function that takes it, or another shape of your choosing. Syntax is yours; the round cannot start without it.
3. **Names and imports.** `file`, `time`, `args`, `env` as module names; `rollback` for the library; whether `kestrel run` reloads by default or behind a flag; and how a program imports a module from another directory (the rollback library needs it before packages exist).
4. **`pure`:** inferred, with a written `pure` as an optional checked promise, or written as today.
5. **Linux timing.** Before the repo is shared, or the README line only.
