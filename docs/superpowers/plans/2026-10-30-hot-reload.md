# Hot Reload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `kestrel run` on a program written as `pub setup() -> State` and `pub frame(var s: State) -> bool` recompiles on every save and keeps the running state when its shape did not change.

**Architecture:**
- **Checker and driver:** the reloadable shape is recognised in the driver after checking (`setup`/`frame` present, no `main`); a synthesized `main` (`let s = setup()` then `while frame(var s) {}`) is lowered for everything but the reloading host. The `State` type is judged by a reload-specific snapshot rule (GPU allowed).
- **KIR and codegen:** the module carries `ReloadInfo { state: String (the description text), setup_hash: u64 }`; the description gains `gpu`; each reloadable module gets a generated `k_reload_drop` export.
- **Runtime and window:** `kestrel_rt_start` gains a form that runs a host closure on the one game thread; `window.open` while open applies size and title.
- **CLI host:** `cli/src/reload/` runs `setup`/`frame` through export thunks on the game thread, polls file times, compiles on a helper thread, swaps between frames, adopts or resets the state.

**Tech Stack:** Rust (Cargo workspace `kestrel/`), Cranelift 0.137 (`cranelift-jit`, `JITModule` is `Send`). Tests in `kestrel/check/tests`, `kestrel/driver/tests`, `kestrel/tests/programs`, `kestrel/tests/errors`, `kestrel/cli/tests`.

**Spec:** `docs/superpowers/specs/2026-10-30-hot-reload-design.md` (read it whole; decisions 1-9).

**Ledger:** `.superpowers/sdd/2026-10-30-hot-reload/progress.md` (git-ignored), one line per task, plus briefs, reports and reviews.

## Global Constraints

