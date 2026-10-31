# Scalar replacement design (sub-project 5, part 1)

Status: decided. On 2026-10-11 the user approved the spike's pick ("go with ur pick") and this design ("Yes"): stay on Cranelift with one backend, keep the JIT and hot reload, and keep small structs and the fields of `var` places in registers. Everything below that isn't that pick is Claude's rulings, listed at the end. There is no syntax change, and no program's output may change.

## Why

The spike (2026-10-11, throwaway code in a scratch folder) asked why Kestrel's one-thread particle step takes 3.4x Rust's time. It timed the step in-process (an exported `bench(n, iters)` function called through its thunk, 200,000 particles, `--release`, `pow` hoisted, 3 interleaved rounds):

| Variant | ns per particle |
|---|---:|
| Kestrel, the demo's step | 12.81 |
| Kestrel, the same step written with float locals (`px`, `py`, `vx`, `vy`), `self` read once and written once | 4.34 |
| Kestrel, float locals for the intermediate values, but `self`'s fields read and written in memory each line | 5.50 |
| Rust | 3.74 |
| Rust without vectorizing | 4.28 |
| Rust with every `vec2` temporary stored to a stack slot and read back | 13.17 |
| Kestrel with checks on | 12.83 |

The machine code shows why. Every struct value lives in memory, including a `vec2`:
- KIR lowering spills each vector operand that isn't a place into an `Addressed` temporary (`kir/src/lower/vector.rs`, `operand`), and every struct local is `Addressed` (`kir/src/lower/mod.rs`).
- Codegen gives each one a stack slot, and its `assign` builds any struct value that isn't a `Load` in another stack slot before copying it (`codegen/src/func/place.rs`).

The demo's step stores to and reloads from eight stack slots per particle, about 13 store-then-load waits in one dependency chain. Cranelift doesn't remove them: its alias analysis forwards only from the latest store in each memory category, so a store to `.y` makes it forget the store to `.x`.

The 9.1 ns gap splits into:
- 81% for struct temporaries in memory;
- 13% for `self`'s fields re-read from memory after each write;
- 7% for Rust's vectorizing, which only pairs `x` and `y` in one register, because the bounce branches stop it from processing several particles at once.

On 16 threads the hand-written float version takes a 200,000-particle step from 0.35 ms to 0.15 ms. With `pow` called for every particle (the demo as written) Kestrel takes 26.13 ns against Rust's 15.24; with float locals it takes 17.45. `pow` costs about 1.7 ns more per call than in Rust, because it goes through `kestrel_rt_math`, which picks the operation with a `match`.

