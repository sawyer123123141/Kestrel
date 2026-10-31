# Range analysis: removing overflow and bounds checks, and `kestrel check --report`

Status: built 2026-10-19 and 2026-10-20 (see "As built" at the end); designed 2026-10-19; revised the same day after an Opus review (four Critical fixes, all folded in below) and the controller's rulings. This is Round A of the work the handoff's Next names ("range analysis with `@prove`"). Round B builds `@prove(...)` and `@assume(...)` on top of it (`2026-10-18-at-annotations-design.md`, whose syntax the user decided). The `@` spec says the report comes first: "`kestrel check --report`: built first, before contracts fail anything ... Contracts are then the same analysis as a build failure." Everything below that isn't the `@` spec's is Claude's, listed at the end with what each ruling costs if it is wrong.

## Why

Kestrel checks every integer `+ - *`, every `xs[i]`, every `/` and `%`, every shift and every narrowing conversion (`int(x)` of an `i64`) at run time, the way GDScript checks `arr[i]` and stops with "Invalid index". That is what makes Kestrel safe, and it costs:

- **The 3D voxel world takes 2.4 times as long with checks on** as with `--release` (1.2 s against 0.49, `kestrel/bench/voxels3d/results.md`). Each noise lookup does five checked 64-bit multiplies, checked adds, shift checks, a narrowing check and bounds checks, and each check is a branch in the hottest loop.
- **A plain `for i` loop over `parts[i]` pays 8 bounds checks per particle,** each re-reading the list's length (`docs/superpowers/plans/2026-10-09-core-2a-followups.md`, "Bounds checks and list-header reloads"). The draw-copy kernel is about 4 times Rust's time partly for this reason (`kestrel/bench/kernels/results.md`).

Most of these checks can't fail. `h & 0x7FFFFFFF` times a 31-bit constant can't overflow an `i64`; in `for i from 0 to xs.len`, `xs[i]` is always inside `xs`. This round teaches the compiler to prove that and leave such checks out, in every program, not only the voxel world.

It also adds **`kestrel check --report`**, which lists every check and allocation left in each function and loop, with its line and why it stayed. In Godot terms: the profiler shows where time went after the game ran; the report shows, before anything runs, what the compiler couldn't make free. Round B's `@prove(no_checks)` is the same list turned into a build error.

## Goals

- Remove overflow, shift, division and narrowing checks whose operands' ranges show they can't fail, and bounds checks whose index is proven inside its list or array.
- Do it in both builds: with checks on, and with `--release`, which keeps bounds and division-by-zero checks.
- **Never remove a check that could fail.** A removed check that could have fired is a Critical bug. When the analysis isn't sure, the check stays.
- `kestrel check --report`, listing checks, allocations, other ways to fail and calls that weren't inlined, with reasons, following calls, in the shape Round B's contracts need.
- No noticeable compile time.

## Not in this round

- `@prove` and `@assume` (Round B; the hooks it needs are under "For Round B").
- **Integer fields in memory as terms.** Facts are about locals and lengths only. A field `promote` keeps in a register is a local, so `var self` methods still get facts about their fields; a field read from memory (a non-promoted struct, a loop element of an integer list) has its type's range. None of this round's named wins needs more, and fields carry the hardest aliasing (views, `promote`'s write-backs, enum variants sharing bytes).
- Moving a check out of a loop (hoisting a loop-invariant check with its value), or merging several checks into one.
- Facts across calls: a function's result range, or the length of a list it returns. (`mesh` in the voxel world can't know that `solids` returns 39,304 items, so `solid[i + 1]` keeps its check. Round B's `@assume` covers that.)
- Products in facts: `y * w + x < xs.len` from `y < h`, `x < w` and `xs.len == w * h`.
- The list-header reload: with its bounds check gone, `xs[i]` no longer reads the length, but it still re-reads the data pointer after a store, because Cranelift can't prove the store didn't change it. Promoting the fields of `parts[i]` into registers (scalar replacement of an indexed place) also waits.
- Checks inside the runtime's own functions (`remove_at`, `insert`, a string's `s[i]`) and the map-changed check of a loop over a map: the report lists them; they stay.

## What changes for a program

