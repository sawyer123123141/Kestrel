# Performance Round Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** every measured case within 1.3x of Rust `--release` (the spec's Goals table names the honest exceptions), by KIR and codegen fixes only, with every deterministic program's bits unchanged and no checked build slower.

**Architecture:**
- **Codegen arithmetic (Tasks 1-2):** division by any non-zero constant as pure instructions; `% 2^k == 0` as a mask; small `if`s as selects; the exact shift after a passed even test.
- **Codegen lists (Task 3):** list headers in Cranelift variables across a loop that can't change them (read-only and `add`-only lists); `map` reserves its length; plain structs copied in 8-byte moves.
- **KIR before range analysis (Tasks 4-5):** `parts[i]` bound once and promoted; plain enums split into a tag and leaves.
- **KIR after range marking (Tasks 6, 7, 9):** accumulator recursion as a loop (`--release`), loop-invariant checks before the loop plus the nest-counter range rule, xor/and/or chains regrouped.
- **Runtime sort (Task 8):** keys filled by compiled code, a stable radix sort of `(key bits, position)`, one permutation.

**Tech Stack:** Rust (Cargo workspace `kestrel/`), Cranelift 0.137. Tests in `kestrel/codegen/tests`, `kestrel/kir/tests`, `kestrel/runtime` unit tests, `kestrel/tests/programs`, `kestrel/tests/errors`, `kestrel/cli/tests`.

**Spec:** `docs/superpowers/specs/2026-11-01-perf-round-design.md` (read it whole; decisions 1-9).

**Ledger:** `.superpowers/sdd/2026-11-01-perf-round/progress.md` (git-ignored), one line per task, plus briefs, reports and reviews.

**Parallel lanes.** After Task 0 the tasks run in five lanes, each in its own `git worktree` with its own short `CARGO_TARGET_DIR` (`C:/kt/pa` ... `C:/kt/pe`), merged into `main` in task order with a rebase:

| Lane | Tasks | Files only this lane touches |
|---|---|---|
| A | 1, then 2 | `codegen/src/func/divide.rs`, new `magic.rs`, `select.rs`, `expr.rs`, `stmt.rs` (`if_stmt` only), `arith.rs` |
| B | 3 | `codegen/src/func/headers.rs`, `place.rs`, `collections.rs`, `lists.rs`, `kir/src/lower/list_fns.rs`, `ListOp::Reserve` across `kir`, `runtime/src/list.rs` |
| C | 4 and 5 in parallel | 4: `kir/src/bind_index.rs`, `promote.rs`; 5: `kir/src/scalarize.rs`, `leaves.rs`, `split.rs` |
| D | 6, 7 and 9 in parallel | 6: `kir/src/recurse.rs`; 7: `kir/src/hoist_checks.rs`, `kir/src/range/loops.rs`; 9: `kir/src/reassoc.rs` |
| E | 8 | `kir/src/lower/sort.rs`, `runtime/src/sort.rs` |

