# Language Round Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `exit(code)` and `fail("sentence")`; a built-in `Result[T, E]` with `try` and `let x = r else why { }`, which `file` and `from_bytes` give; interfaces with placeholders (`interface Rollback[I]`) and `==` on plain structs, so the rollback library takes a game's own input and Pong goes back to its 2-byte `Pad`; `pure` inferred, with a written `pure` a checked promise. Packages are designed (spec decisions 30-33) and built in their own round.

**Architecture:**
- **A, `exit` and `fail`** (tasks 1-2): two `never` built-ins in `check`; `kir`'s `Stmt::Fail` widened to take a message expression and a line, and a new `Exit`, both ending a block; runtime calls through `panic::fail` and a new `kestrel_rt_exit`; replay format 2 with an exit entry; the hosts pass the code on.
- **B, `Result`** (tasks 3-6): a Kestrel prelude (`driver/std/prelude.kes`) every file sees; the `Ok` conversion beside `T` to `T?`; `try` a prefix operator lowered as a tag test and a return; `else why` on `let`; `from_bytes` reasons from the bytes reader; `file.kes` wrapping private natives.
- **C, interface placeholders** (tasks 7-10): `==` on plain structs and enums; interface instances as `IfaceId`s made like struct instances; rules with types and one inference step; the rollback library on `Rollback[I]`.
- **D, inferred `pure`** (task 11): one pass over the call graph after bodies are checked, discharging recorded purity obligations with chain messages.

**Tech Stack:** Rust (Cargo workspace `kestrel/`), Kestrel. Tests in `kestrel/{syntax,check,kir,runtime,driver,lsp}` unit tests, `kestrel/tests/programs`, `kestrel/tests/errors`, `kestrel/cli/tests`, `kestrel/voxel/tests`.

**Spec:** `docs/superpowers/specs/2026-11-01-language-round-design.md` (read it whole; decisions 1-33, the grammar, the table of how the features meet the rest). Each task names the decisions it builds.

**Ledger:** `.superpowers/sdd/2026-11-01-language-round/progress.md` (git-ignored), one line per task, plus briefs, reports and reviews.

## Global Constraints

- **Load.** While the user is at the PC: one cargo command at a time across all worktrees, `CARGO_BUILD_JOBS=4`, `--test-threads=4`, idle priority, no exe passes, no benchmarks, no parallel task builds (parallel tasks below then run one after another). When the controller says the PC is free: `CARGO_BUILD_JOBS=8`, `--test-threads=8`, and the parallel groups may build at once, each worktree with its own short `CARGO_TARGET_DIR` (`C:/kt/lr<N>`). While the user is gaming: no builds at all, reading and writing only.
- **Tests per task:** the touched crates' unit tests, `cargo test -p kestrel-cli --test programs -- programs_run_via_jit` (JIT only), `--test errors`, and the task's own CLI test files. The exe pass and `cargo test --workspace` run once, in task 12. After a rebase onto `main`, rerun only if `main` moved (`git rev-parse main` before and after).
- **Never a visible window:** `KESTREL_HEADLESS=<frames>` with `KESTREL_HEADLESS_QUIET=1`, or `KESTREL_HIDDEN=1`.
- **Never call a bare `link.exe`.** On LNK1104 use a short `CARGO_TARGET_DIR`.
- **Files under about 400 lines;** split by responsibility (new files are named in each task). Messages are the spec's sentences, word for word; comments are plain sentences.
- **No new Cargo dependencies, no syntax beyond the spec's grammar section.** `try` is the only new keyword.
- **Every existing test keeps its output** except where the spec names the change (decisions 6, 13, 14, 21, 29) and the task lists the file.
- **Never stage** `kestrel/test.kes`, `voxels.exe`, `kestrel/b.exe` or `kestrel/check/src/body/enums.rs` (it holds an uncommitted change of the user's). A task that must edit `enums.rs` stops and asks the controller.
- **One commit per task on `main`**, a sentence saying what changed and why, ending with the Co-Authored-By line the session gives; the controller pushes.
- **Reviews:** one round review at the end (task 12). Per-task reviews only for the risky tasks, marked **(review)**: 4, 9 and 11.

## Review Focus

1. A `try` returns exactly what a hand-written `match` with a `return` does: it leaves `arena { }` and `in` blocks, drops what `return` drops, moves the error without a copy, and its escape check fires on an error made inside the arena it leaves (task 4).
2. `fail` goes through `panic::fail`, so recordings, the Godot hook and `par for`'s lowest-index rule see it as any runtime error; `exit` never ends the Godot editor and always finishes a recording (tasks 1-2).
3. An interface instance is one `IfaceId` per declaration and types everywhere (conformance, `implements`, method tables, copies), and a type that lists `Rollback[Pad]` is accepted where any spelling of that instance is expected (tasks 8-9).
4. Inferring `pure` changes no program's output, makes no error appear in a program that had none (other than decision 27's unused results, which task 11 fixes by binding the value and lists: `owning_temporaries.kes:60` and any the suite finds), never reports a `fail(...)` statement as an unused result, and every purity error names a chain whose last link is a real effect (task 11).
5. Pong's headless and desync outputs stay byte for byte through tasks 2, 6 and 10.