- **Builds and tests:** idle priority; `CARGO_BUILD_JOBS=8`, `--test-threads=8` when the PC is free; **none at all while the user is gaming** (the controller says which).
- **Never a visible window:** `KESTREL_HEADLESS=<frames>` with `KESTREL_HEADLESS_QUIET=1`, or `KESTREL_HIDDEN=1`.
- **Never call a bare `link.exe`.** On LNK1104 use a short `CARGO_TARGET_DIR` such as `C:/kt/hr`.
- **Files under about 400 lines;** split by responsibility. Messages and comments are plain sentences that say what to do.
- **No new syntax, keywords, flags beyond `--no-reload`, or Cargo dependencies.**
- **The JIT path only reloads;** `kestrel build` exes run the synthesized `main`.
- **Never stage** `kestrel/test.kes`, `voxels.exe` or `kestrel/check/src/body/enums.rs`.
- **One commit per task on `main`**, a sentence saying what changed and why, ending with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`; the controller pushes.

## Review Focus

1. A save that lands while a compile is running must not be lost: the next poll must see it and compile again (Task 4).
2. A compile error after a successful reload must report against the *new* file contents and keep the *latest good* module running, not the first (Task 4).
3. A `State` whose shape is the same but whose `setup` changed must keep the state and print the hint, never silently reset (Task 4).
4. `window.open` called twice by an ordinary `main` program must resize, not fail, and headless must behave the same (Task 3).
5. A reloadable program under `kestrel build` and under `kestrel check --report` must behave exactly like its `main` twin: same output, same proofs, same report lines (Task 1).

---

### Task 1: The reloadable shape: checker rule, synthesized `main`, `ReloadInfo`

**Files:**
- Modify: `kestrel/check/src/collect.rs` (the `main` rule near line 117: a program with `setup` and `frame` and no `main` is accepted; `main` plus `setup` refused: "a program has `main` or `setup` and `frame`, not both"; `setup` without `frame` or the reverse: "a reloadable program needs both `pub setup() -> State` and `pub frame(var s: State) -> bool`"; both must be `pub` free functions of the entry file, `setup` with no parameters returning a struct, `frame` with one `var` parameter of that type returning `bool`: "`frame` takes the state `setup` makes: `var s: Game`, not `var s: World`"), `kestrel/check/src/snapshot.rs` (`reload_fault(t, ty) -> Option<Fault>`: the snapshot walk with `Gpu` allowed; message "`setup`'s state can't hold a function value; keep functions out of the state" / "... an `Arena`; keep arenas out of the state"), `kestrel/check/src/lib.rs` (`Tables.reload: Option<ReloadShape { setup: FnIdx, frame: FnIdx, state: Ty }>`)
- Modify: `kestrel/driver/src/lib.rs` (after checking, when `reload` is set and the caller is not the reloading host: synthesize `main` in the AST or in KIR: pick KIR: a `main` function whose body is `Let s = Call(setup)` then `While(Call(frame, [Ref s])) {}`; the `exported` flag on `setup` and `frame` already holds since they are `pub` entry-file functions), `kestrel/kir/src/ir.rs` (`Module.reload: Option<ReloadInfo { state_description: String, setup_hash: u64, state_size: u32, state_align: u32 }>`), `kestrel/kir/src/lower/mod.rs` (fill it), `kestrel/kir/src/lower/bytes_fingerprint.rs` (`describe` writes `gpu` for `KTy::Gpu`; `pub fn description(ty, t) -> String` made public to the crate's module; the `unreachable!` stays for `Arena`, `Fn`, `Iface`), `kestrel/kir/src/lower/helpers.rs` or a new `kestrel/kir/src/lower/reload.rs` (`k_reload_drop(s: var State)`: the drop statements for the state type, as an exported function `__kestrel_reload_drop` with `exported: true` and export name `reload_drop`)
- Test: `kestrel/tests/errors/reload_*.kes` (each message above), `kestrel/tests/programs/reload_shape.kes` (a `setup`/`frame` counter program printing 1..3 then ending; the same output as `reload_shape_main.kes` written with `main`), `kestrel/check/tests/reload.rs` (`reload_fault` accepts `Mesh?`, refuses `fn`), `kestrel/kir/tests/reload.rs` (`ReloadInfo` filled, description contains `gpu` for a `Texture` field, `setup_hash` changes when `setup`'s body changes and not when `frame`'s does)

**Interfaces:**
- Produces: `kestrel_check::ReloadShape`, `kestrel_kir::ReloadInfo`, the export names `setup`, `frame`, `reload_drop` on `JitProgram::exports()`, the synthesized `main` for non-host paths.

- [ ] **Step 1: Failing tests** (the error files, the twin program tests, the unit tests). **Step 2: See them fail. Step 3: Implement. Step 4: Green** (`cargo test -p kestrel-check -p kestrel-kir -p kestrel-driver`, `cargo test -p kestrel-cli --test programs --test errors --test report`), clippy, **commit.**

### Task 2: The runtime host entry and the compile-thread helper

**Files:**
- Modify: `kestrel/runtime/src/start.rs` (`pub fn start_host(host: impl FnOnce() -> i32 + Send) -> i32`: the same big-stack thread as `kestrel_rt_start`, running `run_entry`'s duties around the closure: `fpu::pin`, `mark_started`, then the closure, then `print::flush` and the arena reports; the closure returns the exit code), `kestrel/runtime/src/panic.rs` (`pub fn reset_call_site()` sets `call_line` 0 and `call_file` null)
- Modify: `kestrel/driver/src/lib.rs` (`pub fn compile_for_reload(path, style, release) -> Result<Compiled, Diagnostics>` doing what `run` does up to `prove`, usable from any thread; `Compiled.files` with times read **before** parsing starts)
- Test: `kestrel/runtime/src/start_test.rs` (`start_host` runs the closure on a thread with the FPU pinned and returns its code), `kestrel/driver/tests/reload.rs` (`compile_for_reload` of a changed file returns new `SourceFile` times taken before the compile)

**Interfaces:**
- Produces: `kestrel_runtime::start::start_host`, `kestrel_runtime::panic::reset_call_site`, `kestrel_driver::compile_for_reload`.

- [ ] **Step 1: Failing tests. Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-runtime -p kestrel-driver`), clippy, **commit.**

### Task 3: `window.open` while open applies size and title

**Files:**
- Modify: `kestrel/window/src/state.rs:211-239` (`open` in `Open`: real backend `SetWindowSize` and `SetWindowTitle`, headless records the new size for `window.size()`, replay unchanged; `Closed` still fails as today), `kestrel/window/src/real.rs`, `kestrel/window/src/headless.rs`
- Test: `kestrel/tests/programs/window_open_twice.kes` (headless: `open(320, 200, "a")`, `open(640, 480, "b")`, print `window.size()` giving `640 480`), `kestrel/window/src/state.rs` unit test for the `Open` arm

- [ ] **Step 1: Failing test. Step 2: See it fail. Step 3: Implement. Step 4: Green** (`-p kestrel-window`, `cargo test -p kestrel-cli --test programs -- window_open`), clippy, **commit.**

### Task 4: The reloading host in the CLI