Disjoint pairs that can run at the same time: any two lanes; inside lane C, Tasks 4 and 5; inside lane D, Tasks 6, 7 and 9. Task 0 lays every seam the lanes would otherwise share (`kir/src/lib.rs`, `codegen/src/lib.rs`, the loop hooks in `stmt.rs`, `rt.rs`/`jit.rs` registrations), so no two lanes edit one file. Task 2 waits for Task 1 (both edit `expr.rs`; Task 2 reuses Task 1's mask-test recogniser). Task 10 runs last, after every merge.

## Global Constraints

- **Builds and tests:** idle priority; `CARGO_BUILD_JOBS=8`, `--test-threads=8` when the PC is free; **none at all while the user is gaming** (the controller says which). Five lanes at once only when the PC is free; otherwise one lane at a time. Per task: the touched crates plus `--test programs --test errors` JIT-only; the exe pass and the full suite once, in Task 10.
- **No benchmarks before Task 10** except a quick single-kernel check a task asks for, on a free PC, old and new in turn.
- **Never a visible window:** `KESTREL_HEADLESS=<frames>` with `KESTREL_HEADLESS_QUIET=1`, or `KESTREL_HIDDEN=1`.
- **Never call a bare `link.exe`.** On LNK1104 use a short `CARGO_TARGET_DIR`.
- **Files under about 400 lines;** split by responsibility. Messages and comments are plain sentences that say what to do.
- **No syntax, keyword, built-in, library name or Cargo dependency.** No float operation reassociated, contracted, reordered or replaced.
- **Every existing test keeps its output**, except arena-byte numbers that Task 3's `map` reserve changes, each listed in its commit message with the reason.
- **`kestrel check --report` keeps its text;** counts may only go down (Tasks 4, 5). Passes after range marking never touch it.
- **Never stage** `kestrel/test.kes`, `voxels.exe`, `kestrel/b.exe` or `kestrel/check/src/body/enums.rs`.
- **One commit per task**, a sentence saying what changed and why, ending with the Co-Authored-By line the session gives; the controller merges and pushes.

## Review Focus

1. **Exact for every input:** the magic numbers (Task 1), the exact shift (Task 2), the accumulator (Task 6) and the regrouping (Task 9) give the same bits as the code they replace, for every value, `MIN` and `MAX` included.
2. **No stale header (Task 3):** each case (a)-(d) of spec decision 3 stops caching, and a program test for each tries to read an old `data` or `len`; the write-through length is in memory before anything else in the loop can read it.
3. **Same failure:** a failing index, overflow or division names the same line, message and index, after the same output, in Tasks 3, 4 and 7.
4. **Determinism:** the results hash, Pong's reference outputs and the `det_*` and rollback programs agree in JIT, checked exe and `--release` exe after every task.
5. **No checked build slower,** and nothing speculated by a select can trap, print or write memory (Task 2).

---

### Task 0: Seams for the lanes

**Files:**
- Create: `kestrel/kir/src/after.rs` (`pub fn after_marking(m: &mut Module, arith: Arith)`: calls `recurse::run`, `hoist_checks::run`, `reassoc::run` in that order, each a no-op today; doc comment: these run on the build's analysed copy, so `--report` and contracts never see them), `kestrel/kir/src/recurse.rs`, `hoist_checks.rs`, `reassoc.rs` (each `pub(crate) fn run(m: &mut Module, arith: Arith) {}` with a one-line doc saying which spec decision fills it), `kestrel/kir/src/bind_index.rs` (`pub fn bind_indexes(m: &mut Module) {}`), `kestrel/codegen/src/func/headers.rs` (`cache_headers(cond, body) -> usize` returning 0, `uncache(n)`, the `hdr_cache` field)
- Modify: `kestrel/kir/src/lib.rs` (the new modules; `optimize` calls `bind_index::bind_indexes` after `inline_small`, before `scalarize`), `kestrel/codegen/src/lib.rs` (`define_module` always works on a copy when checks are marked, and calls `kir::after_marking(&mut copy, mode)` after `mark_proven`; an already analysed module is cloned once for it), `kestrel/codegen/src/func/stmt.rs` (`while_loop` and `for_range` call `cache_headers` before the loop and `uncache` after), `kestrel/codegen/src/func/mod.rs` (the field), `kestrel/runtime/src/list.rs` and `runtime/src/sort.rs` (stubs `kestrel_rt_list_reserve(header, n, size, align)`, `kestrel_rt_sort_keyed_int(items, len, size, keys)`, `kestrel_rt_sort_keyed_float(...)`, each failing with "not built yet" until its task), `kestrel/codegen/src/rt.rs` and `jit.rs` (their declarations and JIT symbols)
- Test: none new; the existing suite is the test.

- [ ] **Step 1: Add the seams.** **Step 2: Green** (`cargo test -p kestrel-kir -p kestrel-codegen -p kestrel-runtime`, `--test programs --test errors` JIT-only), clippy. **Step 3: Before-dumps** (PC free): `bench/kernels/target/release/kernels.exe <kernel> --release --dump <fn>` for `copy.kes`, `step_index.kes`, `map.kes`, `sort_by.kes` and `--dump main` of `bench/2a` fib, collatz, loop, swarm and `bench/heightmap/heightmap.kes`, and `--dump voxels3d.mesh` of the frozen world, checked and `--release`, into `.superpowers/sdd/2026-11-01-perf-round/before/`. **Commit.**

**Done when:** every existing test passes unchanged, the dumps are saved, and each lane can start without editing a file another lane edits.

**Review Focus:** the copy in `define_module` doesn't double the compile time of a big program (time `kestrel build --release voxel/main.kes`, three runs each way); the hooks run nothing yet.

### Task 1: Division and remainder by a constant (spec decision 1)

**Files:**
- Create: `kestrel/codegen/src/func/magic.rs` (the signed magic numbers `M`, `s` for `i32` and `i64`, a pure function with unit tests; the sequence of decision 1)
- Modify: `kestrel/codegen/src/func/divide.rs` (`div_by_const`: `-2^k`, `MIN`, magic numbers for the rest; `u8` widened to `i32`), `kestrel/codegen/src/func/expr.rs` (the `Binary` arm: `x % 2^k == 0` and `!= 0` against a literal 0 lower to a mask test; the recogniser `pow2_rem_test(&Expr) -> Option<(&Expr, u32)>` is `pub(super)` for Task 2)
- Test: `kestrel/codegen/tests/divide.rs` (through the JIT, each of about 60 divisors (`±3 .. ±13`, `1000003`, `1000000007`, `65537`, `MIN`, `MAX`, `MIN + 1`, `-2^k` for k 1-62, `2^31 - 1`) times edge dividends (`0`, `±1`, `MIN`, `MAX`, `MIN + 1`, multiples of the divisor and their neighbours) and 100,000 seeded dividends, `/` and `%`, `i32` and `i64`, against Rust's `wrapping_div`/`wrapping_rem`; the dump of `loop` has no `idiv`; the dump of heightmap's checksum has no `idiv` inside the y loop), `kestrel/tests/programs/div_const.kes` (printed quotients and remainders of negative and positive dividends by `7`, `-7`, `10`, `-1000003`, and `% 2 == 0` on negatives)

- [ ] **Step 1: Failing tests.** **Step 2: See them fail** (the dumps still have `idiv`). **Step 3: Implement.** **Step 4: Green** (`cargo test -p kestrel-codegen`, `--test programs --test errors` JIT-only), clippy, **commit.**

**Done when:** every divisor matches Rust for every dividend tested, no literal divisor other than 0 and -1 emits `sdiv`/`srem`/`udiv`/`urem`, and the zero and `MIN / -1` panics keep their messages.

**Review Focus:** Review Focus 1; the `t += n` / `t -= n` correction for each sign of `M` and `d`; `n % -2^k`; `MIN`.

### Task 2: Small branches as selects, and the exact shift (spec decision 2)

**Files:**
- Create: `kestrel/codegen/src/func/select.rs` (`fn selectable(...)`: the decision's limits as named constants; lowering both sides and the selects; floats through `wide_pick.rs`'s compare-and-blend)
- Modify: `kestrel/codegen/src/func/stmt.rs` (`if_stmt` only), `kestrel/codegen/src/func/expr.rs` (the value `if` arm), `kestrel/codegen/src/func/divide.rs` (an exact `n / 2^k` used inside the then side of a passed mask test on a register local not yet assigned there)
- Test: `kestrel/codegen/tests/select.rs` (collatz's loop has a `select` and no branch on the parity; an `if` with a call, a store, a check left, a nested `if` or four operations stays a branch; a float select gives the same bits as the branch for NaN, `-0.0` and infinities), `kestrel/tests/programs/select_if.kes` (collatz lengths for 1-30 and some negatives that wrap; swarm's bounce on a few particles with exact printed floats; an `if` whose unchosen side would overflow in a checked build prints the same in both builds)

**Interfaces:**
- Consumes: Task 1's `pow2_rem_test`.

- [ ] **Step 1: Failing tests.** **Step 2: See them fail.** **Step 3: Implement.** **Step 4: Green** (`cargo test -p kestrel-codegen`, `--test programs --test errors` JIT-only), clippy. **Step 5 (PC free):** time `step.kes`, `step_index.kes`, `iface_step_direct.kes` and collatz old and new in turn; tighten the limits until none is slower beyond noise. **Commit.**

**Done when:** collatz's loop is branch-free on the parity, the step kernels aren't slower, and the program tests print the same in JIT and both exes.

**Review Focus:** Review Focus 1 and 5; an unassigned side keeps the old value; the exact shift is never used where `n` was written after the test.

### Task 3: List headers in SSA, `map` reserve, 8-byte copies (spec decision 3)

**Files:**
- Modify: `kestrel/codegen/src/func/headers.rs` (the scan of rule (a)-(d) and the cache; port the shape of `hdr_cache.rs` from `.superpowers/sdd/list-spike-partial.patch`, with rule (c)'s index and sibling-field exceptions; the add cap as a constant set from Step 5), `kestrel/codegen/src/func/place.rs` (`element_addr` and `bounds_check` read a cached `data`/`len`; `copy_fields` copies a plain struct of 4-byte leaves whose size is a multiple of 8 in 8-byte moves without the `aligned` flag), `kestrel/codegen/src/func/collections.rs` (`len` and `list_add` on a cached list: write-through length, grow path reloads `data` and `cap`), `kestrel/codegen/src/func/lists.rs` (`ListOp::Reserve`), `kestrel/kir` (`ListOp::Reserve` in the IR and every walker that matches list ops: range analysis treats it as leaving the length unchanged, contracts as an allocation), `kestrel/kir/src/lower/list_fns.rs` (`map` reserves `xs.len` before its loop), `kestrel_rt_list_reserve` in `runtime/src/list.rs` (growing through `arena.rs` `kestrel_rt_list_grow`'s allocation)
- Test: `kestrel/tests/programs/headers_*.kes`, one per case: an `add` that grows past capacity mid-loop and then reads `xs[0]`; `clear` in the loop; the list passed `var` to a call that adds to it; a sibling field of the struct holding the list written in the loop; the list in a `par for`; a closure capturing it; a list of lists whose inner list is added to while the outer is read; a loop element rebound; `xs = ys` inside the loop; each prints exact values. `kestrel/tests/errors` or a program with `// expect-stderr:` for an out-of-range index in a cached loop (same line and index as before). `kestrel/codegen/tests/headers.rs` (the copy loop's block has no load of either header; the swarm loop's element pointer comes from a variable; with each case (a)-(d) the header load is back in the loop). `kestrel/cli/tests/snapshot_memory.rs` and other arena-byte tests: update only the numbers `map`'s reserve changes, listing each.

- [ ] **Step 1: Failing tests** (the stale-header programs pass today and must keep passing; the dump tests fail). **Step 2: See them fail.** **Step 3: Implement read-only caching, then add-caching, then the reserve and the 8-byte copy.** **Step 4: Green** (`cargo test -p kestrel-codegen -p kestrel-kir -p kestrel-runtime`, `--test programs --test errors --test snapshot_memory --test poison` JIT-only), clippy. **Step 5 (PC free):** the race kernels with the add cap at 2, 3 and 4; keep the best. **Commit.**

**Done when:** every stale-header test prints its expected values, `KESTREL_POISON=1` runs of the `headers_*` programs pass, and the dumps show the header loads out of the loops.

**Review Focus:** Review Focus 2 and 3; a header that sits in another list's buffer; the grow path's reload; a `for ... in` over a cached list whose body adds to another cached list.

### Task 4: Promoting `parts[i]` (spec decision 4)

**Files:**
- Modify: `kestrel/kir/src/bind_index.rs` (the run scan and the `Bind` insertion: one index local not written in the run, every place of the root through `R[i]`, no `Arg::Ref` of the root but `R[i]...`, no `Bind` of it, no closure, interface value or `par for`, no write to the header; the `Bind` carries the first access's check and line and goes where that access is its statement's first failure or effect), `kestrel/kir/src/promote.rs` (a region that only copies whole plain-struct values out is left in memory)
- Test: `kestrel/kir/tests/bind_index.rs` (the KIR of `step_index.kes`'s loop after `optimize`: one `Bind`, no `parts[i]` place left, the fields promoted; the cases that must not bind: `i` written in the run, `parts[j]` beside `parts[i]`, `parts` passed on, an `add` to `parts`), `kestrel/tests/programs/bind_index.kes` (the step on three particles with exact output), a failure test whose `parts[i]` is out of range with a print before it (same line, index and output as before), `kestrel/cli/tests` `--report` text for `step_index.kes` unchanged but for counts

- [ ] **Step 1: Failing tests.** **Step 2: See them fail.** **Step 3: Implement.** **Step 4: Green** (`cargo test -p kestrel-kir`, `--test programs --test errors --test report` JIT-only), clippy, **commit.**

**Done when:** `step_index.kes`'s loop has the same machine code shape as `step.kes`'s (`--dump bench`: no stack slot, four field loads, four stores, the data pointer once), and the failure test passes.

**Review Focus:** Review Focus 3; evaluation order of `parts[i].vel = f(parts[i].pos)`; the copy-only rule doesn't stop promotion of a region that also does arithmetic on the element.

### Task 5: Plain enums in registers (spec decision 5)

**Files:**
- Modify: `kestrel/kir/src/leaves.rs` (an enum's leaves: the tag, then each variant's plain leaves), `kestrel/kir/src/scalarize.rs` (which enum locals split: plain variants, no address taken, only `VariantLit` or split-local stores, only `Tag`, variant-field or split-local reads; a value `if`/`match` assigned to one is pushed into its arms first), `kestrel/kir/src/split.rs` (`VariantLit` stores, `Tag` reads, `Proj::Variant` places)
- Test: `kestrel/kir/tests/scalarize.rs` (heightmap's `Chunk.checksum` after `optimize` has no enum-typed addressed local; an enum passed to `checksum`, `to_bytes`, compared with `==`, stored in a list or passed `var` isn't split), `kestrel/tests/programs/enum_split.kes` (`block_at`/`weight` from heightmap on a small grid; an enum reassigned across variants in a loop; exact output), `codegen/tests` dump of heightmap's checksum: no `explicit_slot`

- [ ] **Step 1: Failing tests.** **Step 2: See them fail.** **Step 3: Implement.** **Step 4: Green** (`cargo test -p kestrel-kir`, `--test programs --test errors --test report` JIT-only), clippy, **commit.**

**Done when:** heightmap prints `columns: 262144 / tallest: 57 / checksum: 205794432` in JIT and both exes with no enum stack slot in its checksum loop.

**Review Focus:** no reader of whole enum bytes ever sees a split local; a `match` arm binding a variant's fields reads the right leaf locals.

### Task 6: Accumulator recursion as a loop (spec decision 6)

**Files:**
- Modify: `kestrel/kir/src/recurse.rs` (`run` acts only when `arith` is wrapping; the shapes `if c { base } else { f(a) op f(b) }` and `f(a) op x`, `op` in `+ * ^ & |` on integers, identities `0 1 0 -1 0`; the call order kept)
- Test: `kestrel/kir/tests/recurse.rs` (fib's KIR after `after_marking` in `--release` has one self-call inside a loop; in a checked build it is unchanged; a float `+` or a call to another function isn't rewritten), `kestrel/tests/programs/recurse_acc.kes` (fib 0-40, a `*` and an `^` accumulation, one that wraps in `--release`, a recursive function that prints, showing the print order is kept), and a `kestrel/tests/release/recurse_acc_release.kes` for the wrapping `--release` output

- [ ] **Step 1: Failing tests.** **Step 2: See them fail.** **Step 3: Implement.** **Step 4: Green** (`cargo test -p kestrel-kir -p kestrel-codegen`, `--test programs --test errors` JIT-only, plus the release harness), clippy, **commit.**

**Done when:** fib's `--release` dump has one `call` per loop iteration and every program prints the same in all builds.

**Review Focus:** Review Focus 1; range marks on the moved argument expressions are still valid (each keeps its guarding condition); `acc op base` at the exit.

### Task 7: Loop-invariant checks before the loop, the nest counter (spec decision 7)

**Files:**
- Modify: `kestrel/kir/src/hoist_checks.rs` (invariant remaining checks, in written order, until the first effect or a non-hoistable check that can fail; to the outermost loop each is invariant in; value into a new local; the at-least-once guard), `kestrel/kir/src/range/loops.rs` (the nest counter rule; a new file under `range/` if `loops.rs` passes 400 lines)
- Test: `kestrel/kir/tests/hoist_checks.rs` (a check after a `print` stays; a check after a non-invariant check that can fail stays; a loop that runs zero times doesn't fail), `kestrel/tests/programs/hoist_checks.kes` and failure tests with `// expect-stderr:` (an invariant overflow fails on the same line after the same output; zero-iteration loop prints normally), `kestrel/kir/tests` range report for a 34-cubed nest counter: `out[i]` and `i + 1` proven, `kestrel/codegen/tests` the frozen world's checked sample block has no `trap user`

- [ ] **Step 1: Failing tests.** **Step 2: See them fail.** **Step 3: Implement the counter rule, then the hoisting.** **Step 4: Green** (`cargo test -p kestrel-kir -p kestrel-codegen`, `--test programs --test errors --test report` JIT-only), clippy, **commit.**

**Done when:** `kestrel check --report` of the frozen world lists `out[i]` and `i + 1` as proven, the checked sample block has no check, and every failure test names the same line as before.

**Review Focus:** Review Focus 3; what counts as an observable effect (print, write, file, `time`, `args`, `env`, native calls); a `while` loop's guard.

### Task 8: Sort with keys computed once (spec decision 8)

**Files:**
- Modify: `kestrel/kir/src/lower/sort.rs` (`sort_by` with a number or bool key and `sort()` on numbers: a generated loop fills a scratch buffer with each key, the literal inlined, then `ListOp`/runtime call to the keyed sort; the old key-function path removed if nothing else uses it), `kestrel/runtime/src/sort.rs` (`kestrel_rt_sort_keyed_int`/`_float`: order bits, stable LSD radix sort with 11-bit digits and shared digits skipped, insertion sort below 64, one permutation through an uninitialised buffer)
- Test: runtime unit tests (the keyed sorts against `slice::sort_by` with `float_order` over random, all-equal, sorted, reversed, NaN-holding, `-0.0`/`0.0`, infinity and `MIN`/`MAX` inputs, sizes 0-1,000 and 100,000), `kestrel/tests/programs/sort_keyed.kes` (exact orders of items with equal keys, NaN and `-0.0` keys, int keys at `MIN`/`MAX`), the existing sort programs unchanged

- [ ] **Step 1: Failing tests.** **Step 2: See them fail.** **Step 3: Implement.** **Step 4: Green** (`cargo test -p kestrel-runtime -p kestrel-kir`, `--test programs --test errors` JIT-only), clippy, **commit.**

**Done when:** every existing sort test passes unchanged and the keyed sort equals the standard library's stable sort on every input tested.

**Review Focus:** Review Focus 4; the NaN and `-0.0` mapping matches `float_order`; the key runs once per item, in item order (a key that prints shows it).

### Task 9: Xor, and, or chains regrouped (spec decision 9)

**Files:**
- Modify: `kestrel/kir/src/reassoc.rs` (in a loop, a chain of one of `^ & |` on integers: operands the loop never writes grouped into one subexpression, kept in written order inside each group; `hoist.rs`'s `invariant` reused)
- Test: `kestrel/kir/tests/reassoc.rs` (`x*A ^ y*B ^ z*C ^ s*D` in an x loop becomes `x*A ^ (...)`; `+` and floats untouched), `kestrel/tests/programs/reassoc.kes` (the voxel hash over a small grid, exact output), `kestrel/codegen/tests` the frozen world's `--release` sample block has fewer `xor`s than Task 0's before-dump

- [ ] **Step 1: Failing tests.** **Step 2: See them fail.** **Step 3: Implement.** **Step 4: Green** (`cargo test -p kestrel-kir -p kestrel-codegen`, `--test programs --test errors` JIT-only), clippy, **commit.**

**Done when:** the frozen world prints 463,749 faces in all builds and its sample block's xor count is at most the 84 it had on Cranelift 0.116 (`bench/voxels3d/results.md:316`).

**Review Focus:** Review Focus 1; mixed chains (`a ^ b & c`) are left alone.

### Task 10: Measurements, docs, full suite (the controller)

**Files:**
- Modify: `kestrel/bench/kernels/results.md` (a new top section "The performance round (2026-11-xx)": the spec's Goals table with every case's old, new, Rust and ratio, met or missed, median, range and load), `kestrel/bench/2a/results.md`, `kestrel/bench/heightmap/results.md`, `kestrel/bench/voxels3d/results.md` (short sections pointing to it), `docs/superpowers/specs/2026-11-01-perf-round-design.md` ("As built"), `docs/superpowers/specs/2026-10-26-list-costs-design.md` (status: parts 1-2 built by this round), `docs/superpowers/plans/2026-10-09-core-2a-followups.md` ("Performance" and "Found while building scalar replacement" updated; entries 38-39 closed; new entries for what missed), `docs/superpowers/plans/2026-10-10-handoff.md`, `CLAUDE.md`
- Test: `cargo test --workspace --no-fail-fast` (the exe pass included), `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 1: Full suite and clippy.** **Step 2 (PC free, no game):** build the old compiler (the commit before Task 0) in `git worktree` `C:\kt\perf-old`; run, old and new in turn within every round: `bench/2a/run.ps1` (and its copy pointed at the old `kestrel.exe`), `bench/heightmap/run.ps1`, `bench/kernels` `py measure.py --rounds 9 --threads 1 --old DIR` (every kernel, the race included), the frozen and tuned world with the Rust port, the engine's generation and `bench.kes` p99, compile time of `voxel/main.kes` and `bench/voxels3d/main.kes` (9 runs each); sample the load before, during and after. **Step 3:** the determinism checks (results hash, Pong references, `det_*` and rollback programs in all three builds). **Step 4: Docs. Commit and push.**

**Done when:** every row of the Goals table has a measured ratio, each miss has its cause written down, the determinism checks are byte-identical, compile time is within 5%, and the suite and clippy are green.

**Review Focus:** the numbers are release builds on a free PC, old against new taking turns; a difference under 10% on a sub-nanosecond kernel survives one unused function added before it is called a result.