## Order and parallel groups

Build order: 1, 2 (A); 3, 4, 5, 6 (B); 7, 8, 9, 10 (C); 11 (D); 12. The builder check (`.superpowers/sdd/2026-11-01-frontier/language-spec-builder-check.md`) found the shared files behind the dependencies below. Tasks whose files are disjoint may run at the same time when the PC is free:

- **Group 1:** tasks 1, 3 and 7 start together (task 7's `kir/src/lower/mod.rs` edit is one line beside task 1's; rebase). Tasks 3 and 7 both touch `check/src/generic/rules.rs` (3 lets `Result` take an optional, 7 widens `equatable`: different functions); the second to land rebases.
- **Group 2:** task 2 after 1; tasks 4 and 5 after 3 (together: 4 is `syntax` and `check/src/body/{try_expr,let_else}.rs`, 5 is `check/src/body/from_bytes.rs`, `kir/src/lower/bytes_read.rs` and `runtime/src/bytes.rs`); task 8 after 3 **and 4** (8 and 3 share `check/src/tables.rs`, which task 3 splits; 8 and 4 share `syntax/src/ast.rs`, `sexpr.rs`, `visit.rs` and `lsp/src/`, and `ast.rs` is 366 lines, so task 8 splits it if it passes 400); task 9 after 8. Tasks 2 and 3 both add cases to `cli/tests/reload.rs` (append only; the second rebases).
- **Group 3:** task 6 after 2, 4 and 5 (2 and 6 both rewrite Pong's `check_file`); task 10 after 2, 6, 7 and 9 (6 and 10 both edit `demos/pong/pong.kes` and `cli/tests/pong_demo.rs`); task 11 after 9, alongside 10 (only `tests/errors/rollback_step_not_pure.kes` is shared: 10 respells the interface, 11 changes the message; the second to land rebases). Task 11's whole-suite output comparison takes its baseline from the commit after task 10 lands (or task 10's own commit if 11 finishes first).
- Task 12 last.
- **Files near 400 lines:** the first task that touches each splits it in a step of its own, before its change: `kir/src/lower/reload_hash.rs` (395, task 1: move the statement walk to `reload_hash_stmt.rs`), `kir/src/range/events.rs` (398, task 1: `fail`'s event goes in `events/reasons.rs` or a new file, never `events.rs`), `driver/src/load.rs` (397, task 3: the prelude goes in a new `driver/src/prelude.rs`), `check/src/tables.rs` (370, task 3: the interface tables move to `tables_iface.rs`, as tasks 3, 8 and 11 all add to it). Watch, and split if passed: `check/src/body/mod.rs` (382), `check/src/collect.rs` (376, tasks 3 and 11), `kir/src/lower/mod.rs` (371, tasks 1 and 7), `check/src/generic/copies.rs` (368, task 9), `syntax/src/ast.rs` (366, tasks 8 and 4), `kir/src/lower/bytes_read.rs` (357, task 5), `check/src/body/lists.rs` (353, task 7).

---

### Task 1: `exit` and `fail` in the language and runtime (decisions 1-4)

**Files:**
- Modify: `kestrel/check/src/builtins.rs` (`Exit` and `Fail` in `Builtin`, both in `GLOBAL_NAMES`, result `Ty::Never`, `is_pure`: `fail` yes, `exit` no), `kestrel/check/src/body/builtins.rs` (argument types and messages; `exit` in a written `pure` function and in a `par for` body; `exit` marks the function's `io`), `kestrel/check/src/body/call.rs` (`check_discarded_call` skips `never` built-ins, so a `fail(...)` statement isn't "the result of `fail` is unused"), `kestrel/check/src/body/consts.rs` (`is_const_expr` refuses `fail` and `exit` with decision 1's message), `kestrel/check/src/body/mod.rs` (`let n = fail("x")`: decision 1's message instead of `reject_never`'s)
- Modify: `kestrel/kir/src/ir_stmt.rs` (today's `Stmt::Fail(String)` becomes `Fail { message: Expr, line: u32 }`, line 0 for the method-table copies in `lower/iface_copy.rs`; new `Exit { code: Expr, line: u32 }`; both end their block like `Return`), the call lowering for built-ins in `kestrel/kir/src/lower/` (find where `print` lowers; put the two in a new `lower/stop.rs`)
- Modify every walker that matches `Stmt::Fail` or `Trap` (each must now walk the statement's expression, as `Return(Some(e))`, and keep ending the block): `kestrel/kir/src/{visit,visit_mut,rename,promote}.rs`, `kir/src/inline/descend.rs`, `kir/src/lower/{scratch,reload_hash,iface_copy}.rs`, `kir/src/range/{stmt,writes}.rs`, `kir/src/range/events/{stmts,reasons}.rs`
- Split first: `kir/src/lower/reload_hash.rs` (395 lines: move its statement walk to a new `lower/reload_hash_stmt.rs`, no change in behaviour, as its own step) before adding the two arms; `fail`'s event goes in `range/events/reasons.rs` or a new `range/events/stop.rs`, never `range/events.rs` (398 lines)
- Modify: `kestrel/kir/src/range/events/reasons.rs` (`fail` a way to fail, "`fail` stops the program"; allocations inside `fail`'s argument not counted for `no_alloc`), `kestrel/kir/src/range/contracts.rs` (decision 4's direct form "it can fail on line 3: `fail` stops the program" and the caller's chain), `kestrel/kir/src/range/report.rs` ("line 3: can fail: `fail` stops the program")
- Modify: `kestrel/codegen/src/func/stmt.rs`, `kestrel/codegen/src/rt.rs` (the two runtime symbols, both `noreturn`, followed by a trap)
- Modify: `kestrel/runtime/src/panic.rs` (`kestrel_rt_fail(msg, line, file) -> !` through `fail`, with " on line L" and the file as `kestrel_rt_panic` writes them; `kestrel_rt_exit(code, line, file) -> !`: the 0-to-255 check through `fail` naming the line, `print::flush`, then `replay::on_exit(code)` (a stub that task 2 fills), then the exit hook if set, else `process::exit`; `set_exit_hook`), `kestrel/runtime/src/lib.rs` or the JIT symbol table that lists runtime functions
- Test: `kestrel/tests/programs/{exit_code,exit_range,fail_pure,fail_value,fail_par}.kes`, `kestrel/tests/errors/{exit_fail_errors,prove_no_panic_fail}.kes` (with a `fail(...)` statement that must not be an unused result, `let X = fail("x")` at top level, and `let n = fail("x")`), a `check` unit test that both names are reserved, a `--report` case in `cli/tests/report.rs`

- [ ] **Step 0: Split `reload_hash.rs`** (tests green, nothing else changed). **Step 1: Failing tests** with the spec's lines (Tests, A). **Step 2: See them fail.** **Step 3: Implement** (`never` already ends blocks for `while true`; reuse it). **Step 4: Green** (`cargo test -p kestrel-check -p kestrel-kir -p kestrel-runtime`, the programs JIT pass, `--test errors --test report`), clippy, **commit.**

**Done when:** the five programs print and exit as the spec says in the JIT, the error tests pass, `@prove(no_panic)` fails naming `fail`, and a `no_alloc` function whose only allocation is `fail`'s message proves.

**Review Focus:** range analysis after `if bad { fail(...) }` knows `bad` is false, as after a `return`; a `fail` in a `par for` reports the lowest failing index.

### Task 2: Exit codes in replays and hosts; the library refuses (decisions 5-6)

**Files:**
- Modify: `kestrel/runtime/src/replay_file.rs` (`VERSION = 2`, entry kind 3 `Exit(i32)`, version 1 still read), `kestrel/runtime/src/replay/{mod,compare}.rs` (`on_exit(code)` finishing a recording with the entry, and in a replay comparing endings: "replay differs at the end: the recording exited with code 0, this run with 1" or "... failed"), `kestrel/runtime/src/replay_test.rs`
- Modify: `kestrel/cli/src/replay_cmd.rs` (the ending comparison's report and exit code), `kestrel/cli/src/reload/mod.rs` (the session's code when `exit` runs on the game thread; `frame` false stays 0), `kestrel/cli/src/main.rs` if `run` maps the program's code
- Modify: `kestrel/godot/src/worker.rs` (100 lines; spec decision 3: `set_exit_hook` beside `set_failure_hook`; `exit(0)` sends the call's output back as a normal end with no result, another code as `Reply::Failed` with `the program called exit(3)` (or a new `Reply` variant), then the worker parks as `on_failure` does and the next call starts a new worker; a `fail` stays the panic path; the host process never exits)
- Modify: `kestrel/driver/std/rollback.kes` (`new_session` with decision 6's two `fail`s; the comment's clamping sentence goes), `kestrel/tests/programs/rollback_delay.kes` (the `clamped:` lines go)
- Modify: `kestrel/demos/pong/pong.kes` (`check_file`: `exit(1)` after printing a differing check, `exit(2)` after a file that can't be read or isn't a replay), `kestrel/cli/tests/pong_demo.rs` (the three codes)
- Test: `kestrel/tests/programs/{rollback_bad_me,rollback_bad_delay}.kes`, `kestrel/cli/tests/exit_codes.rs` (new: the spec's CLI cases), `kestrel/cli/tests/replay.rs` (an `Exit` entry), `kestrel/cli/tests/reload.rs` (one case: `frame` calls `exit(4)` on frame 3; the process ends with 4)

- [ ] **Step 1: Failing tests. Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-runtime`, programs JIT, `--test exit_codes --test replay --test reload --test pong_demo`), clippy, **commit.**

**Done when:** a recorded `exit(3)` replays to `replay matches`, a changed code is reported at the end, version 1 files replay, Pong's check exits 0, 1 and 2 with its old lines, and `new_session(..., me: 7, ...)` fails with the spec's sentence naming `rollback.kes`.

**Review Focus:** Review Focus 2; `on_exit` runs before the hook and before `process::exit`, and a replay that matches still exits 0; the Godot worker parks after an exit exactly as after a failure.

### Task 3: The built-in `Result` (decisions 7-8)

**Files:**
- Create: `kestrel/driver/std/prelude.kes` (the enum and a comment saying what it is for)
- Create: `kestrel/driver/src/prelude.rs` (`load.rs` is 397 lines: loading the prelude and adding it to the modules passed to `link` live here, `load.rs` only calls it)
- Modify: `kestrel/driver/src/builtin.rs` (the prelude's part, never found by `use`), `kestrel/driver/src/load.rs` (one call), and the module each file gets from the driver (every file, built-ins included, sees the prelude's `pub` items as if imported; a clash names the reserved type, not an import). `syntax::link`'s own logic doesn't change, so syntax tests that link without the driver (`parser_brackets.rs:79`) stay as they are.
- Split first: `kestrel/check/src/tables.rs` (370 lines; tasks 3, 8 and 11 all add to it): move the interface tables (`IfaceInfo`, `ifaces`, `iface_by_name` and their helpers) to a new `check/src/tables_iface.rs`, no change in behaviour, as its own step
- Modify: `kestrel/check/src/collect.rs` (`Result` a reserved type name: "`Result` is a built-in type; pick another name"), `kestrel/check/src/body/mod.rs` (`coerce`, line 344, calls the new wrap), Create: `kestrel/check/src/body/ok_wrap.rs` (the `Ok` conversion, converting to `T` first, and its two refusals; `E` is never wrapped), Modify: `kir/src/lower/expr.rs` (the `Ok` wrap applied where `ExprKind::Some` is today), `kestrel/check/src/generic/rules.rs` (lines 52-54: an optional may be `T` or `E` when the owner is the prelude's `Result`), `kestrel/check/src/lib.rs` or `tables.rs` (a way to know the `Result` enum: its declaration's module is the prelude)
- Modify: `kestrel/lsp/src/` (hover on `Result` names the prelude; definition jumps to it as to a built-in module's item)
- Modify (renames to `Outcome`): `kestrel/tests/programs/generic_result.kes`, `kestrel/tests/errors/{generic_written,generic_infer}.kes`, `kestrel/check/tests/generics_{written,bodies,infer}.rs`, `kestrel/cli/tests/modules_generics.rs`
- Test: `kestrel/tests/programs/{result_basic,result_bytes}.kes`, `kestrel/tests/errors/result_errors.kes` (decisions 7-8's messages; tasks 4 adds its own), `kestrel/cli/tests/reload.rs` (a state holding a `Result[int, string]` keeps it across an edit), a driver unit test that the prelude is not a module (`use prelude` finds nothing)

- [ ] **Step 0: Split `tables.rs`** (tests green). **Step 1: Failing tests. Step 2: See them fail. Step 3: Implement** (no new layout, printing or byte code: it is a generic enum). **Step 4: Green** (`-p kestrel-driver -p kestrel-syntax -p kestrel-check -p kestrel-lsp`, programs JIT, `--test errors --test modules_generics --test reload`), clippy, **commit.**

**Done when:** `result_basic.kes` and `result_bytes.kes` print the spec's lines, every renamed test passes with `Outcome` in its lines, and a user `enum Result` is refused.

**Review Focus:** the prelude is linked once and its items are one definition across modules (a `Result[int, string]` from a module is the same type as the program's); the `Ok` conversion fires in every position decision 8 lists and nowhere a `T` doesn't go into a `T?`.

### Task 4: `try` and `let ... else why` (decisions 9-11) **(review)**

**Files:**
- Modify: `kestrel/syntax/src/token.rs` and `lexer.rs` (`try` keyword), `kestrel/syntax/src/ast.rs` (`ExprKind::Try`, the `else` name on a binding), `kestrel/syntax/src/parser/expr.rs` (unary `try`), `kestrel/syntax/src/parser/stmt.rs` (`else IDENT? block`), `kestrel/syntax/src/{sexpr,visit}.rs`
- Create: `kestrel/check/src/body/try_expr.rs` (typing and every refusal of decision 9); Modify: `kestrel/check/src/body/let_else.rs` (`Result` and `else why`), `kestrel/check/src/body/stmt.rs` (a `try` statement is not a no-effect line), `kestrel/check/src/body/regions.rs` (the arena message for `try`)
- Create: `kestrel/kir/src/lower/try_expr.rs` (tag test, `Err` built from the payload, `Return`); Modify: the `let ... else` lowering (on a `Result`, the error bound in the block)
- Modify: `kestrel/lsp/src/` (keyword list and highlighting)
- Test: `kestrel/tests/programs/{result_try,result_else,result_arena}.kes`, `kestrel/tests/errors/result_errors.kes` (decisions 9-10's messages), `kestrel/syntax/tests` parser cases (`try a.b(c) + 1`, `else why {`), a `kir` unit test that a `try` and the hand-written `match` lower to the same statements

- [ ] **Step 1: Failing tests. Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-syntax -p kestrel-check -p kestrel-kir -p kestrel-lsp`, programs JIT, `--test errors`), clippy, **commit.**

**Done when:** the three programs print the spec's lines, every refusal has its error test, and the `kir` test shows `try` equal to the `match`.

**Review Focus:** Review Focus 1; `try` on an optional; `else why` reads the payload in place for a place and moves it for a temporary, as `let ... else` does for optionals; `KESTREL_POISON=1` on `result_arena.kes`.

### Task 5: `from_bytes` gives `Result[T, string]` (decision 13)

**Files:**
- Modify: `kestrel/check/src/body/from_bytes.rs` (the result type), `kestrel/kir/src/lower/bytes_read.rs` (a reason code per failure and the offset local; `decode_fn` builds `Ok` or `Err` with the runtime call; if it passes 400 lines, move the header checks to `lower/bytes_header.rs`), `kestrel/runtime/src/bytes.rs` (`kestrel_rt_bytes_reason(code, at, len, type_name)` writing decision 13's sentences in the current arena), `kestrel/codegen/src/rt.rs`
- Modify: `kestrel/tests/programs/{from_bytes_none,from_bytes_roundtrip,from_bytes_sizes,from_bytes_fuzz,bytes_proofs,to_bytes_constant}.kes`, `kestrel/tests/errors/{from_bytes_no_alloc,from_bytes_refused,from_bytes_refused_generic,from_bytes_no_type}.kes` (only where a line changes), `kestrel/kir/tests/bytes_read.rs`, `kestrel/cli/tests/from_bytes_reference.rs`, `kestrel/bench/io/bytes.kes`, `kestrel/driver/std/rollback/replay.kes` and `kestrel/tests/programs/rollback_replay.kes` if they read bytes
- Test: as above; `from_bytes_none.kes` prints each reason with a `match` (no generic helper: a `show[T](r: Result[T, string])` can't take `T = int?`, and the test reads `int?` values)

- [ ] **Step 1: Update the tests to the reasons. Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-kir -p kestrel-runtime -p kestrel-check`, programs JIT, `--test errors --test from_bytes_reference --test bytes_reference`), clippy, **commit.**

**Done when:** every reason of decision 13 has a line in `from_bytes_none.kes`, the 10,000-mutation fuzz still never panics, and a valid read allocates as before (`from_bytes_no_alloc.kes` unchanged in meaning).

**Review Focus:** the reason sentence is built only on the failure path; the offsets are the byte where reading went wrong.

### Task 6: `file` gives `Result`s; the voxel engine and Pong on them (decisions 12, 14)

**Files:**
- First: confirm that a non-`pub` `native` is accepted and callable inside a built-in module (none exists yet; a driver unit test with a scratch module part); if it isn't, make it so in `check/src/natives.rs` before anything else.
- Modify: `kestrel/driver/std/file.kes` (private natives `read_raw`, `read_text_raw`, `write_raw`, `write_text_raw`, `list_dir_raw`, `make_dir_raw`, `remove_raw` and `error_raw`; `exists` stays a `pub` native; Kestrel wrappers with the old names; the module comment's example with `try` and `else why`; `error()` goes), `kestrel/kir/src/native.rs` (`native_symbol`: a built-in module's `_raw` native imports the symbol of the name without the suffix, so `file.read_raw` is `kestrel_file_read`; a unit test), `kestrel/kir/src/range/events/effects.rs` (`Effect::of_native` and `PINNED` look the name up without `_raw`, so their keys stay `file.read` ...; the chain through a built-in wrapper whose only effect is its `_raw` native is one link, "it calls `file.read` (line 8), which reads a file"), `kestrel/runtime/src/natives/{file,mod}.rs` (no symbol changes; touch only if the error-text native needs it), `kestrel/driver/tests/builtin.rs`
- Modify: `kestrel/voxel/save.kes` (`save_world`, `load_world` per decision 14; `load_failure` goes), `kestrel/voxel/main.kes` (F5 and F9 print the error), `kestrel/voxel/tests/save_then_load.kes` (its lines)
- Modify: `kestrel/demos/pong/pong.kes` (`save_replay`, `check_file` with `else why`, exit codes from task 2 kept)
- Modify: `kestrel/cli/tests/file_module.rs` (every native's error through the `Result`), `kestrel/bench/io/files.kes`, `kestrel/tests/errors/prove_deterministic_file.kes` (its three messages unchanged; `keep` and `tidy` return `Result[bool, string]`, and `tidy` reads `file.exists(...)` then `try file.remove(...)`)
- Test: as above, plus `cli/tests/voxel_engine.rs` and `pong_demo.rs` unchanged and green

- [ ] **Step 1: Update the tests. Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-driver`, programs JIT, `--test errors --test file_module --test voxel_engine --test pong_demo`), clippy, **commit.**

**Done when:** no `.kes` file calls `file.error()` or `load_failure`, the voxel save test prints the spec's two new lines, and Pong's outputs are byte for byte.

**Review Focus:** the wrappers' error text is the native's, unchanged; no `kestrel_file_*` symbol and no `effects.rs` key changed (`every_windowless_native_has_its_own_effect` green); `prove_deterministic_file.kes` keeps its three messages word for word.

### Task 7: `==` on plain structs and enums; `equatable` widened (decision 19)

**Files:**
- Modify: `kestrel/check/src/body/ops.rs` (`==`/`!=` on equatable structs and enums, the refusal naming the first field that isn't), `kestrel/check/src/generic/rules.rs` (`equatable` per decision 19, judged per instance; the rule message's new list), `kestrel/check/src/body/lists.rs` (353 lines: `has`/`find` and their message's new list; split if it passes 400)
- Modify (each rewritten to a case that still errors, with the new message, never deleted): `kestrel/check/tests/structs.rs:96` (`P == P`: `P` gains a list field), `kestrel/check/tests/collections.rs:139` (`find` on a `list[S]`: `S` holds an interface value), `kestrel/check/tests/generics_infer.rs:299` (`Enemy` gains a `list[int]` field); `tests/errors/list_methods.kes:21` and `check/tests/gpu.rs:181` stay errors unchanged
- Create: `kestrel/kir/src/lower/eq.rs` (inline field compares for structs of scalars; a generated `__kestrel_eq_N` per other type, like the checksum helpers); Modify: where `kir` lowers `==` and `has`/`find`
- Test: `kestrel/tests/programs/equatable_structs.kes`, `kestrel/tests/errors/` (the two refusals, in `iface_generic_errors.kes` or a new `equatable_errors.kes`), a `contracts` case: a function comparing two `Pad`s proves `no_alloc, no_panic`

- [ ] **Step 1: Failing tests. Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-check -p kestrel-kir`, programs JIT, `--test errors --test contracts`), clippy, **commit.**

**Done when:** `equatable_structs.kes` prints its lines in the JIT, `count[T: equatable]` works over a struct, and a `World ==` says which field.

**Review Focus:** NaN and `-0.0` follow float `==`; an enum compares tags before payloads and never reads another variant's bytes; recursion through a generic instance terminates.

### Task 8: Interface placeholders: syntax, instances, conformance (decisions 15-16)

**Files:**
- Modify: `kestrel/syntax/src/parser/iface.rs` (placeholders after the name; angle brackets get the structs' message; a method's own `[` gets decision 15's message), `kestrel/syntax/src/parser/item.rs` (interface lists of `iref`), `kestrel/syntax/src/parser/generics.rs` (rules as `iref`), `kestrel/syntax/src/ast.rs` (366 lines; `IfaceRef { name, span, args: Vec<TypeExpr> }` for lists and rules; split the declarations out if it passes 400), `kestrel/syntax/src/sexpr.rs` (lines 171-174), `kestrel/syntax/src/visit.rs`, `kestrel/syntax/src/link/walk.rs` and `link/chains.rs` (dotted refs with types)
- Modify: `kestrel/check/src/tables_iface.rs` (split off in task 3: `IfaceInfo.params`, `instance`), `kestrel/check/src/iface.rs` (`resolve_list`; methods of an instance substituted, each instance with its own abstract method `FnId`s; conformance per listed instance with the instance's types in the message; one instance per interface per type; a generic type's list with its own placeholders refused, decision 16), `kestrel/check/src/generic/declare.rs` (`apply_rules`; the two-rule check at lines 145-164 gains decision 17's two-instance case), `kestrel/check/src/generic/instances.rs` (lines 41, 84, 121, 133, 161, 166: an instance copies `decl.ifaces` as instances) and `expand.rs` (interface instances interned like struct instances; the endless-copies check follows them), `kestrel/check/src/collect_ty.rs` (`Handler[Click]` in type positions; a bare generic interface needs its types)
- Modify: `kestrel/kir/src/lower/iface.rs` (`implementations()` finds the interface by the instance's own method `FnId`s, not `position(|i| i.methods.contains(..))` across declarations; method tables are already keyed by `(KTy, IfaceId)` in `lower/tables.rs`), `kestrel/kir/src/lower/prove.rs:53` (abstract methods walked per instance, not by declaration span), `kestrel/kir/src/range/summary/cycles.rs:39-61` (implementations keyed by the abstract method's `FnId`, not its printed name, so `Handler[Click].handle` and `Handler[Key].handle` never merge)
- Modify: `kestrel/lsp/src/find.rs:66-97` (interface lists and rules as `IfaceRef`), `kestrel/lsp/src/resolve.rs` (hover writes the instance)
- Modify: `kestrel/tests/errors/{generic_parse,newcomer_spellings}.kes`, `kestrel/syntax/tests/parser_generics.rs`
- Test: `kestrel/tests/programs/iface_generic.kes` (its value-type half: `list[Handler[int]]`, borrowed and boxed calls; two types implementing `Handler[int]` and `Handler[string]` under `@prove(no_panic)`, so a contract through one instance checks only its own implementations), `kestrel/tests/errors/iface_generic_errors.kes` (two instances, bare name, method placeholder, angle brackets, `struct Box[T]: Holder[T]`), parser tests

- [ ] **Step 1: Failing tests. Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-syntax -p kestrel-check -p kestrel-kir -p kestrel-lsp`, programs JIT, `--test errors --test modules`), clippy, **commit.**

**Done when:** a `list[Handler[int]]` holds two struct types and calls through the table, every non-generic interface test passes unchanged, and the errors read as the spec says.

**Review Focus:** Review Focus 3; `rollback.Rollback` (still non-generic until task 10) and every existing interface keep working unchanged.

### Task 9: Interface instances in rules, inference and copies (decisions 17-18) **(review)**

**Files:**
- Modify: `kestrel/check/src/generic/rules.rs` (a rule is an instance), `kestrel/check/src/generic/infer.rs` and `kestrel/check/src/body/infer.rs` (the step between values and expected type; decision 18's conflict message), `kestrel/check/src/body/param_calls.rs` (calls through an instance rule use its substituted signature), `kestrel/check/src/generic/copies.rs` (368 lines: a copy's rule instances substituted; split if it passes 400), `kestrel/kir/src/lower/iface.rs` (tables for instance types inside copies)
- Test: `kestrel/tests/programs/iface_generic.kes` (its generic half: `[H: Handler[I], I: equatable]`, `I` from the list; an interface type as `H`), `kestrel/tests/errors/iface_generic_errors.kes` (the conflict and the double rule), `kestrel/check/tests/generics_infer.rs` cases

- [ ] **Step 1: Failing tests. Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-check -p kestrel-kir`, programs JIT, `--test errors --test modules_generics --test contracts`), clippy, **commit.**

**Done when:** `iface_generic.kes` prints all its lines, a copy's call through `Handler[I]` is direct (the report shows it inlined where it was before for plain rules), and the conflict message names both sources.

**Review Focus:** Review Focus 3; the inference step runs after the values and before the expected type, and only for placeholders still open.

### Task 10: The rollback library on `Rollback[I]`; Pong on `Pad` (decisions 20-21)

**Files:**
- Before any change: run today's Pong headless for 600 frames and with `DESYNC_AT = 300` and check both against `kestrel/cli/tests/pong_expected.txt` and `pong_expected_desync.txt` (they must already match).
- Modify: `kestrel/driver/std/rollback.kes` (`Rollback[I]`, `Packet[I]`, `new_session(start, me, delay, idle)` keeping `idle` as a field and as `last`'s start in place of `Input()` (line 96), the comment's example with `Rollback[Pad]`), `kestrel/driver/std/rollback/session.kes` (`Session[G: Rollback[I], I: equatable]`; `idle` wherever `Input()` was, in `advance` (line 144) and `run_frame` (159); histories and `outgoing`'s inputs (218) `list[I]` built with `add`; `==` instead of `same`), `rollback/replay.kes` (`Replay[G, I]`), `rollback/fakenet.kes` (`FakeNet[I]`; `new_fake_net(seed, latency, jitter, loss, idle)` storing `idle` and filling its 32 slots (lines 81-90) and `shortened` (95) with `SENT` copies of it)
- Modify: `kestrel/tests/programs/rollback_*.kes` (`Rollback[Input]`, `Session[Tally, Input]`, `idle: Input()` to `new_session` and, in `rollback_fakenet_pong.kes`, `rollback_lost.kes` and every other caller, to `new_fake_net`; lines unchanged), `kestrel/tests/errors/rollback_*.kes` (spelling), `kestrel/cli/tests/{poison,snapshot_memory}.rs` (spelling and `new_fake_net`'s `idle`), `kestrel/cli/tests/contracts_rollback.rs` (line 101: `new_session[Rollback](g, ...)` becomes `new_session[Rollback[Input], Input](g, ...)`)
- Modify: `kestrel/demos/pong/game.kes` (`Game: Rollback[Pad]`, `step` on `Pad`s, `bot` gives a `Pad`; `UP`, `DOWN`, `held` go), `kestrel/demos/pong/pong.kes` (`Session[Game, Pad]`, `Replay[Game, Pad]`, `FakeNet[Pad]`, keys read into a `Pad`, `idle: Pad()` to `new_session` and to `new_fake_net` at line 250; task 6's `check_file` and task 2's exit codes kept), `kestrel/cli/tests/pong_demo.rs` (decision 21's offsets and byte, on top of tasks 2 and 6)
- Test: `kestrel/tests/programs/rollback_own_input.kes`, all of the above

- [ ] **Step 1: Write `rollback_own_input.kes` and update the offsets. Step 2: See them fail. Step 3: Rewrite the library, the tests' spellings, then Pong. Step 4: Green** (programs JIT, `--test errors --test contracts_rollback --test snapshot_memory --test poison --test pong_demo --test reload --test replay`), clippy, **commit.**

**Done when:** every rollback program prints its old lines, Pong's outputs are byte for byte, its replay is 4 input bytes a frame, and `advance_peer` still proves `deterministic, no_alloc`.

**Review Focus:** Review Focus 5; the session's frames before `delay` use `idle` exactly where they used `Input()`; `FakeNet` sends without allocating (`snapshot_memory.rs`); `==` on `Pad` is inline.

### Task 11: Inferred `pure` (decisions 22-29) **(review)**

**Files:**
- Create: `kestrel/check/src/purity.rs` (the pass: direct effects and call edges recorded while bodies are checked, the fixed point, then the obligations: written-`pure` calls, `par for` calls, required closures, `pure`-typed uses of named functions, implementations of `pure` interface methods, unused results; each error with its chain), `kestrel/check/src/purity_chain.rs` if the chain text passes 400 lines together
- Modify: `kestrel/check/src/tables.rs` (`FnInfo.pure_written`; `is_pure` set by the pass), `kestrel/check/src/collect.rs` (376 lines; `pure_written` from the declaration, line 350's `Sig` stays built from it; split if it passes 400), `kestrel/check/src/body/call.rs` (`check_callee_purity` and `check_discarded_call` record instead of erroring for unwritten callees; the unused-result obligation keeps today's `var`-parameter exemption and skips `never` and no-value calls, decision 27), `kestrel/check/src/body/{closure,fnvalue,list_fns,par}.rs` (the same), `kestrel/check/src/body/param_calls.rs` (line 34 picks a rule method by `pure_written`), `kestrel/check/src/closure.rs`, `kestrel/check/src/iface.rs` (a missing `pure` on an implementation moves to the pass), `kestrel/check/src/lib.rs` (run the pass after bodies, before anything reads `is_pure`)
- Modify: `kestrel/lsp/src/resolve.rs` (hover `pure (inferred)`)
- Modify: the tests decision 29 lists (`tests/errors/{par_for,purity,closure_types,generic_bodies,iface_use,list_fns,list_sort,rollback_step_not_pure}.kes`, `check/tests/{check,structs,natives,closures,list_fns,list_sort,fn_values}.rs`), and `tests/programs/owning_temporaries.kes:60` (`make(1)` becomes `let kept = make(1)`; its lines unchanged). Any other program the whole-suite run shows with a new unused-result error is fixed the same way, by binding the value, and named in the task report and the commit message.
- Test: `kestrel/tests/programs/pure_inferred.kes`, `kestrel/tests/errors/pure_chain.kes`, an LSP hover test

- [ ] **Step 1: Failing tests** (and the changed error tests). **Step 2: See them fail. Step 3: Implement. Step 4: Green** (`-p kestrel-check -p kestrel-kir -p kestrel-lsp`, programs JIT, `--test errors --test contracts --test report`); then run the programs suite once more and compare every program's output with the baseline (the commit after task 10 lands, or task 10's own commit; they must be equal), clippy, **commit.**

**Done when:** `pure_inferred.kes` prints the same as its all-`pure` version, `pure_chain.kes` passes, no program test changed its output, and every program that gained an unused-result error (`owning_temporaries.kes` and any the suite finds) binds its value and is listed.

**Review Focus:** Review Focus 4; mutual recursion with an effect in one function marks the whole cycle impure; a generic function's purity is one answer for all copies; `kir/src/lower/lines.rs` storing fewer lines changes no runtime error text.

### Task 12: Measurements, docs, full suite (the controller)

**Files:**
- Create: `kestrel/bench/kernels/result.kes` (the spec's hot loop, its variants selected by a constant or an argument)
- Modify: `kestrel/bench/kernels/results.md` ("Result and try": the spec's measurements), `kestrel/bench/determinism/results.md` or `bench/rollback/` (Pong's network frame on `Pad`, replay size), `docs/superpowers/specs/2026-10-09-kestrel-syntax-design.md` (Built-in functions: `exit`, `fail`; Types: `Result[T, E]`; Optional values: `try`, `else why`; Interfaces and Generics: placeholders on interfaces, rules with types, `equatable` structs, "Not built" updated; Declarations: `pure` inferred, a written `pure` a promise; `file` table with `Result`s and `error()` gone; `from_bytes` reasons; the rollback paragraph: `Rollback[I]`, `idle`, the refusals; Grammar sketch; Out of scope: "error types beyond `?`" goes), `README.md` (a short "Errors" paragraph: `Result`, `try`, `else why`, `fail`, `exit`), `docs/superpowers/plans/2026-10-09-core-2a-followups.md` (58, 83 and 96 closed or updated; new: catching `fail`, `Result` methods, a `must use` rule, a unit type, packages' round), `docs/superpowers/plans/2026-10-10-handoff.md` (Start here), `CLAUDE.md`
- Test: `cargo test --workspace --no-fail-fast` (with the exe pass), `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 1: Measurements (PC free only). Step 2: Docs. Step 3: Full suite and clippy. Step 4: The round review** (a reviewer reads the whole diff against the spec and the Review Focus). **Step 5: Commit and push.**

**Done when:** the numbers are in the results files with median, range and load; the syntax spec describes everything built; the full suite and clippy are green; the review's findings are fixed or listed as follow-ups.

**Review Focus:** release builds on a quiet PC; the docs name every alternative in the spec's Names table; packages are recorded as the next round with decisions 30-33 as its starting point.
