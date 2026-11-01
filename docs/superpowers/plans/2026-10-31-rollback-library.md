# Rollback Library and Modules in Folders Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `use rollback` gives any deterministic game a two-player rollback session (snapshot ring, prediction, rollback, confirmed checksums, desync report, replay) by implementing one interface, Pong runs on it with unchanged output, and `use net.packet` loads `net/packet.kes`.

**Architecture:**
- **Part B (syntax, link, driver, LSP):** dotted names in `use` and in types; the loader resolves them relative to the using file and keys files by canonical path; each file gets a full name from its path; runtime errors name files by their path from the entry's folder.
- **Built-in modules of several files (driver):** the embedding table holds parts; the loader parses each part as its own `Source` and merges them into one module before linking.
- **The `rollback` module (Kestrel, `driver/std/rollback.kes` + `std/rollback/`):** `Input`, `Rollback`, `Packet`, constants; `Session[G: Rollback]`; `Replay[G]`, `check_replay`, `difference`; `FakeNet`.
- **Pong:** `game.kes` implements `Rollback`; `pong.kes` drives two sessions over a `FakeNet`; `peer.kes`, `net.kes`, `replay.kes` deleted.

**Tech Stack:** Rust (Cargo workspace `kestrel/`), Kestrel. Tests in `kestrel/syntax/tests`, `kestrel/driver` unit tests, `kestrel/lsp`, `kestrel/tests/programs`, `kestrel/tests/errors`, `kestrel/cli/tests`.

**Spec:** `docs/superpowers/specs/2026-10-31-rollback-library-design.md` (read it whole; decisions 1-17).

**Ledger:** `.superpowers/sdd/2026-10-31-rollback-library/progress.md` (git-ignored), one line per task, plus briefs, reports and reviews.

## Global Constraints