- **Programs run faster with checks on,** and a little faster with `--release` (bounds and division-by-zero checks).
- **One new limit: a list or an array holds at most 2,147,483,647 items** (`i32::MAX`, like C# arrays). `xs.len` is an `int`, so a longer list already failed at its first `xs.len` in a checked build and gave a wrong length in `--release`. Now growing past the limit fails where it happens: "a list can't hold more than 2,147,483,647 items" (from `add`, `insert`, every built-in that builds a list, and a Godot array passed in through the add-on as a list; one passed as a `[T]` says "an array can't hold more than 2,147,483,647 items"), and "an array can't hold more than 2,147,483,647 items; this one would have 3000000000 on line 12 of world.kes" (from `array[T](n)`). With the limit, every `xs.len` of a list or array fits an `int`, so the conversion check in each `xs.len` (once per `for x in xs`, and once per iteration of `while i < xs.len`) goes away too. Strings aren't limited; their `.len` keeps its check.
- **Nothing else a program does changes:** the same output, the same errors on the same lines. A check either stays or provably never fires.

## The report

```
kestrel check --report world.kes
kestrel check --report --release world.kes
```

`--report` goes only with `check` ("`--report` goes with `check`: kestrel check --report file.kes"). The report goes to standard output; diagnostics still go to standard error, and a program with errors gets no report (exit 1). Without `--release` it reports a checked build; with it, a `--release` build, which keeps only bounds and division-by-zero checks and wraps on overflow.

An illustration (the exact text a test pins is in the plan, Task 4):

```
world.kes
  function `hash` (line 10): no checks left (14 removed), no allocations
  function `octave` (line 31): its own copy is never called; its checks are listed where it is inlined
  function `solids` (line 50): 5 checks left (312 removed), 1 allocation
    line 51: allocation: `array[u8](...)` makes a new array
    loop at line 53
      loop at line 54
        loop at line 55
          line 56: overflow check: `cx * 32` may exceed `int` (`cx` can be any `int`)
          line 41 (in `density`): overflow check: `seed + 1` may exceed `int` (`seed` can be any `int`)
          line 58: bounds check: `out[i]` may be outside `out` (`i` is 0 or more; nothing shows it is below `out.len`)
          line 60: overflow check: `i + 1` may exceed `int` (`i` is 0 or more)
total: 1,234 checks, 1,100 removed, 134 left; 12 allocations; 3 other ways to fail
```

- **One line per item,** grouped by file, then function (in source order), then loop (nested, by the line of the loop's header). Items and loops inside a group are in line order; items on one line are in the order the code runs them. A `par for` body appears under its function as "par for at line N", with its own items.
- **An item's line is where the source is.** An item from an inlined function has that function's line and says so ("line 41 (in `density`)"), found from each function's first and last lines; a line in another file is written `geom.kes:41`.
- **A function whose own compiled copy is never called** (nothing calls it, takes it as a value, puts it in a method table or calls it from a host, so every call to it was inlined) is one line; its checks are listed where it was inlined.
- **Kinds of item:**
  - `overflow check`, `shift check`, `division check` (zero, or the minimum divided by -1), `narrowing check`, `bounds check`: each with the expression and why it may fail, with the ranges the analysis found for each operand that isn't a constant.
  - `allocation`: with what allocates. The runtime's own count, as the `@` spec's `no_alloc` requires: `sort` and `sort_by` (their index buffer), a `par for` (an arena per thread), list and map growth, string building, copies of lists, maps and strings, stored closures, boxed interface values, `arena { }` (a new arena) and `in x { }` (the first time). A generated function (the loop behind `filter`, `map`, `sort_by` and the like) is named by the built-in it implements.
  - `can fail`: other runtime errors, for `no_panic`: a runtime index (`remove_at`, `insert`, `s[i]`, `slice`), `split` with an empty separator, a list growing past the item limit (`add`, `insert`, and the built-ins that build lists), a map replaced during a loop over it, `array[T](n)` with a bad length, a native function, a call through a function value or an interface.
  - `call`: a call that wasn't inlined, and why, as the inliner recorded it when it decided ("it calls itself", "it is 912 nodes; the limit is 256", "it has an early `return`", "the inlining budget of `bench` ran out", "inlined calls nest more than 8 deep"), for `inline`.
- **Calls are followed.** A call to a function that allocates, can fail or has checks left is an item of the caller, with the chain to the first reason: "allocation: calls `spawn` (line 40), which adds to `ps` (line 12) and may grow the list". A call through a function value or an interface counts as "may allocate, may fail": "calls the function value `f` (line 8); its body isn't known here". A native function counts as "may allocate, may fail" too (no native is marked safe yet).
- **Wording:** plain sentences. Expressions are printed in Kestrel's syntax from KIR (local names, `xs.len`, `xs[i]`, `int(x)`); a value the compiler made with no name prints as `(...)`; an expression longer than 40 characters is cut with `...`. Ranges print as "`x` is exactly 5", "`x` is 0 to 99", "`x` is 0 or more", "`x` is at most 10" or "`x` can be any `int`". In the report, numbers of five or more digits get commas (run-time error messages keep printing plain numbers, as now).

## How the analysis works

### Where it runs

- **On KIR, after `kestrel_kir::optimize`** (inlining, `scalarize`, `promote`), so it sees what will be compiled: inlined bodies with their constant arguments, and fields kept in registers. This is the `@` spec's ruling 2: contracts are decided on KIR after Kestrel's own optimisations, never on Cranelift's output.
- **Inside codegen's entry** (`define_module`, which `JitProgram::compile`, `emit_object` and `dump` all go through), on a copy of the module, with the build's own arithmetic mode. So `kestrel run`, `kestrel build`, the Godot add-on and the benchmark harnesses all get it, and the mode the proofs assumed is always the mode of the code that is emitted.
- **`kestrel check --report`** runs the same function on the optimised module and prints what it returns. Plain `kestrel check` doesn't run it.
- **Per function,** each on its own: no facts cross a call, except the `par for` body's (below).

### What it knows

For each function, at each point, the analysis keeps a **state**:

- **A range for every tracked integer local:** a lowest and a highest value, always inside the local's type (`u8` is 0 to 255, `int` is 32-bit, `i64` 64-bit). A local is tracked when it holds its own value (a `Value` or `Addressed` local). A reference local (a loop element, a `let ... else` or `match` view, a capture, a constant) reads memory, so a load through one has its type's range. Every tracked local and parameter starts at its type's range, never at an empty one (an empty range would "prove" every check vacuously).
- **Facts between terms** of the form `a - b <= c`, where `a` and `b` are terms and `c` a constant. `i < xs.len` is `i - xs.len <= -1`; `j == i + 1` is two facts. The facts are kept closed (every fact the others imply is written down, so a question is one lookup), over at most 64 terms per function.
- **Terms** are: a tracked integer local; `xs.len` of a stable place that is a list or an array; and a loop's hidden end (below). A place is **stable** when its path is fields, enum variants, optional payloads and indexes that are an integer constant or a tracked local, from any root, references included (`self.items`, `grid[y]`, `row` in `for row in grid`). A place indexed by anything else has no length term.
- **Which 64:** a scan of the function before the analysis picks the terms that appear in an index, in a comparison, as a length, or as a copy of one of those (`let j = i + 1`). Locals past the 64th keep ranges but no facts.
- **No overflow inside the analysis:** it computes in 128-bit integers; a shift's amount is first clipped to 0 to 127; and a fact whose constant is larger than 2^66 is dropped, so summing up to 64 facts while closing them stays inside 128 bits.

An integer expression evaluates to a range and, when it has one, a **linear form** `t + c` (a term plus a constant): a tracked local, `xs.len` of a list or array (and `int(xs.len)`, exact by the item limit; never a string's length), `e + c`, `e - c`, a widening conversion, and a narrowing one that is proven or, with checks on, checked.

### Ranges of expressions

| Expression | Range | Its check is proven when |
|---|---|---|
| a constant | exactly it (wrapped to its type, as codegen writes it) | |
| a tracked local | what the state says | |
| any other integer load (memory, a reference, a field) | the type's range | |
| `xs.len` (list, array) | 0 to 2,147,483,647 | |
| a string's length | 0 or more | |
| `a + b`, `a - b`, `a * b`, `-a` | exact from the operands' ranges | the exact result fits the type |
| `a / b`, `a % b` | `/`: at most the dividend's size, either sign (narrower when the divisor's sign is known); `%`: smaller in size than the divisor's largest size, with the dividend's sign | `b` can't be 0, and (signed) not both `a` the type's minimum and `b` -1 |
| `a >> n` | the operand's ends shifted | `n` is 0 to the width minus 1 |
| `a << n` | exact when both ends shifted fit the type, otherwise the type's range (a left shift wraps in both builds; it has no overflow check) | `n` is 0 to the width minus 1 |
| `a & b` | 0 to the smaller high end of the sides that are 0 or more, when one side is; otherwise the type's range | |
| `a \| b`, `a ^ b` | 0 to the next power of two above both high ends, minus 1, when both are 0 or more; otherwise the type's range | |
| `~a` | `-hi - 1` to `-lo - 1` | |
| a widening conversion | the operand's range | |
| a `bool` converted to an integer | 0 to 1 | |
| a narrowing conversion | the operand's range clipped to the target | the operand's range fits the target |
| a float to an integer | the target's range (it saturates; `u8` clamps to 0 to 255) | |
| `rand_int(a, b)` | from the lower of the two low ends to the higher of the two high ends (the runtime swaps reversed bounds); the type's range when that doesn't fit the result's type (codegen narrows the result) | |
| an `if` value (`abs`, `min`, `max` lower to one) | the arms' ranges joined, each arm under its condition | |
| a call, anything else | the type's range | |

A literal divisor is judged as codegen writes it, wrapped to its type: `x / 256` on a `u8` divides by 0, so it keeps its check; a literal that wraps to anything other than 0 and (signed) -1 has no check, the analysis leaves it alone, and the report doesn't list it.

**After a check that isn't proven,** the two builds differ:

- **With checks on,** the result is exact: if it weren't, the check stopped the program. So `let j = i + 1` keeps `j == i + 1` and a range clipped to the type.
- **With `--release`** (overflow, shift and narrowing checks off), the operation wraps, so its result is any value of its type, with no linear form, unless the operation is proven. A bounds proof never rests on arithmetic that could have wrapped.

A **division** is proven only as a whole: its divisor can't be 0 and it can't be the minimum divided by -1, in both builds, so codegen can emit a plain divide. (With `--release`, a division that can't be by 0 but could be the minimum by -1 keeps its zero check: a cost of the simple rule.)

### Conditions

`if c`, `while c`, the condition of a value `if`, and the right side of `and` and `or` are analysed with what the condition says:

- A comparison (`<`, `<=`, `>`, `>=`, `==`, `!=`) of two integer expressions adds a fact between their linear forms, or narrows a range when one side has no linear form. `!=` narrows only an end (`n != 0` with `n` 0 or more makes `n` 1 or more).
- `a and b` true: both hold, and `b` is analysed with `a` true. False: `a` is false, or `a` is true and `b` false, joined. `or` the other way round. `not c` swaps.
- Anything else (a `bool` local, a call) teaches nothing.
- **A condition goes through the same steps as a statement** (next section): forget everything it may write first; refine with its parts in their order; add the facts from checks in it that passed; then forget everything it may write again before either arm, or a loop's body or exit, sees the state. So in `if i >= 0 and i < xs.len and pop_one(var xs) > 0 { xs[i] }`, the fact `i < xs.len` is gone by the time the arm runs, because the pop came after it. This holds for `if`, `while` and the condition of a value `if`.
- A branch the facts show can't run is unreachable: nothing flows out of it, and **its checks stay** (removing them gains nothing).

### One statement at a time

Codegen evaluates a statement's operands in its own order, so the analysis must not depend on it. Each simple statement (`let`, an assignment, a call, a list operation, `print`, ...) goes through four steps:

1. **Forget first:** everything the statement's expressions may write is forgotten (a `var` argument of a call inside it, a value lent to an interface parameter, the statements of a `Seq` an inlined call left, a move).
2. **Each operand from that state:** every operand of an expression is analysed from the state of step 1. Facts an operand makes (inside an inlined call's `Seq`, from a value `if`'s condition, from a check) don't reach its sibling operands. Only the orders KIR itself defines pass facts forward: a `Seq`'s statements before its value, an `if`'s condition before its arms, the left side of `and`/`or` before the right.
3. **Decide the checks** in the statement from what step 2 found.
4. **Then, in this order:** add the facts from checks that passed (next section); forget again everything the expressions may write; apply the statement's own effect (the assignment, the list operation). So `i = xs[i]` first learns `i < xs.len` about the old `i`, then the assignment replaces what is known about `i`, and the fact goes with it. And `s = xs[i] + shrink(var xs)` learns about `xs` and forgets it again, because the call may have shortened `xs`.

`if`, `while` and the other statements that hold statements follow their own rules (Conditions, Loops); the simple statements inside them follow these four steps.

### What a write forgets

A fact holds until something changes a value in it.

- **Every place KIR's walker marks as written** (`visit::places` with its `written` flag) is a **whole write** of that place, unless it is one of the length changes below. A whole write forgets every term inside that place or equal to it. This covers assignments, moves, freeing an arena, `var` arguments, values lent to interface parameters (a borrowed `MakeIface`), `var self` method calls, map writes (`m[k] = v`, `remove`, a loop's lock and unlock), dropping a GPU object, copying a closure's or an interface value's contents, and `in x { }` creating its arena, as well as anything added to KIR later that the walker marks.
- **Two places may be the same** when they belong to the same variable (a loop element `for p in ps` belongs to `ps`, following each `Bind` to its list; a local bound in two different places, which lowering doesn't make today, overlaps every place) and their paths match step by step, where any index may equal any other and two variants of the same enum overlap (they share bytes). So `grid[0] = []` inside `for row in grid` forgets `row.len`.
- **Length changes:** `add` and `insert` add 1; `remove_at` takes 1 away; `clear` makes it 0; `SetLen` sets it; `Compact` (the end of a `remove b` loop) leaves it unknown; `sort` and `Dup` (a copy of a list's items) keep it. **A length is moved only for terms that must be the written list:** the same root, the same path, the same index locals, none of them changed since. Every term that only may be the same list is forgotten. (In `for a in grid { for b in grid { ... a.add(0) ... } }`, `b.len` is forgotten, not moved: `a` and `b` may be different rows.)
- **Assigning a local** replaces what is known about it, and forgets every length term whose place uses it as an index (`grid[y].len` when `y` changes). `x = x + 1` shifts the facts on `x` by 1 instead of forgetting them, when the add is exact.
- **Pointing a loop element at the next item** (`Bind`) forgets the terms rooted at that element.
- **A call** changes only its `var` arguments and the places it lends through interface values. This rests on three of the checker's rules: there are no global variables that change; a closure can't change what it captured; and a `var` argument's place can't overlap another argument of the same call, so two parameters never share memory when one is changed. (Since 2026-10-31 places are compared by field path, not by variable: `step(var g.a, g.b)` passes, while `g` beside `g.a`, two elements of one list, two lanes of one `vec4`, and anything beside a changed place holding an `Arena` still clash.) (The same rules let `promote` keep fields in registers.)
- **A `par for`** forgets every term inside every place it passes to its body.

### Facts from a check that passed

A check that ran and didn't fail teaches something, so in `parts[i].x = 1` then `parts[i].y = 2`, the second check goes.

- **With checks on:** after `xs[i]`, `0 <= i < xs.len`; after an overflow check, the operation's result is exact; after `int(x)`, `x` fits an `int`; after a shift's check, its amount is 0 to the width minus 1.
- **With `--release`, only bounds checks and the zero check of a division teach anything,** because the others aren't emitted: `1 << n` under `--release` says nothing about `n`.
- **Only checks that ran on every path through the statement teach:** not one in an `if` arm, nor one on the right of `and`/`or`.
- **They are added at step 4 of their statement,** before its writes are applied (above). Within one statement no check relies on another, so `parts[i].x = parts[i].x + 1` keeps both its checks unless something else proves `i`.

### Loops

- **Every loop is analysed as a `while`.** `for i from a to b` is `i = a; end = b; while i < end { body; i = i + 1 }`, where `end` is a hidden term, fresh each time the loop is entered (the end is worked out once, before the loop), and the step is a wrapping add, as codegen emits it. `for x in xs` is the same with `end = int(xs.len)`, as lowering builds it. So a loop variable the body changes (only inlining could make one) is handled like any other variable.
- **`continue` in a `for` range goes through the step,** as in codegen: its state gets `i = i + 1` (wrapping) before it joins the top. In a `while`, `continue` goes straight to the top.
- **The state at the top of the loop** is found by repeating: start from the state before the loop; analyse the body under the condition; join the state before the loop with each way back to the top (the end of the body after the step, and every `continue`); widen; repeat until it stops changing. **Widening** moves a range's end that is still growing straight to its type's limit, and drops a fact that got weaker, so the repeat ends after a few rounds. Then **one more pass** over the body, with the final state, decides the checks.
- **The state after the loop** is the top's state with the condition false, joined with every `break`.
- **Nested loops** repeat inside each round of the outer loop; a check is decided only in the final pass of every loop around it, so an inner loop skips its deciding pass unless every loop around it is deciding. Each level then costs about two or three passes of its body.
- **Limits:** at most 200,000 statement visits per function over all passes, or 100 per statement of the function if that is more; and statements or expressions nested at most 256 deep, so the analysis never recurses deeper than that on any thread (the Godot add-on and test threads have small stacks). A function past either limit keeps every check, and the report says "range analysis stopped early in `f` (its code nests too deeply), so every check in it stays". Programs nested 1,000 levels deep still compile.

So `for i from 0 to xs.len { xs[i] }`, `while i < xs.len { xs[i]; i += 1 }`, and the draw copy (`while positions.len < parts.len { positions.add(...) }` then `for i, p in parts { positions[i] = p.pos }`) are proven. A loop whose body pops from `xs` keeps the checks that needed `xs.len`.

### Functions, calls and `par for`

- **At a function's start,** parameters have their types' ranges and lengths are 0 to 2,147,483,647; nothing relates them.
- **A `par for` body** (a function of its own) starts with `0 <= start <= end <= xs.len`, where `xs` is the list it loops over (the runtime hands each thread chunks inside `0..count`, and `count` is `xs.len`), including when `xs` is a temporary the loop made (`par for p in make()`). The body can't change `xs` itself (the checker's `par for` rules), and no other thread does either.
- **A call's result** has its type's range (no facts across calls this round).

### Marking checks

- **Bounds:** `Proj::Index { at, checked: bool }` becomes `Proj::Index { at, check: IndexCheck }` with three values: `Checked`; `Known`, the old `false` (lowering's own knowledge: a `for ... in` loop's own index, and built-in helpers); and `Proven`, set by range analysis.
- **Arithmetic:** a new `ExprKind::Proven(Box<Expr>)` wraps an integer `+ - *`, unary `-`, `<<`, `>>`, `/`, `%` or narrowing conversion whose checks were all proven. Codegen emits the operation inside it without them. Only range analysis makes it, as KIR's last step, so the earlier passes never see one (their matches just walk into it). Code that looks for a shape with `if let` (like codegen's literal-divisor test) sees through it, or is shown not to care.

### Checking the proofs

With **`KESTREL_CHECK_PROOFS=1`** in the environment of `kestrel run` or `kestrel build`, codegen still emits every proven check, but a failing one stops the program with "range analysis removed the bounds check on line 12 of world.kes, but it fails; this is a compiler bug" (exit 101).

- **Always in the program tests:** their JIT pass runs this way, and so does a new release pass (`kestrel run --release` on every program, which fails only on that sentence, since overflow output differs from the checked expectations) and every `tests/release` program. The harness fails any run whose standard error has the sentence.
- **Required at the end of the tasks that build the analysis:** every program in the JIT pass, the executable pass and the release pass, all with the variable on, in light mode (one batch at a time). The executable pass without it, which runs the real unchecked code, runs with every task's own programs and in the final full suite.

### For Round B

Round B plugs in without redoing the analysis:

- **`@assume(cond)`** becomes a KIR statement that the analysis treats as "`cond` holds here": the same refinement `if cond` uses. "It holds until any variable in it changes" is how facts are already forgotten (an assignment, a `var` argument, a change to a list's length), on every path, loop back edges included, since a loop's top state joins every way back. Checking it at run time is codegen's.
- **The end of its block** isn't modelled now: facts outlive blocks, which is sound for an assumption that was checked where it was written and whose variables haven't changed. If Round B keeps the `@` spec's "or its block ends", it adds forgetting at the block's end (less precise, never unsound).
- **What an assumption removes** (the "removes no check; delete it" warning, and whether a bounds check used it, which keeps it checked under `--release`) is found by analysing the function again without it and comparing. Assumptions are rare, so this costs little, and the facts need no record of where they came from.
- **Contracts read the report's data:** `no_checks` is the checks left in a region and in what it calls; `no_alloc` the allocations; `no_panic` the checks and the other ways to fail; `inline` the calls not inlined, with the inliner's own reasons. Each item carries its line, its enclosing loops and its reason; each function's summary carries its first reason, which makes the call-chain message; and each function's first and last lines say which function an inlined item came from.
- **Loops are named by the line of their header** in the report; Round B maps an `@prove` on a loop to the KIR loop that starts on that line (or adds a loop id to KIR, if two loops can share a line).

## Tests

All under the light-mode rules in the plan's Global Constraints.

- **KIR unit tests** (`kir/tests/range_*.rs`): for each rule, a small function in source, optimised, analysed, and the checks counted (proven and left) in it. Each proof has a near miss that must keep its check: `for i from 0 to xs.len` proves `xs[i]` but not `xs[i + 1]`; `i < xs.len` proves `xs[i]` but not after `xs.pop()`; `h & 0x7FFFFFFF` times a 31-bit constant fits `i64`, times a 33-bit one doesn't; `1 << 5` is proven, `1 << n` isn't, nor `x >> n` with `n` from -1 to 3; a `u8` add of two values up to 127 is proven, `200 + 100` isn't, nor `u8(x)` with `x` from -1 to 10; `/` by a divisor guarded `> 0` is proven, unguarded isn't; checked and release differ where wrapping matters.
- **One test for each Critical rule,** a KIR test and a program that must still fail: a passed check about an index the same statement then changes (`i = xs[i]`); a check before a shrinking call in the same statement (`xs[i] + shrink(var xs)`, and the inlined `get(xs, i) + pop_one(var xs)` form); a `--release` shift that would have "taught" its amount (`1 << n` then `xs[n]`); a `continue` that skips to a `for`'s step; and a condition whose later part writes what an earlier part taught (`if i >= 0 and i < xs.len and pop_one(var xs) > 0 { xs[i] }`).
- **Codegen tests** count the checks left in machine code (`trap user2` in `dump`'s output), and run a proven operation that would overflow to show its check is gone.
- **Program tests that must still fail** (`tests/programs/range_keep_*.kes`, JIT and exe, with `// expect-stderr:` and `// expect-exit: 101`), one failure each: an off-by-one loop `0 to xs.len + 1`; a pop inside the loop; a shorter list through a loop element's list (`grid[0] = []` inside `for row in grid`); a `var` call that shrinks the list between the guard and the index; a borrowed interface value's `var self` method that clears it; `remove_at` in a `while` loop; a `remove b` loop then `xs[n - 1]` with the old length; the four Critical cases; an accumulating multiply that overflows; a shift by a computed 40; `int(x)` of a big `i64`; a divisor that is 0 by arithmetic; and the item limit (`array[u8](3000000000)`).
- **Release program tests** (a new folder `tests/release/`, built with `--release` and run, also under the proof check): each guarded so that a wrong rule would prove it: an index that wrapped (`let j = i + 1` with `i` at the maximum, then `if i >= 0 and j < xs.len { xs[j] }`) fails as an index error, not a crash; `if n <= xs.len and i >= 0 and i < n - 1 { xs[i] }` with `n` at the minimum and `i` 5 isn't proven through the wrapped `n - 1`; a shift's amount isn't a fact; a division by a value that wrapped to 0 fails as a division by zero.
- **The proof check** over every program, as above.
- **The report:** `cli/tests/report.rs` runs `kestrel check --report` on small programs: one whole output compared exactly, then each kind of item, a loop nest, a `par for`, an inlined item's "(in `f`)", a function inlined everywhere, a call chain for an allocation, `add` as both an allocation and a way to fail, the `--release` report, the limits message, and the usage error.
- **The item limit:** runtime unit tests for list growth at the limit and for the shared length test, the Godot bridge refusing a longer array, and the program test above.

## Measurement

The last task measures, with the methods in `kestrel/bench/kernels/results.md` (medians, ranges, old and new compilers taking turns, a check with an unused function added for any difference under 10%):

| What | Now | Target |
|---|---|---|
| `bench/voxels3d`, checked exe against `--release` | 2.4x (1,182-1,232 against 481-505 ms) | at most 1.2x; the report shows no check left in the inlined `hash`, `lattice` and `octave` code |
| `bench/voxels3d` `--release` | 481-505 ms | not slower |
| `kernels/copy.kes` (bounds check proven) | 1.09 ns, about 4x Rust | no bounds check in the loop (`--dump bench`); faster; still behind Rust (the data pointer is reloaded) |
| a new `kernels/step_index.kes` (`for i from 0 to parts.len { parts[i].step(...) }`) | 8 checks per particle | no bounds check in the loop; record the gain against `step.kes` |
| every other kernel, `bench/2a`, `bench/heightmap` | | not slower beyond noise |
| compile time (`kestrel build`) of the voxel world and the particles demo | | the analysis adds under 10% |
| checks removed | | the report's totals for the voxel world, the particles demo and the kernels, recorded |

The voxel target is a judgement: after the analysis, about a dozen checks per sample remain (the chunk offsets `cx * 32 + x - 1`, `seed + 1`, `i += 1`, `out[i]`) against about 250 before. If the result misses it, the results file says by how much and why.

## Docs to update

- The syntax spec (`2026-10-09-kestrel-syntax-design.md`): the item limit; that checks the compiler proves can't fail are left out; `kestrel check --report`.
- The README: the report, and what it means for speed.
- The `@` spec: a line under its status that the report and the analysis are built, pointing here, and the block-end question for Round B.
- `docs/superpowers/plans/2026-10-09-core-2a-followups.md`: close "Range analysis for overflow checks" and the bounds half of "Bounds checks and list-header reloads"; add what this round found.
- The handoff's "Start here"; `kestrel/bench/voxels3d/results.md` and `kestrel/bench/kernels/results.md`; this spec's "As built".

## Rulings made on the user's behalf

1. **The analysis runs inside codegen's entry, on a copy of the module, with the build's arithmetic mode** — so no host can compile with proofs made for the other mode, and every way of running a program gets it — cost if wrong: every build pays for a module copy and the analysis, and plain `kestrel check` doesn't run it (only `--report` does).
2. **Ranges for every tracked integer local, plus `a - b <= c` facts between at most 64 terms per function, closed; terms are locals, list and array lengths and loop ends only (no fields in memory, the controller's ruling)** — this covers counted loops, guards, `i + 1`, `n - 1`, `j < i < xs.len` and lengths, and keeps the hardest aliasing out — cost if wrong: no facts about products (`y * w + x`), sums of two variables or fields read from memory, so grid indexes and guards on non-promoted fields keep their checks; a function with more than 64 interesting values loses some proofs.
3. **A list or an array holds at most 2,147,483,647 items, from every route, the Godot bridge included** — `xs.len` is an `int`, so longer lists were already broken; the limit makes every `xs.len` fit and removes its conversion check — cost if wrong: a program that needs a bigger single list or array (over 2 GB of `u8`s) can't have one.
4. **With `--release`, an unproven `+ - *`, shift or narrowing is any value of its type, and only bounds checks and division-zero checks teach facts** — a wrapped value, or a check that was never emitted, must never prove a bounds check — cost if wrong: `--release` proves fewer bounds checks after arithmetic that can't actually wrap but wasn't proven.
5. **One statement (or condition) at a time: forget its writes first, analyse each operand from that state without passing facts between siblings, then add facts from checks that passed on every path, forget the writes again and apply the statement's effect** — the analysis then never depends on codegen's evaluation order, and a fact can't outlive the value it was about — cost if wrong: `xs[i]` twice in one statement keeps both checks, and facts made inside an inlined call used as an operand are lost after its statement.
6. **Every place KIR's walker marks as written is a whole write by default; lengths move only for the surely-same list; writes are matched by variable (following loop elements to their list) and by path, with any index matching any index and enum variants overlapping; a call changes only its `var` arguments and lent interface values** — new writes are covered without anyone remembering to list them, and a "may be the same" list is never shifted — cost if wrong: this rests on the checker's rules (no changing globals, closures can't change captures, no `var` argument sharing a variable with another argument); if one is relaxed, this rule must change with it.
7. **Loops are found by repeating to a fixed point with widening to the type's limits, then one deciding pass; `continue` in a `for` range goes through the step; a function gets 200,000 statement visits (or 100 per statement) and 256 levels of nesting** — simple, always ends, and never overflows a small stack — cost if wrong: deeply nested code keeps every check, and widening loses bounds that only a cleverer guess (a threshold) would keep.
8. **Code the analysis finds unreachable keeps its checks** — removing them gains nothing and would rest on the same reasoning twice — cost: none known.
9. **A division is proven only as a whole (never 0 and never minimum by -1)** — codegen then emits a plain divide — cost if wrong: under `--release` a division that can't be by 0 but could be the minimum by -1 keeps its zero check.
10. **No facts across calls this round** (results, returned lengths) — keeps each function's analysis independent and fast — cost if wrong: `mesh`'s `solid[i ± k]` and similar keep their checks until Round B's `@assume` or a later summary pass.
11. **A `par for` body starts knowing `0 <= start <= end <= xs.len`** — the runtime guarantees it — cost if wrong: if the runtime's chunking ever changes, this must change with it (a runtime test holds it).
12. **`KESTREL_CHECK_PROOFS=1` keeps every proven check as an internal-error check; the program tests' JIT and release passes always use it, and a run of every program in all three passes with it is required at the end of the analysis tasks** — every exercised proof is tested in both modes — cost if wrong: the checked JIT pass no longer runs the unchecked code (the executable pass does), test time grows by one release pass, and the variable is one more thing the CLI reads.
13. **The report is plain text on standard output, grouped by file, function and loop; `--release` gives the release build's report; it includes the runtime's own allocations, other ways to fail (the new item limit included) and calls not inlined with the inliner's recorded reasons, followed through calls; functions carry their first and last lines** — what Round B's four contracts need — cost if wrong: the format may need changes once the user reads real reports; a function inlined everywhere shows only one line; KIR carries a little more data.
14. **Round B learns what an assumption removes by analysing again without it,** with no record of where each fact came from; facts outlive blocks — cost if wrong: a function with many assumptions is analysed once per assumption, and Round B must add block-end forgetting if it keeps that rule.
15. **Hoisting checks out of loops, list-header reloads and promoting indexed places wait** — each is its own change with its own risks — cost if wrong: the voxel target may be missed by the loop-invariant offset checks that remain.

## As built

Built 2026-10-19 and 2026-10-20 on `main` in five tasks: Task 1, the markers, the proof check and the item limit (6547141); Task 2, ranges and the interpreter (8312382); Task 3, lengths and facts between values (7390eee, and the fix for the review's Critical, 6d60ab7); Task 4, `kestrel check --report` (dc6277b, with review fixes 8d20290 and dc78375); and Task 5, the docs and the measurement (the commits "Document range analysis; measurement owed" and "Measure range analysis"). Round A is done: the analysis removes overflow, shift, division, narrowing and bounds checks it proves can't fire, in checked and `--release` builds, and `kestrel check --report [--release]` lists what is left. `@prove` and `@assume` (Round B) are not built.

**Owed, because the user asked that tests not lag the machine:** one full `cargo test --workspace --no-fail-fast` (including the exe pass without the proof check), and workspace clippy and the Godot clippy (`kestrel/godot`) as a whole. (The timing measurement was owed too and was done afterwards, below.) Every task ran focused targets only: the `kir`, `codegen` and `runtime` unit tests, the cli `errors`, `report`, `modules` and `build` targets, the `programs` target filtered with `KESTREL_TEST_ONLY`, and clippy on the crates touched (`kir`, `codegen`, `cli`, `driver`; `godot` and `bench/kernels` were built and clippy-checked after Task 1 and `cargo check`ed after Task 4).

**What was checked.** (The run after the final review's fixes is in the paragraph after the holes, below.) The proof check (`KESTREL_CHECK_PROOFS=1`) ran over every program in all three passes (JIT, exe with `KESTREL_TEST_VERIFY_EXE=1`, and the release pass), in light mode and in three batches of `KESTREL_TEST_ONLY` prefixes, at the end of Tasks 2 and 3 and again after Task 3's fix: 201 programs and 6 `tests/release` programs, no "range analysis removed ..." and no release-pass timeout. Each of the five Critical rules (statement order, sibling operands, release facts, `continue` through the step, conditions that write) has a KIR test and a `range_keep_*` program, and each was shown to be caught by mutation (the tables are in the task reports: the rule broken, the tests that failed). The task reviews (Tasks 1 to 4; Opus for 2 and 3, Sonnet for 1 and 4) found no removed check that could fire except as listed below. Task 5 has no task review of its own and goes to the final review.

**Checks removed.** Counted by a temporary test (deleted) that compiled each of the 183 programs that existed at Task 2 and ran `remove_checks`:

| | Checked: removed / left | `--release`: removed / left | Bounds, both builds: removed / left |
|---|---:|---:|---:|
| Task 2 (arithmetic only) | 950 / 4,420 | 25 / 605 | 0 / all |
| Task 3 first commit | 1,606 / 3,764 | 454 / 176 | 429 / 167 |
| **After Task 3's fix** | **1,608 / 3,762** | **456 / 174** | **431 / 165** |

Over all 207 files (201 programs and 6 release programs): checked 1,677 removed and 3,826 left; `--release` 461 and 204. Two functions in those programs give up (the budget or the nesting limit) and keep every check. Most of the checks left are in programs that exist to fail, and in code the analysis can't know (a value from `rand_int`, an integer field in memory, a list built elsewhere). The voxel world's, the demos' and the kernels' totals are in the measurement below.

**Holes found in review and fixed (soundness).** Five removed checks that could have fired were found before anyone relied on them: three during the tasks (they are why the five rules have programs), one by the final review and one in its fix wave.
1. *(Found in Task 3, in Task 2's code.)* A `Seq`'s value didn't forget what the `Seq` writes: after inlining, `h() { var x = 5 \n bump(var x, 1) + x * 200000000 }` proved the multiply from `x = 5` while `bump` runs first. A `Seq`'s value, and a value `if`'s arm values, now forget their own writes first (`range_keep_seq_value`).
2. *(Found in Task 3, in Task 2's code.)* A condition part didn't forget its own writes before it was evaluated: in `if x >= 0 and x < 100 and bump(var x, 1) + x * 10000000 > 5` the multiply used `x < 100` from the earlier part (`range_keep_compare_write`).
3. *(Task 3's review, Critical.)* Two reads of one written term in one expression shared a node, so `xs.len` before an inlined pop and `xs.len` after it were taken for one value, and a comparison of `L + c` against `L` was decided as a tautology; three routes were reproduced, one of them a segfault under `--release`. Fixed with **volatile nodes**: while an expression is evaluated, every node it writes is volatile, and no linear form or learned fact may name one (`Cx::within`; mutations V1 to V4 in the Task 3 report).
4. *(The final review, Critical.)* Assigning a whole loop element (`row = []` in `for k, row in grid`, `p = Part(items: [])` in `for k, p in parts`, `a = []` in `for a in grid` around `for b in grid`) forgot lengths only through the element itself, with a new local value's rules, so a guard on `grid[k].len`, on `parts[k].items.len` or on another element `b.len` survived it. A memory-safety hole in both builds (bounds checks stay under `--release`): without the proof check two of the probes segfaulted. `assign` now writes an untracked local's new value as a place, so every length its memory may hold goes (`homes.meets`); a tracked integer local is still only its own value (`range_keep_elem_assign`, `range_keep_elem_assign_struct`, `range_keep_elem_assign_other`, and the first two in `tests/release`).
5. *(Found in the final fix wave, while checking the review's minor M2.)* `inline_small` moves a lent interface argument into a local of the inlined body (`let d = <g lent>`), and the call's other arguments run before the method through `d` does. So `cure_now(var g, g.items[1])` checked `g.items[1]` after `g` was forgotten at the `let`, and then `d.heal()` emptied `g.items` with nothing forgotten: a later `g.items[1]` lost its check. `Homes` now points such a local at the lent place (as a loop element points into its list), and a virtual call writes its receiver as a whole (`range_keep_iface_inlined`).

**Checked after the final review's fixes (2026-10-20, night).** Both new holes were shown RED first (the review's probes and the new programs printed "range analysis removed the bounds check ..."), then GREEN, and each fix was removed again to show its KIR test and programs fail (the mutation tables are in `.superpowers/sdd/2026-10-19-range-analysis/final-fix-report.md`). Then the proof check ran over every program in all three passes in one run (JIT, exe with `KESTREL_TEST_VERIFY_EXE=1`, release pass, and the `tests/release` programs in both builds): 206 programs and 8 `tests/release` programs, all passing, no "range analysis removed ..." and no release-pass timeout. The exe pass without the proof check passed too, as did the `kir` and `codegen` unit tests (383), the cli `errors` and `report` targets, and clippy on `kir`, `codegen` and `cli`. The full workspace suite is still owed.

Task 4's review found three Important report problems (not proofs): the code of a closure, a `sort_by` comparison and a live closure over the inliner's size limit was left out of a caller's summary, and a dead inlined closure was counted twice. All fixed (8d20290, dc78375).

**What differs from the plan (and from the spec above).**
- **Files:** the spec's list of `kir/src/range/` files grew: `facts.rs` (the closed `a - b <= c` matrix), `tracked.rs` (which terms exist, the fact-work budget), `bounds.rs` (linear forms and bounds decisions), `arith.rs` (arithmetic with linear forms, split from `eval.rs` for stack size), `par.rs` (a `par for` body's list and chunk), `events.rs` and `events/reasons.rs` (allocations, ways to fail and calls), `summary.rs` (call-chain reasons) and `items.rs`. All are under 400 lines.
- **Bounds live in the ranges, not in the facts matrix.** A first version put zero in the matrix; every range change then cost O(n^2) and a 40-loop test went from 0.7 s to 15 s. The matrix holds only relations between terms, each term's range is kept one step tight against them, and with closed facts that answers what the full closure would.
- **A work budget on facts:** 64 units per statement visit of the function's budget; past it no new facts between terms are made (sound: fewer facts). A loop's top drops its facts after 8 rounds (`SETTLE_ROUNDS`) so repeating always ends. No program in the suite reaches either except the 40-nested-loop budget test.
- **Terms** are tracked integer locals, lengths of stable places and `for` loop ends (hidden terms), as ruled; `pick` chooses at most 64 per function, in first-appearance order.
- **`refine` is a method of the interpreter** (it needs to evaluate and decide), not a free function. Nothing calls it yet (`#[allow(dead_code)]`); it is Round B's `@assume` hook. It truncates `learned` back after use (a leak the stopped Task 2 implementation had).
- **`Stmt::Call` and `Stmt::PushArena` carry a line** (45 mechanical edits), so `idle()` or `arena { }` get their own line in the report. `Func::lines` is the packed first and last line of a declaration. `Module::not_inlined` records every call the inliner left, with the spec's five reasons.
- **Report order:** on one line, the analysis's checks come first and then the other items in run order (two separate run-ordered sequences); loops are grouped at the place of their first item, not sorted by header line; items from an inlined body sort where the item before them does. Built-in module functions and their counts are left out. Totals count an inlined body in the callee's own entry and again in each caller that inlined it, so they are not distinct counts. Numbers of five digits or more get commas, as the spec's rule says (its illustration said "1,234").
- **One dependence on codegen's evaluation order** (ruling 5 says there is none): a list statement's length change (`add`, `insert`, `remove_at`, `clear`, `SetLen`) is applied to the place as named after the statement's operands ran (`change_len`, `set_len`). `grid[i].add(next(var i))` shifts the length of `grid[i]` for the new `i`. That is right because codegen evaluates a list statement's operands before it finds the place (`list_add` in `codegen/src/func/collections.rs`, `list_op` and `set_len` in `lists.rs`); the code says so at both ends, and `range_order_list_place.kes` fails if the order changes. The alternative, forgetting a length whose index locals the operands wrote instead of moving it, was not taken: it costs proofs and nothing needs it while the order holds.
- **Release-pass timeouts** (30 s) are printed on standard error and listed in any failure message, but do not fail the test.
- **Under `--release`, the proof check keeps a proven check even where the build would emit none** (an overflow check, say): a proof is a claim about the operation's real behaviour, and the release pass exists to catch a wrong "can't wrap".

**Rulings.** The spec's fifteen rulings stand. Made later by the controller on the user's behalf, with what each costs if wrong:
1. **Two rounds, A (analysis and report) now and B (`@prove` and `@assume`) next,** because the `@` spec says the report comes first and both are big. Cost: Round B may need analysis changes if A's hooks don't fit.
2. **Integer fields in memory are not terms** (the plan's `Load(PlaceKey)` was dropped): no named win needs them and they carry the hardest aliasing. Cost: a guard on a non-promoted field keeps its checks.
3. **The proof-check run over every program in three passes is required at the end of Tasks 2 and 3,** in light mode. Cost: two slow idle-priority runs.
4. **Lists and arrays are capped at 2,147,483,647 items, the Godot bridge included.** Cost: a new language-visible limit.
5. **The spec and plan had two Opus reviews (four Critical, then one); the last fix was applied without a third review.** Cost: a slip reaches the task review first. The reviews of Tasks 2 and 3 tested it with a negative test.
6. **Task reviews on Tasks 1 to 4, none on Task 5** (the final review covers it). Cost: none beyond review time.
7. **Tasks 1 and 4 on Sonnet, Tasks 2 and 3 on Opus.** Cost: a fix round if Sonnet struggles (Task 4 needed two).

**Not done, and known limits.** What the spec's "Not in this round" lists stays undone, with these consequences: a product like `y * w + x` keeps its bounds check until Round B's `@assume`; a check that doesn't depend on its loop isn't hoisted; a field read from memory has its type's range; a function's result range or returned length is unknown to its caller; the list header's data pointer is still reloaded after a store. These, the deferred review notes and the questions for Round B are in `docs/superpowers/plans/2026-10-09-core-2a-followups.md`, "Found while building range analysis (2026-10-20)".

**Measured (2026-10-20, evening, a quiet machine: no game, the usual desktop background, no idle-priority watcher).** Old compiler (b7166fe) against new (dc78375), exes and kernels built by each and timed in turns per round, one thread, 5-9 rounds, medians. Tables, ranges and run conditions: `kestrel/bench/voxels3d/results.md` and `kestrel/bench/kernels/results.md`, "Range analysis". Against the "Measurement" table above:

| What | Target | Result |
|---|---|---|
| `bench/voxels3d`, checked exe against `--release` | at most 1.2x (was 2.4x) | **met: 1.09x** (643 against 592 ms; the old compiler 1,415 against 594, 2.38x); the checked build is 2.2 times faster |
| the report shows no check left in the inlined `hash`, `lattice` and `octave` code | none | **met**: none in `hash`, `lattice`, `smooth`, `mix`, `octave`; 14 checks per sample remain, in `solids` and `density` (9 chunk offsets, `seed + 1`, `seed + 2`, `y - 64`, `out[i]`, `i + 1`), as the design predicted |
| `bench/voxels3d` `--release` not slower | not slower | **met**: 594 to 592 ms |
| `copy.kes`: no bounds check in the loop; faster | no check; faster; still behind Rust | **mostly met**: no check (`--dump bench`: no `trap user2`); 0.94 times the old time as written and 0-24% faster in other code layouts, steady at 0.91-0.93 ns; 3.4 times Rust (0.27) |
| `step_index.kes`: no bounds check; gain | no check; record the gain | **met**: no check; 8.28 ns against 11.13 (26% faster); 1.43 times `step.kes` (was 1.92) |
| every other kernel, `bench/2a`, `bench/heightmap` | not slower beyond noise | **met**: three kernel differences over 5% (`iface_step_direct`, `iface_hp_direct`, `filter_loop`) all vanish with an unused function added; the checked heightmap is 26% faster (82 to 61 ms) |
| compile time of the voxel world and the particles demo | the analysis adds under 10% | **missed on the voxel programs**: `kestrel build` +30% checked and +43% `--release` for the voxel world (+42 and +48 ms), +32% and +44% for the voxel viewer, +2% for the particles demo (met) |
| report totals | recorded | voxel world: 339 checks, 318 removed, 21 left (`--release`: 8, 7, 1); particles demo: 11 / 9 / 2 (`--release` 1 / 1 / 0); voxel viewer: 454 / 381 / 73 (`--release` 27 / 4 / 23); kernels in the kernels results file |

**The compile-time miss.** About 45 ms of the voxel world's build is the analysis itself (compiling to object code with and without `--keep-checks`: 58 and 102 ms checked, 32 and 80 ms `--release`), in the one function that holds the inlined noise code. A build of a large program takes 110-260 ms either way. It is not profiled; the knobs are `MAX_TERMS`, the fact-work budget and the statement budget. The analysis is the same cost in both builds because it tracks everything either way.

**After the final review (2026-10-20, night).** Codegen now runs the analysis as `mark_proven`, which makes no report text for the checks it leaves (`remove_checks`, for `kestrel check --report`, still does). Measured with the release CLI on `demos/voxels/voxels.kes` (a quiet machine, the idle-priority watcher on): the analysis itself, timed inside the compiler over 9 builds, took a median of 77 ms with the texts and 75 ms without in a checked build, and 50 against 51 ms under `--release`; `kestrel build` end to end, 16 runs each, took a median of 249 ms before and 251.5 ms after (checked) and 224 against 224 ms (`--release`). So the text cost nothing measurable, and the miss stands. Where the time goes, from the same timing: the `par for` body that holds the inlined noise code takes 46 ms of the 75, with 411 statements, 25 terms and 26,562 statement visits (65 per statement, from the nested loops' repeats); `world.solids` and `world.chunk_mesh` take about 13 ms each (401 and 405 statements, 20 terms, about 8,700 visits, 22 per statement). That is about 1.8 microseconds a visit with 20-25 terms. The next steps are in the follow-ups.

**What the voxel result says about the design's judgement.** The design guessed about a dozen checks per sample would remain and the target would be at risk; 14 remain and the target is met with room (1.09x against 1.2x). So hoisting loop-invariant checks is not needed for the target. It would still remove the chunk-offset checks.