`bench/2a/results.md` already names the same cause for its `particles` (1.26x Rust, while C keeps the particle in registers: 143 ms against Kestrel's 459) and `swarm` (1.4-1.6x: "Cranelift reloads a field right after storing to it").

## What changes

Two new KIR passes run after inlining, and math calls change in codegen and the runtime:

```
lower → inline_small → scalarize → promote → codegen
```

`kestrel_kir::optimize(&mut Module)` runs the three passes in that order. The driver (which the Godot add-on uses too) and codegen's test helpers (`codegen/tests/common/mod.rs`, `codegen/tests/natives.rs`) call it in place of `inline_small`.

Both passes only rewrite KIR. They produce scalar locals of kind `Value`, which codegen already turns into Cranelift variables (registers). Codegen itself changes only for math calls (part C) and a debugging hook (see Testing).

### Plain structs

A **plain struct** is a struct whose fields are numbers or bools (`Bool`, `U8`, `I32`, `I64`, `F32`, `F64`) or plain structs, with at most 16 **leaves** (the scalar fields at the bottom, counted through nested structs). `vec2`, `vec3`, `Particle` (two `vec2`s), `Color` and a `Camera` (10 leaves) are plain; anything holding a string, array, list, map, arena, GPU handle, enum or optional is not. A shared helper (`kir/src/leaves.rs`) gives a plain type's leaves: each one's path of `Proj::Field` steps and its scalar type. A scalar type counts as a plain type with one leaf and an empty path.

### A. `scalarize` (`kir/src/scalarize.rs`)

**Which locals.** A local is split when its type is plain, it isn't a parameter, and nothing needs its address after inlining. A local needs an address when a place rooted at it is:
- an `Arg::Ref` (a `var` argument), in a call or in `Stmt::ParFor`'s arguments;
- the place of a `Stmt::Bind`.

The decision scans the uses; it doesn't trust `LocalKind`. A local the checker marked `addressed` because it was passed as `var` is split if inlining removed every such use. That is what happens to `bench/2a`'s `var p` once `p.step(dt)` is inlined. A **scalar** local of kind `Addressed` with no remaining use that needs its address becomes `Value`; it is not split, as it has one leaf.

**Splitting.** A split local gets one new `Value` local per leaf, named `<local>.<field path>` (for example `tmp10.x`, `p.vel.y`), and the original is no longer used. Every place rooted at it is rewritten:
- A `Load` of a leaf becomes a `Load` of the leaf's local.
- A `Load` of the whole value, or of a nested struct field, becomes a `StructLit` tree of leaf loads. Every consumer of a struct value accepts a `StructLit`: codegen builds it in place (a call argument, a `Return` through the result pointer, a list element, a printed value). The plan checks each consumer.
- A `Let` or `Assign` of a scalar leaf becomes a `Let` or `Assign` of the leaf's local.
- A `Let` or `Assign` of a whole value or a nested struct field is a **struct store** (below).

**Struct stores.** A store of a struct-valued expression `e` to destination `d` is rewritten when `d` is (part of) a split local, whatever `e` is, and when `d` is memory and `e` isn't a plain `Load` of a place (a copy between two memory places stays one aggregate copy). The second case covers stores like `Assign(self.vel, StructLit(...))`, so codegen never builds a temporary for them:
1. `e` is broken into statements to run first and one scalar expression per leaf (`split`, below).
2. Each leaf's expression is stored in a fresh `Value` temporary, in leaf order. That is the order codegen stores fields in today, so side effects and panics happen in the same order.
3. Then each temporary is stored to the matching leaf of `d`.

All the leaves are computed before any is stored, so a value that reads its own destination (`v = v.yx`, `p.vel = p.vel.yx`) stays right. Cranelift removes the extra temporaries. (As built, step 3 applies only to a split local; a memory destination gets one `Assign` of a `StructLit` of the leaf temporaries. See "As built".)

**`split(e)`** gives the statements to run first and the leaf expressions of a plain-typed expression:
- `StructLit`: each field in order. A plain-struct field is split recursively.
- `Load(place)`: the leaf loads of `place`. An index expression inside the place's path (`xs[f()].pos`) is first stored in a `Value` temporary, so it runs once. This mirrors `rereadable` in `kir/src/lower/vector.rs`.
- `FieldOf(base, i)`: `split(base)`, then the leaves under field `i`. The other fields' expressions still run (they are in temporaries).
- `Seq(stmts, v)`: the statements, then `split(v)`.
- `If { cond, then_body, then_val, else_body, else_val }`:
  - One fresh `Value` local per leaf, each given a zero first (`Let`).
  - Then a `Stmt::If` whose branches end by assigning `split(then_val)` and `split(else_val)` to those locals. A branch without a value (it never finishes) assigns nothing.
  - The leaves are loads of those locals.
- Anything else (a `Call`, or any other struct-valued kind): stored first in an `Addressed` temporary, which is not split, and read back by leaf. A call's result therefore still goes through memory. That costs little, because the hot calls are inlined.

`scalarize` works on every function, and on statements nested inside expressions (`Seq`, `If`).

### B. `promote` (`kir/src/promote.rs`)

`promote` keeps the fields of an **exclusive place** in `Value` locals for a **region**, so one memory location isn't read again after each write to it. There are two kinds:
- **A pointer parameter** (`LocalKind::RefParam` among `Func::params`) whose type is plain: a `var` parameter (`var self` included) or a read-only struct parameter. Its region is the function body.
- **A loop element** (a local set by `Stmt::Bind`: a `for ... in` element, or a `par for` item in a `par for` body), whose type is plain. Its region is the rest of the statement list after the `Bind`, normally the rest of one loop iteration. It ends early at the next `Bind` of the same local.

**Why it is safe.** Only one route reaches the place's memory during the region:
- For a `var` parameter, the checker refuses to let the argument's place overlap any other argument of the same call (`check_exclusive` in `check/src/body/var_clash.rs`), and Kestrel has no global variables that can change (heap constants are read-only). Since 2026-10-31 places are compared by field path, not by variable, so `step(var g.a, g.b)` passes and its two parameters still never share memory.
- For a read-only struct parameter, nothing can change the memory during the call: no argument of the call may be a `var` use of an overlapping place.
- For a loop element, the conditions below rule out any other route.

**When a place is not promoted (the conservative skips).** A pointer parameter is skipped if, in its region, a place rooted at it is:
- an `Arg::Ref` (passed as `var` on to a call);
- a `Stmt::ParFor` argument;
- the place of a `Bind`.

A loop element `R` bound to a place rooted at `C` is skipped:
- for the same reasons, about places rooted at `R`;
- if the region mentions `C`, or any local bound to a place rooted at `C`, or (when `C` is itself a loop element) the same, following the chain to the local the memory belongs to.

That skips a body that reads the list it loops over (`for p in ps { ... ps[0] ... }`), a nested loop over the same list, and a body that changes the list.

**The rewrite.**
1. At the start of the region: `Let(leaf_local, Load(place.leaf))` for every leaf the region reads or writes.
2. In the region:
   - Every leaf load becomes a load of the leaf's local, and every leaf store a store to it.
   - A whole-value read becomes a `StructLit` of the leaf locals.
   - A struct store goes through `scalarize`'s struct-store rule. `scalarize` has already run, so only a plain `Load` copy from another place can remain, and it is split leaf by leaf.
3. Writing back (only leaves stored somewhere in the region, the **dirty** leaves; a read-only parameter is never written back): `Assign(place.leaf, Load(leaf_local))` for each dirty leaf is inserted
   - at the end of the region's statements, when control can reach it;
   - before every `Continue` and `Break` that leave the region's loop (not those in the body of a loop nested inside the region; a `break` or `continue` in a nested loop's condition or bounds runs before that loop starts, so it leaves the region's loop and does get the write-back);
   - before every `Return`, anywhere in the region, including in statements nested inside expressions.

   A `Return(Some(e))` whose `e` stores to a dirty leaf (only possible through a `Seq` or `If` inside it) first stores `e` in a temporary, then writes back, then returns the temporary.

**Loading at the start** is always safe: the memory behind a pointer parameter is valid for the whole call, and a loop element's address was computed, and bounds-checked if needed, by its `Bind`.

**A panic** in the middle of a region loses the leaves not yet written back. Nothing can observe that: a runtime panic ends the process (exit 101), a panic on a `par for` thread is handed to the caller, which then panics, and the Godot add-on parks the worker and throws the failed call's memory away (`godot/src/worker.rs`).

### C. Math calls

`kestrel_rt_math(op, a, b)` is replaced by one runtime function per operation: `kestrel_rt_sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `exp`, `log` and `pow`. The one-argument functions take one `f64`; `atan2` and `pow` take two. Codegen calls the one it needs (`codegen/src/func/math.rs`, `runtime_math`). The maths stays in `f64` with the same Rust calls, so every result is bit-identical. The new symbols join the JIT's symbol table (`codegen/src/rt.rs`) and the runtime's exports. `kestrel_rt_math` goes away once nothing calls it.

## Testing

**Program tests** (`kestrel/tests/programs/`), each with `// expect:` lines worked out by hand:
- `scalarize_swizzle.kes`: `v = v.yx`, `w = w.zyx`, `p.vel = p.vel.yx` on a struct field, and a whole-struct copy between locals followed by changing one of them.
- `scalarize_if.kes`: a struct-valued `if` (including `normalized()` of a zero and a non-zero vector), a nested plain struct built in branches, and a struct temporary whose index expression has a side effect (it runs once).
- `scalarize_calls.kes`: split locals passed to read-only struct parameters, returned from functions, printed, interpolated, added to a list, and passed as `var` (which keeps the local in memory).
- `promote_var_self.kes`: a `var self` method too big to inline (over 256 KIR nodes), with an early `return` after changing fields, a `for` loop inside it with `break` and `continue`, and a `var` scalar parameter.
- `promote_loops.kes`:
  - `for p in ps` changing `p`, with `continue` after a change and a `break` out of an inner loop;
  - a body that reads `ps[0]` while changing `p` (must not be promoted, and must print the aliased values);
  - nested loops over the same list;
  - a `par for` over plain items.
- `promote_panic.kes`: an index panic in the middle of a promoted loop (`// expect-exit: 101`, the same message as today).

**KIR unit tests** (`kestrel/kir/tests/scalarize.rs`, `promote.rs`):
- which locals are split, and which are kept and why;
- the struct-store order;
- write-back at each kind of exit;
- every skip condition.

**A codegen test:** after `optimize`, the particle demo's step in a `par for` body compiles with no stack slots. It reads the optimised CLIF through a new `#[doc(hidden)] pub fn kestrel_codegen::dump(module, opts, name) -> Result<String, String>`, which gives a function's CLIF after Cranelift's optimisation and its machine code. The benchmark harness uses it too.

**Existing tests:** everything stays green.
- The 529 workspace tests (`cargo test --workspace --no-fail-fast`).
- The Godot headless test (30 checks).
- Clippy in both workspaces.

## Measuring

**The harness:** the spike's harness moves into `kestrel/bench/kernels/`, its own Cargo workspace excluded from the main one, like `bench/particles/rust`.
- A Rust binary compiles a `.kes` file with `kestrel_driver`, JIT-compiles it, and times its exported `bench(n: int, iters: int) -> float` (iterations minus a zero-iteration call, the median of reps).
- `--dump <function>` prints `kestrel_codegen::dump`.
- A Rust reference sits beside it, with `results.md`.
- Programs:
  - `step.kes`: the demo's step with `pow` hoisted;
  - `step_pow.kes`: the demo as written;
  - `step_scalar.kes`: the hand-written float version, the ceiling to match;
  - `copy.kes`: the demo's draw copy.

**Before and after, reported in `results.md`, regressions included:**
- the step, on one thread and on 16;
- `bench/2a` (all kernels, especially `particles` and `swarm`);
- `bench/voxels3d`, `bench/heightmap` and `bench/maps`;
- the particles demo's update at 200,000, in a hidden window (`KESTREL_HIDDEN=1`; headless mode's clock is fixed, so it can't time anything), never a visible one;
- compile time of the voxel viewer.

**Targets:**
- `step.kes` within 1.2x of Rust (about 4.5 ns on this machine) and within 5% of `step_scalar.kes`.
- `step_pow.kes` within 1.15x of Rust's per-particle `pow` version.
- No other benchmark slower beyond this machine's noise. A slowdown is investigated before the work is called done.

## Not in this design

Follow-ups for later sub-project 5 work:
- **Vectorizing** (Rust's last 7% here; the SIMD half of sub-project 3).
- **Bounds checks and list-header reloads.** A plain `for i` loop over `parts[i]` adds about 1 ns per particle (8 checks, with the list's length and data pointer re-read for each), and the copy loop takes 0.88 ns per element against Rust's 0.19.
  - Tagging list elements and list headers as separate memory for Cranelift has a trap: a list's header can itself be an element of another list, so a careless split would let Cranelift reuse a stale header.
  - Promoting an inlined `var self` call on a checked indexed place (`parts[i].step()` in a plain `for i`) belongs with this work, because its bounds check would move.
- **Range analysis for overflow checks** (the 3D world's 2.4x cost with checks on).

## Rulings made on the user's behalf

What each costs if it is wrong:
- **KIR passes**, not struct values held in codegen (as the Rust compiler's Cranelift backend does) and not a stack-slot-to-register pass over Cranelift IR. Cost: a whole-struct value at a call boundary still goes through memory.
- **Plain structs have at most 16 leaves.** Cost: a bigger plain struct stays in memory.
- **Splitting is decided by scanning uses after inlining,** not by `LocalKind`. Cost: none known.
- **Every struct store computes all leaves before storing any.** Cost: none after Cranelift's optimisation; KIR has more temporaries.
- **Promotion covers pointer parameters and loop elements only,** and skips any region with another route to the memory. Cost: some places that could be promoted aren't.
- **Fields are loaded at the start of the region,** even ones a path through it doesn't read. Cost: a few loads per region that may be unused.
- **A panic loses unwritten fields.** Cost: none observable today. A future way to recover from a panic and keep the program's memory would have to write back first.
- **Math calls keep `f64`.** Cost: no `f32` speed-up from `powf` (which would change results).
- **`kestrel_kir::optimize`** is the one entry point. Cost: none.
- **The harness is permanent** (`bench/kernels`), with `kestrel_codegen::dump` as a hidden public function. Cost: one more benchmark workspace to keep building.

## As built (2026-10-11)

Built in six tasks (commits f329f57..c4fac4a for the first five, then Task 6 below, 6af9575 and its review fix 1cb9ce5; run subagent-driven; `scalarize` and `promote` were each reviewed and took one fix round, and the final whole-branch review found one more `promote` bug, below). The pipeline is `lower → inline_small → scalarize → promote → codegen` behind `kestrel_kir::optimize`, the math calls are one runtime function each, and `bench/kernels` and `kestrel_codegen::dump` are in. 554 workspace tests pass (the 529 of the design plus 25 new), the Godot headless test passes with its 30 checks (run again by the final review, which rebuilt the add-on, at 1cb9ce5: 30 ok, 0 failures; not run after the loop-header fix below), and clippy is clean in `kestrel/` and `kestrel/godot`. Where the build differs from the text above:

- **`split`'s `FieldOf` arm only applies to a plain-struct base.** The design's `FieldOf(base, i)` rule calls `split(base)`, which assumes the base is plain. A field taken from a value that isn't a place and isn't plain, like `make(3).pos` where the struct also holds a string, reached that rule and crashed the compiler (the review found it). The arm is now guarded by `is_plain_struct(base.ty)`: any other base goes the "anything else" route (the value is stored in a temporary in memory and the field read from there). Program test `scalarize_field_of.kes`.
- **`scalarize` rewrites every function,** not only those that have a local to split. The design says it works on every function, and that a struct store to memory from anything but a plain `Load` is rewritten whatever the function holds (`Assign(self.vel, StructLit(...))` is the case); the first code skipped functions without a split local, and the review had it removed (commit b5d7c4d).
- **A loop element isn't promoted when its region can return after freeing memory.** The design wrote the write-back before every `Return`. But lowering emits a `return`'s own cleanup (`PopArena { free: true }`, `FreeArena`, drop calls) before the `Return` statement, so the write-back ran after the cleanup, and for a loop element living in an arena that the `return` frees, it wrote into freed memory. The review reproduced this as a segfault (a 45-million-element array inside `arena { }`, returning from a loop over it). The fix: a loop-element region that contains a `Return` and also anything that can free memory is left alone. "Anything that can free memory" is `PopArena { free: true }`, `FreeArena`, `DropGpu`, or a call with a `var` (`Arg::Ref`) argument, which is how lowering's generated drop calls look. Putting the write-back before the cleanup instead would depend on recognising lowering's exit sequence (the spill, the unlocks, the drops inside `If`s and calls), which breaks quietly when lowering changes. Pointer parameters are unaffected, because their memory is the caller's and no `return` of this function frees it. Cost: a loop that returns and also frees memory (or calls anything that takes a `var`) keeps its element in memory. The check is broad: a call with a `var` argument anywhere in a returning loop turns promotion off, and so does a scratch-arena pop. A map-value loop with a `return` never promotes either, because its `MapUnlock` counts as a second route to the memory; that was true before this fix. Program test `promote_exits.kes`, and KIR tests for the skip.
- **`promote`'s generated loads and write-backs carry source line 0.** They read and write memory the `Bind` or the caller's pointer already supplied, so they can't raise a runtime error of their own, and they have no source line to name.
- **A `break` or `continue` in the header of a loop inside the region's loop gets the write-back** (found and fixed by the final whole-branch review; it was a silent miscompile). A loop's header (a `while`'s condition, a `for`'s `start` and `end`) runs before the loop starts, so a `break` there leaves the region's loop. `Exits` raised its loop depth before walking the whole inner loop, header included, so such a `break` was taken for the inner loop's own and got no write-back: the element's changes were lost (`while (if go { go = false; true } else { break }) { }` after `p.x += 10` left `p.x` unchanged). Now only the inner loop's body is one level deeper. A `break` in the body of the inner loop still gets none. Four cases at the end of `promote_exits.kes` and three KIR tests in `kir/tests/promote.rs`.
- **Extra program tests:** `scalarize_payloads.kes` (split `vec2` values stored in enum payloads and struct fields), `scalarize_field_of.kes` and `promote_exits.kes` (above), `scalarize_store_order.kes` (Task 6, below), besides the design's six (`scalarize_swizzle`, `scalarize_if`, `scalarize_calls`, `promote_var_self`, `promote_loops`, `promote_panic`). KIR unit tests are in `kir/tests/scalarize.rs` and `promote.rs`, and the codegen stack-slot test is in `codegen/tests/dump.rs`.
- **Math** (Task 4) is as designed. `kestrel_rt_math` is gone; four older documents under `docs/superpowers/` still name it (they are history and were left as written).

**Measured** (the details are in `kestrel/bench/kernels/results.md`; old and new compilers were run alternately because the machine ran hot and results moved by up to 30% with where the code landed):

| | Before | After |
|---|---:|---:|
| `step.kes`, one thread | 14.46 ns | 4.88 ns (1.15x Rust's 4.25; +2% on `step_scalar.kes`) |
| `step.kes`, 16 threads | 2.10 ns | 0.85 ns |
| `step_pow.kes`, one thread | 27.62 ns | 19.12 ns (1.07x Rust's 17.84) |
| `step_pow.kes`, 16 threads | 3.84 ns | 2.59 ns |
| `copy.kes`, one thread | 0.80 ns | 1.11 ns at 04358c9 (Rust 0.23); **fixed by Task 6**: the pre-change time within noise (paired ratio 1.01, later and on a loaded machine, so no absolute figure) |
| `bench/2a` particles, `--release` exe | 354 ms | 202 ms (checked exe: 351 to 163) |
| `bench/2a` swarm, `--release` exe | 248 ms | 178 ms |
| particles demo update, 200,000, 16 threads / one | 1.33 / 6.23 ms | 0.95 / 4.29 ms |

The three targets for the step were met (within 1.2x of Rust, within 5% of `step_scalar.kes`, `step_pow` within 1.15x of Rust). `fib`, `loop`, `collatz`, `mandelbrot`, `sieve`, `heightmap`, `voxels3d` and the viewer's compile time did not move beyond noise (`check` is about 1 ms slower; `maps` was too noisy to read). **One target was not met at first: the draw copy got slower, and Task 6 fixed it.**
- **Found:** at 04358c9 `copy.kes` took 0.3 ns an element more than before (0.80 to 1.11 ns, every round agreeing).
- **Cause:** the combination of the two passes. `promote` keeps `p.pos` in registers, so `positions[i] = p.pos` becomes a store of a `StructLit` of locals. The struct-store rule then wrote it as two field stores, each of which codegen turned into its own place computation: its own bounds check on `positions[i]` and its own reload of the list's length and data pointer. Before, the store was one aggregate copy from place to place with one check.
- **Fix (Task 6):** a plain struct value stored to memory is now ONE `Assign` of a `StructLit`. `split` computes every leaf first, into constants and `Value` locals, so the `StructLit` reads no memory, and `dest`'s index expressions still run after the value. Codegen's `assign` stores a `StructLit` that reads only locals and constants without a temporary: it reads the values first (nested structs flattened, in field order), then finds the place once, then stores each value. (The first version found the place before reading the values; the review caught that a destination's index expression can change a local the value reads, for a struct with more than 16 leaves, which `split` never touches. Program test `scalarize_store_order.kes`.) A call's result or other value built in memory is read leaf by leaf into locals as well, so a store of it is built from locals too. A store into a split local is still one assignment per leaf. Tests: `a_struct_store_into_a_list_checks_the_index_once` (codegen; the IR has one `trap user2` path, it had two), `struct_stores_to_memory_read_every_field_before_storing`, `a_call_result_stored_into_a_list_is_built_from_locals` (KIR) and `scalarize_store_order.kes`.
- **A deviation from the design's struct-store rule,** besides the `FieldOf` guard and `scalarize` running on every function above: the design's step 3 stored each temporary to its leaf of `d` for every destination, and memory destinations are no longer split per leaf. Steps 1 and 2 are unchanged, which is what keeps `v = v.yx` and `ws[noisy(1)] = vs[noisy(2)].yx` right.
- **Result:** `copy.kes` takes 28% less time than at 04358c9 and, round for round, the same time as the compiler before scalar replacement (paired ratio 1.01, range 0.77-1.27, 22 rounds on a machine loaded by another program; `step.kes`, `step_pow.kes` and `step_scalar.kes` unchanged at 0.98-0.99). The loop is the same work per element as before scalar replacement. The details are in `results.md`, "After the one-check struct store".
- **Still open:** the loop is about 4 times Rust's time (1.27 ns against Rust's 0.31 in that run), the bounds-check and list-header-reload item in "Not in this design".