- **Builds and tests:** idle priority; `CARGO_BUILD_JOBS=8`, `--test-threads=8` when the PC is free; **none at all while the user is gaming** (the controller says which). Per task: the touched crates plus `--test programs --test errors` JIT-only; the exe pass and the full suite once, in Task 5.
- **Never a visible window:** `KESTREL_HEADLESS=<frames>` with `KESTREL_HEADLESS_QUIET=1`, or `KESTREL_HIDDEN=1`.
- **Never call a bare `link.exe`.** On LNK1104 use a short `CARGO_TARGET_DIR` such as `C:/kt/rb`.
- **Files under about 400 lines;** split by responsibility. Messages and comments are plain sentences that say what to do.
- **No new syntax beyond dotted `use` names and dotted type names; no new keywords, built-ins or Cargo dependencies. Nothing in the hot reload host (`cli/src/reload/`) changes.**
- **Every existing test keeps its output** except the ones the spec names (Pong's replay offsets, the built-in module lists).
- **Never stage** `kestrel/test.kes`, `voxels.exe`, `kestrel/b.exe` or `kestrel/check/src/body/enums.rs`.
- **One commit per task on `main`**, a sentence saying what changed and why, ending with the Co-Authored-By line the session gives; the controller pushes.

## Review Focus

1. One file reached by two dotted names (`use net.wire` from the root, `use wire` from `net/packet.kes`) is loaded once and linked once; a type made through one name is accepted where the other names it (Task 1).
2. Every existing module test and error test passes with its message text unchanged; flat programs keep their linked names and runtime file names (Task 1).
3. With `delay: 0`, `Session` does exactly what `peer.kes` does, in the same order, so Pong's output is byte for byte the same; the `DESYNC_AT` nudge moves after the batch of confirmed frames, which is equal only because one `advance` never confirms 30 frames at once (Tasks 2 and 4).
4. `advance` copies no state for a plain `G` (checksums in place, storage-reusing ring writes) and a game's one-line wrapper proves `no_alloc` (Tasks 2 and 4).
5. A packet made in a frame's arena can be sent to a `FakeNet` and given to `remote` without an escape error or a dangling pointer (`KESTREL_POISON=1`) (Task 3).

---

### Task 1: Modules in folders (Part B)

**Files:**
- Modify: `kestrel/syntax/src/parser/item.rs` (`use_decl`: a module name is `ident (. ident)*`, joined into `UseClause.module`, `module_span` covering it; `type_expr`: `ident (. ident)*` then optional `[...]` as one `TypeKind::Named` whose name has dots)
- Modify: `kestrel/syntax/src/link.rs` and `link/walk.rs` (`Module.name` becomes the full name, beside `file` and `resolved`; `qualify` uses the full name; `import` binds the last name unless two whole-module uses share it (then a `Scoped::Ambiguous(a, b)` that errors on use with "`packet` could be `net.packet` or `ui.packet`; write the full name, like `net.packet.Packet`"); the first name of a dotted use is taken in the file; the module-and-item clash across the whole program, at the item; expression chains `a.b.f(...)` and type names `a.b.T` resolve their prefix as a module, short or full; the unknown-prefix message of spec decision 15; "which `net.packet` can't see"; dotted names in interface lists and placeholder rules too). `link` takes each clause's resolved module from the loader (`Module.resolved: Vec<usize>`, in clause order) and stops looking modules up by written name; the missing-module message moves out of `link::import`. If `link.rs` passes 400 lines, move the import rules to `link/imports.rs`; put the module-prefix resolution of expression chains and dotted types in `link/chains.rs` (`walk.rs` is 305 lines).
- Create: `kestrel/driver/src/load.rs` (the loader moved out of `lib.rs`: resolution per spec decision 12 relative to the using file's folder, every candidate (`D/a/b.kes`, `D/a/b/b.kes`) read through `read` so hot reload watches both, files keyed by canonical path with a lexical fallback for paths not on disk, the full name per decision 13, the resolved module per clause, the four missing/ambiguous messages, the "never above" help, the built-in folder-name refusal)
- Modify: `kestrel/driver/src/lib.rs` (call `load`; `Source` gains `module: String` and `Source.path` stays the joined path, never `\\?\`; `module.files` becomes each path relative to the entry's folder with `/`, built-ins relative to `<builtin>/`, computed from the joined path; the KIR module gains `module_names` beside `files`), `kestrel/driver/src/builtin.rs` (the folder-name message), `kestrel/driver/src/analysis.rs` (the `uses` comment: per module, not per source)
- Modify: `kestrel/kir/src/range/report_copies.rs` (`plain_name` takes each file's module full name from `module_names`, not `files[..].strip_suffix(".kes")`), `kestrel/kir/src/ir.rs` (`Module.module_names`)
- Modify: `kestrel/lsp/src/resolve.rs` (`in_uses` matches by `Source.module`; definition on dotted types), `kestrel/lsp/src/nav.rs` (`builtin_file` keeps the subfolder; `is_builtin_copy` accepts the folder and below)
- Modify: `docs/superpowers/specs/2026-10-09-kestrel-syntax-design.md` ("Imports and visibility": folders, full names, the messages; drop "`geom.Point` doesn't work in a type position yet"; generics "Not built": drop "`geom.Stack[int]` in a type position")
- Test: `kestrel/cli/tests/module_folders.rs` (every Part B case in the spec's Tests; a helper `project` that creates parent folders), `kestrel/cli/tests/reload.rs` (one case: editing `net/packet.kes` of a reloadable program prints `reloaded`), `kestrel/syntax` parser tests (dotted `use`, dotted types), `kestrel/lsp` unit tests

- [ ] **Step 1: Failing tests** (plus a `check --report` case: a folder module's function shows its plain name, and a contract chain names `net/packet.kes:N`). **Step 2: See them fail.** **Step 3: Implement** (the canonical key falls back to lexical normalisation and `Source.path` stays the joined path, or the driver's `"main.kes"` tests and the LSP's `analyze("dir/main.kes", ..)` test break). **Step 4: Green** (`cargo test -p kestrel-syntax -p kestrel-driver -p kestrel-kir -p kestrel-lsp`, `--test report`, `cargo test -p kestrel-cli --test module_folders --test modules --test modules_generics --test small_modules --test programs --test errors --test reload`), clippy, **commit.**

**Done when:** every module test and error test that existed passes unchanged, the new folder tests pass, and `use net.packet` works in `kestrel run`, `build` and the LSP.

**Review Focus:** Review Focus 1 and 2; the "could be" error fires for `net.kes` beside `net/net.kes`; a short-name clash errors only where the short name is used.

### Task 2: Built-in modules of several files, and the `Session` core

**Files:**
- Modify: `kestrel/driver/src/builtin.rs` (`MODULES: [(&str, &[(&str, &str)]); 10]`: name and parts; every existing module one part; `rollback` four parts; `find` gives the parts; the unit test lists `rollback`), `kestrel/driver/src/load.rs` (one `Source` per part, `builtin: true`, `module: "rollback"`; the parts' programs merged into one `Module`: items, `proves`, `hex_literals` concatenated, identical `use` clauses once)
- Create: `kestrel/driver/std/rollback.kes` (the module's comment: what a game writes, the five calls a frame, `me` and `delay` rules; `Input`, `Rollback`, `Packet`, the constants of decision 4, `new_session`), `kestrel/driver/std/rollback/session.kes` (`Session[G: Rollback]` with the fields of decision 6 and `local`, `remote`, `catch_up`, `advance`, `outgoing`, `confirmed`, `run_again`, the checksum recording and comparison; `desync`; `record` waits for Task 3, so leave it out), `std/rollback/replay.kes` and `std/rollback/fakenet.kes` as one-line comment placeholders filled in Task 3
- Modify: `kestrel/cli/tests/modules.rs` (457 lines: only `the_built_in_modules_check` using `rollback` too and one `rollback.kes`-beside-a-user line; anything more in `module_folders.rs`)
- Test: `kestrel/tests/programs/rollback_basic.kes`, `rollback_delay.kes`, `rollback_deep.kes`, `rollback_wait.kes`, `rollback_list_state.kes` (packets carried by hand in the program: a `list[Packet]` per direction, delivered after a set number of frames; the tiny `Tally` game), `kestrel/tests/errors/rollback_not_impl.kes`, `rollback_step_not_pure.kes`, `rollback_arena_state.kes`, `rollback_interface_state.kes` (the module-prefixed texts in the spec's Tests; a CLI test for the last if the error-test format can't name a built-in file), a new `kestrel/cli/tests/contracts_rollback.rs` (`contracts.rs` is at 400 lines: the `advance_peer` wrapper proves `deterministic, no_alloc` for `Tally`; fails `no_alloc` naming the ring assignment for a list-holding game)

**Interfaces:**
- Produces: the `rollback` module's Part A API minus `FakeNet`, `Replay`, `check_replay`, `difference`, `record`.

- [ ] **Step 1: Failing tests** (expected lines worked out by hand for `Tally`, whose `step` adds inputs to totals, so a wrong guess and its correction show in the totals). **Step 2: See them fail.** **Step 3: Implement** (port `peer.kes` method by method: `save`/`restore` inline, `predict`, `inputs` in player order, `rollback` as `catch_up`, the fields `local_in`/`remote_in` since a field can't share a method's name, `Input`s compared field by field; then add `delay`, the cleared next slot, `waits`, the sender check; equivalence with `peer.kes` is for `delay: 0`, which Pong passes). **Step 4: Green** (`cargo test -p kestrel-driver`, `cargo test -p kestrel-cli --test programs --test errors --test contracts_rollback --test modules`), clippy, **commit.**

**Done when:** the program tests print their expected lines in the JIT, a hand-carried session pair at `delay: 0` makes the same calls in the same order as `peer.kes` (checked by reading both side by side in the review), and the contracts test passes.

**Review Focus:** Review Focus 3 and 4; `remote` never rolls back; `outgoing`'s frame is `now - 1 + delay`; a frame with no `local` call is `Input()`, not a 64-frame-old input.

### Task 3: `FakeNet`, desync, `Replay` and `check_replay`

**Files:**
- Modify: `kestrel/driver/std/rollback/fakenet.kes` (`FakeNet`, `new_fake_net`, `send` copying field by field, `deliver`; Pong's `Net` generalised with `latency`, `jitter`, `loss`), `std/rollback/replay.kes` (`Replay[G: Rollback]`, `frames`, `new_replay`, `check_replay`, `difference`), `std/rollback/session.kes` (`record`)
- Test: `kestrel/tests/programs/rollback_lost.kes`, `rollback_desync.kes`, `rollback_replay.kes`, `kestrel/cli/tests/snapshot_memory.rs` (a list-holding session's arena bytes equal after 2 and 200 laps), `kestrel/cli/tests/poison.rs` (a packet made in an arena, sent through `FakeNet` and given to `remote`, under `KESTREL_POISON=1`)

**Interfaces:**
- Consumes: Task 2. Produces: the whole Part A API.

- [ ] **Step 1: Failing tests** (`rollback_lost.kes`: 30% loss and a hand-dropped run of 5; `rollback_desync.kes`: peer 2's `Tally` starts one off, `desync()` gives `desync at frame 30: my checksum <a>, theirs <b>` once and `none` after, `difference` prints the field; `rollback_replay.kes`: record 300 frames, round trip through `to_bytes`/`from_bytes[Replay[Tally]]`, `check_replay` gives -1, one changed input gives 60 or the next checksum frame). **Step 2: See them fail. Step 3: Implement. Step 4: Green** (`--test programs --test snapshot_memory --test poison`), clippy, **commit.**

**Done when:** every Part A program test passes, `FakeNet` with Pong's seed and parameters draws the same random numbers in the same order as `net.kes` (a test sends the same packets through both and compares `lost` and each delivery frame), and the poison test is clean.

**Review Focus:** Review Focus 5; `record` ends the replay when a frame left the ring instead of recording a wrong state; `difference` compares confirmed frames only.

### Task 4: Pong on the library

**Files:**
- Before any change: run today's Pong headless for 600 frames (and with `DESYNC_AT = 300`) and save both outputs as `kestrel/cli/tests/pong_expected.txt` and `pong_expected_desync.txt`.
- Modify: `kestrel/demos/pong/game.kes` (`Game: Rollback`, the `step` method with `@prove(deterministic, no_alloc)`, `UP`/`DOWN`, `bot` giving an `Input`; `Inputs` and `same` removed), `kestrel/demos/pong/pong.kes` (spec decision 10: `PongState` with two `Session[Game]`, `FakeNet`, `Replay[Game]`, `nudged`; `DESYNC_AT` moves here; `let RECORD = true` guarding `record`; the comment by the nudge saying why after-the-batch equals `peer.confirm`'s per-frame nudge; `advance_peer`; `check_file` with `from_bytes[Replay[Game]]` and `check_replay`; the header comment's file list)
- Delete: `kestrel/demos/pong/peer.kes`, `net.kes`, `replay.kes`
- Modify: `kestrel/cli/tests/pong_demo.rs` (`FILES` is two; the headless output equals `pong_expected.txt` byte for byte, the desync run equals `pong_expected_desync.txt`; `INPUTS_AT = 13 + 40 + 4`; the inputs' length at bytes `53..57` is `2 * f`; the size is `INPUTS_AT + 24 * f + 4 + 8 * (f / 60)`; at `INPUTS_AT + 24 * 300` the check is `bytes[at] <= 3` and bit 0 (`UP`) is flipped, giving `replay differs at frame 360`; the memory test sets `RECORD = false` in both runs; the desync test finds `let DESYNC_AT = -1` once across `pong.kes` and `game.kes`; the other tests unchanged), `kestrel/cli/tests/demos.rs` if it lists Pong's files
- Test: as above, plus the reload test `an_edit_reloads_pong_and_keeps_the_game` unchanged

**Interfaces:**
- Consumes: Tasks 2 and 3.

- [ ] **Step 1: Capture the reference outputs and update the tests** (they fail on the new offsets only). **Step 2: Rewrite Pong.** **Step 3: Green** (`--test pong_demo --test demos --test determinism --test replay --test reload`), the three builds agree, clippy, **commit.**

**Done when:** Pong's 600-frame headless output and its desync output are byte for byte the reference, `kestrel check demos/pong/pong.kes` prints nothing, its replay file checks, and the reload test passes.

**Review Focus:** Review Focus 3; the order of calls in a network frame matches today's (receive, catch up, bots, advance, record, send, compare); the nudge lands in the confirmed snapshot after its checksum, as `peer.confirm` did, and moving it after the batch is equal only because one `advance` never confirms 30 frames at once.

### Task 5: Measurements, docs, full suite (the controller)

**Files:**
- Create: `kestrel/bench/rollback/crowd.kes` (the heavier game: 1,000 bodies), `kestrel/bench/rollback/advance.kes` (times `advance` with no rollback and with an 8-frame rollback every frame, for Pong's `Game` and the crowd)
- Modify: `kestrel/bench/determinism/results.md` ("Rollback library": the spec's measurements), `docs/superpowers/specs/2026-10-09-kestrel-syntax-design.md` ("Built-in modules": `rollback` and its API in the `random` paragraph's style), `README.md` (a short "Rollback" paragraph; desync shown as `let why = s.p1.desync() else { ... }` in a helper, never `if let`; a sentence that the Godot add-on refuses built-in modules, `rollback` included), `docs/superpowers/plans/2026-10-09-core-2a-followups.md` (76's deferral sentence; new: N players, an `exit`/`fail` built-in for library argument checks, `no_alloc` per instance on generic library code, acknowledgements and resending), `docs/superpowers/plans/2026-10-10-handoff.md`, `CLAUDE.md`
- Test: `cargo test --workspace --no-fail-fast`, `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 1: Measurements (PC free). Step 2: Docs. Step 3: Full suite and clippy. Step 4: Commit and push.**

**Done when:** the numbers are in the results file with median, range and load, Pong's network frame is within 5% of the pre-change one (else a follow-up says why), and the full suite and clippy are green.

**Review Focus:** the numbers are release builds on a quiet PC; the docs name every rename candidate from the spec's Names table.