**Files:**
- Create: `kestrel/cli/src/reload/mod.rs` (`pub fn run(args, compiled: Compiled, opts, natives) -> i32`: on the game thread via `start_host`: build the first `JitProgram`, `run_init`, call `setup` through its thunk into a 16-aligned state buffer of `ReloadInfo.state_size`, then loop: poll (Task 4's `watch`), `frame` through its thunk on the buffer, `print::flush`, break on false), `kestrel/cli/src/reload/watch.rs` (`Watcher { files: Vec<SourceFile>, last_poll: Instant }`, `fn changed(&mut self) -> bool` at most four times a second), `kestrel/cli/src/reload/compile.rs` (`spawn_compile(paths, style, release, tx)`: a 256 MiB thread with `fpu::pin`, `compile_for_reload`, `prove`, `JitProgram::compile_with`, sends `Result<(JitProgram, Compiled, Instant started), String diagnostics>`; one in flight at a time, one queued), `kestrel/cli/src/reload/swap.rs` (`fn swap(old: &mut Live, new: JitProgram, ..)`: `reset_call_site`, `new.run_init()`; same `state_description` → keep the buffer, print `reloaded <file> in <t> s` and, when `setup_hash` differs, `setup() changed but the state was kept; restart to apply it`; different → call old `reload_drop` on the buffer, call new `setup` into the buffer (new size: reallocate the buffer first), print `state reset: <State> changed shape; setup() ran again`; on a compile error print the diagnostics then `still running the previous code; fix the error and save again` and refresh the watcher's times), `kestrel/cli/src/main.rs` (`--no-reload`; `run` of a reloadable program goes to `reload::run` unless `--no-reload`, `--record` (prints `--record runs the program without reloading`) or `replay`; with `--no-reload` the host still runs `setup`/`frame` on one thread without the watcher, so the two paths share code)
- Test: `kestrel/cli/tests/reload.rs` (every case in the spec's "Tests": the harness copies a program to a scratch dir, spawns `kestrel run` headless with a large frame count, reads stdout lines as they come (`BufReader` on the child's stdout; the program prints `frame N value V` each frame), edits the file, waits for `reloaded`/`state reset`/`still running` on stderr, asserts the next lines; kills the child at the end)

**Interfaces:**
- Consumes: Tasks 1-3.

- [ ] **Step 1: Failing tests** (`a_constant_edit_keeps_the_counter`, `a_shape_change_resets_with_a_sentence`, `a_setup_only_edit_keeps_state_and_hints`, `a_compile_error_keeps_the_old_code_then_a_fix_reloads`, `two_saves_give_two_reloads`, `a_save_during_a_compile_is_not_lost` (Review Focus 1: a sleep in the program's `frame` makes compiles slow enough; or a large program), `an_error_after_a_reload_reports_the_new_file_and_keeps_the_latest_good_code` (Review Focus 2), `a_main_program_never_reloads`, `no_reload_and_record_never_reload`, `a_literal_string_in_state_survives`, `a_gpu_object_in_state_survives` (headless `draw.load_texture` of a test asset, its size printed before and after; headless returns a fixed size, so assert the handle id is unchanged by printing `texture.id` if it is readable, else the size)). **Step 2: See them fail. Step 3: Implement. Step 4: Green** (`cargo test -p kestrel-cli --test reload --test programs --test replay --test small_modules`), clippy, **commit.**

### Task 5: The demos become reloadable

**Files:**
- Modify: `kestrel/demos/pong/pong.kes` (`PongState` with `p1`, `p2`, `net`, `replay`, the scalars and the check-mode flag; `setup` does what `main` did before the loop, including the replay-check mode which sets `done = true`; `frame` is one loop body; the end-of-run prints and the replay write happen when `frame` is about to return false), `kestrel/voxel/main.kes` (`VoxelState { w, s, r, p, spawned, captured, last_mouse, press_at, drag, frames, work_sum, interval_start, fps, work_ms }`; `setup` and `frame` likewise), `kestrel/cli/tests/pong_demo.rs` and `voxel_engine.rs` (unchanged expectations; plus one reload test each: a constant edit mid-run (`PADDLE_SPEED` or the walk speed) prints `reloaded` and the score / player position continues from where it was)
- Test: as above

**Interfaces:**
- Consumes: Task 4.

- [ ] **Step 1: Failing reload tests. Step 2: See them fail. Step 3: Convert both demos. Step 4: Green** (`--test pong_demo --test voxel_engine --test replay --test determinism`), clippy, **commit.**

### Task 6: Docs, measurements, full suite

**Files:**
- Modify: `docs/superpowers/specs/2026-10-09-kestrel-syntax-design.md` ("Reloadable programs" under Declarations: the two functions, the state rule, what `kestrel run` does, the three messages), `README.md` ("A window": the shape and the reload sentence), `kestrel/bench/determinism/results.md` (reload latency for Pong and the voxel engine, median of five; the poll's per-frame cost; RSS after ten reloads), `docs/superpowers/plans/2026-10-10-handoff.md`, `CLAUDE.md`
- Test: `cargo test --workspace --no-fail-fast`, `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 1: Docs. Step 2: Measurements (PC free). Step 3: Full suite and clippy. Step 4: Commit and push.**
