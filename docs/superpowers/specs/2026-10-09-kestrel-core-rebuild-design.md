# Kestrel Core Rebuild

Date: 2026-10-09. Status: approved (architecture delegated to Claude;
sections 1-4 walked through with the user in brainstorm).

Sub-project 2 of 5. Replaces `kestrelc/`, `kestrel.js` and
`jit_codegen.rs` with one new compiler that implements the syntax in
`2026-10-09-kestrel-syntax-design.md`.

## Goal

One compiler, one code generator, two outputs: `kestrel run` (JIT,
in-process, no linker) and `kestrel build` (native executable). A typed
middle layer (KIR) that later sub-projects (parallelism + SIMD, release
backend) build on.

## Milestone: voxel heightmap

The first target program is a single-file voxel heightmap: chunks with
arenas, a hash-noise height function written in Kestrel, an enum with
`match`, a list of chunks, and a printed checksum. No `par for`
(sub-project 3).

### Language subset for the milestone

In: `int float bool u8 i64 f64`, `vec2 vec3` (component-wise ops,
swizzles), `string` values (literals + interpolation), `struct` (field
defaults, methods, `self` / `var self`), `enum` (unit and field
variants, methods), `match` (bare variants, bindings, `else`), `[T]`
with `array[T](n)` and literals, `list[T]` with `add`, `len`, indexing,
`Arena`, `arena { }`, `in x { }`, `let`/`var`, block scoping,
`if`/`else` expressions, `while`, `for from/to`, `for in`, `for i, v
in`, `break`/`continue`/`return`, labelled arguments, call-site `var`,
`and`/`or`/`not`, conversions `int(x)` etc., `pure`, `print`, top-level
`let` constants.

Out (after the milestone, still sub-project 2): `use`/modules/`pub`,
`T?` + `let ... else`, `map`, `ivec*`, `vec4`, string operations
beyond interpolation.

Out (later sub-projects): `par for` (3), C FFI/window (4), release
backend (5), generics/interfaces/closures (future).

### Done when

1. Heightmap prints the expected checksum via JIT and via exe.
2. `kestrel run heightmap.kes` wall time < 50 ms including compile.
3. Runtime within 1.5x of the plain-Rust port (`opt-level=3`). If
   missed, `results.md` records the gap and where the time goes; this
   does not block the rest of sub-project 2 (backend quality is
   sub-project 5).
4. Every check in section 3 has at least one failing-program test and
   one passing-program test.
5. No source file over ~400 lines.

## 1. Architecture

Cargo workspace at `kestrel/` (repo root), one crate per stage:

| Crate | Job | Depends on |
|---|---|---|
| `kestrel-syntax` | `Span`, `Diagnostic` + rendering, lexer, parser, AST | — |
| `kestrel-check` | name resolution, types, effects, moves, exclusivity, arena escape | syntax |
| `kestrel-kir` | KIR data types, lowering from checked AST | syntax, check |
| `kestrel-runtime` | arenas, print, list growth, bounds failure, program start | — |
| `kestrel-codegen` | KIR → Cranelift; JIT and object emission | kir, runtime |
| `kestrel-cli` | `kestrel run` / `kestrel build`, linking, test harness | all |

Data flow:

```
source → lex → parse → AST
       → check → CheckedProgram (AST + side tables)
       → lower → KIR
       → codegen ─┬→ JITModule → run in process
                  └→ ObjectModule → .obj → link with runtime → .exe
```

### AST and side tables

The parser produces an AST where every expression has an `ExprId` and
every name use has a `NameId`. `check` does not build a second tree; it
produces side tables indexed by those ids:

- `types[ExprId] -> TypeId`
- `resolved[NameId] -> DefId` (local, param, function, struct, enum,
  variant, field)
- `effects[FnId] -> Effects`
- `value_use[ExprId] -> Move | Copy | Borrow`
- `region[ExprId] -> Region` (for allocated values)

`CheckedProgram { ast, tables, type_interner }` is the contract between
`check` and `kir`.

### KIR

Typed, structured, desugared. Differs from the AST in that:

- Every value has a concrete type; implicit conversions are explicit
  `Cast` nodes.
- Methods are plain functions with an explicit `self` parameter.
- Struct/enum construction is explicit field/tag stores into a place.
- `match` is a `Switch` on an enum tag.
- Copies, moves and drops are explicit instructions.
- Allocation names its arena operand; `arena { }` / `in x { }` become
  `ArenaPush` / `ArenaPop` around the body.
- String interpolation becomes runtime calls.
- Vector ops become component ops.
- **Loops stay loops** (`Loop`, `ForRange`, `ForEach` nodes), not
  basic-block graphs. Sub-project 3 analyses `par for` here.
- Every function carries its `Effects`.

KIR is not shaped around Cranelift: no Cranelift types appear in it, so
a second backend (C output or LLVM, sub-project 5) reads the same KIR.

### Codegen

One lowering, generic over `cranelift_module::Module`, used by both
`JITModule` and `ObjectModule`. Carried-over lessons from `kestrelc`:

- Calling convention always from `isa.default_call_conv()`, never
  hardcoded.
- `enable_probestack` with the inline strategy (Windows stack growth).
- No POSIX-only calls in the runtime.

## 2. Values and memory

### Layout

| Kestrel | Representation | Size |
|---|---|---|
| `int` / `float` | i32 / f32 | 4 |
| `i64` / `f64` | i64 / f64 | 8 |
| `bool` / `u8` | i8 | 1 |
| `vec2` / `vec3` | 2 / 3 packed f32 | 8 / 12 |
| `struct` | fields in declared order, natural alignment | sum + padding |
| `enum` | u8 tag + union sized to the largest variant | e.g. `Block` = 2 |
| `[T]` | `{ ptr, len: i64 }` | 16 |
| `list[T]` | `{ ptr, len: i64, cap: i64, arena }` | 32 |
| `string` | `{ ptr, len: i64 }`, UTF-8, immutable | 16 |
| `Arena` | pointer to runtime arena object | 8 |

Lengths are i64 so a single array can exceed 2^31 elements (a billion
voxels).

### Value semantics

Every variable owns its data. `var b = a` copies.

- **Read-only parameter:** passed by pointer, never copied.
- **`var` parameter / `var self`:** passed by pointer to the caller's
  place.
- **Last use:** a value not used again is moved, not copied.
- **`for v in xs` / `for i, v in xs`:** `v` refers to the element in
  place (changeable when `xs` is a `var`).
- **Returns:** scalars in registers; aggregates through a
  caller-provided return slot.
- **Copy** of an array/list/string deep-copies its data into the
  current arena. Copies over 1 KB of known size warn.

### Arithmetic and indexing

- **Indexing** is always bounds-checked in the milestone (proof-based
  elision returns in sub-project 3). Failure calls the runtime, which
  prints `index <i> out of bounds for length <n>` with the source
  location and exits with code 101.
- **Integer overflow** traps in `kestrel run` and in `kestrel build`
  by default; `kestrel build --release` wraps. Uses Cranelift's
  overflow-checking add/sub/mul.
- **Division by zero** traps in every mode.
- **Float → int** (`int(x)`) saturates; NaN becomes 0.
- **Narrowing int conversions** (`u8(x)`, `int(x)` from i64) trap when
  the value doesn't fit, except under `--release`, which truncates.
- **`while true` with no `break`** never finishes, so it can end a
  value-returning function.
- **Assignment order:** the value is computed first, then the place, once.

### Non-copyable types and drops

A type is **owning** if it contains an `Arena` (directly or through
fields, enum payloads, arrays or lists). Owning types:

- cannot be copied — only moved; copying is a compile error,
- get a generated drop function that frees each contained `Arena`,
- are dropped at scope end when not moved out, when overwritten, and
  when a list holding them is dropped.

Non-owning arrays/lists/strings need no drop: their memory belongs to
an arena.

### Arenas (runtime)

Chunked bump allocator: a list of blocks (64 KB, or larger for a big
request), a bump pointer, an end pointer. `alloc(size, align)` bumps;
on overflow it grabs a new block. Free releases every block.

Current-arena stack, per thread: the program arena at the bottom;
`arena { }` pushes a fresh temporary arena and pops+frees it at the
`}`; `in x { }` pushes `x` and pops it (no free). Allocation uses the
top of the stack. List growth uses the list's own `arena` field, not
the current arena.

## 3. Checks

Run in order; each collects all its errors before the next starts
(later stages skip if earlier ones failed).

1. **Resolve:** every name to one definition; block scopes; shadowing
   is an error; `break`/`continue` only in loops.
2. **Types:** parameter and return types required, locals inferred,
   literals typed by context, implicit int→float and narrow→wide
   integer, explicit `int(x)` etc. otherwise. Struct construction:
   positional or labelled, defaults fill skipped fields. `match` must
   cover every variant or have `else`. No-effect expression statements
   are errors.
3. **Effects:** per function: `Pure`, `MutatesParams`, `Io`,
   `Globals`. Computed to a fixpoint over the call graph. `pure`
   functions must have no `Io`/`Globals`/`MutatesParams`. Allocation is
   not an effect.
4. **Moves:** last-use analysis per local; use-after-move is an error;
   copying an owning type is an error; large-copy warning.
5. **Exclusivity:** a place changed by a call (a `var` argument, or the
   receiver of a `var self` method) may not also be passed as another
   `var` argument, or as a read-only argument of aggregate type (passed by
   pointer, so it would see the change). Number arguments are copied
   before the call, so `p.add(p.x)` and `bump(var n, n)` are fine. Since
   2026-10-31 places are compared by field path, not by variable: `f(var p.x, p.y)`
   passes, while `f(var p.x, p)` conflicts. Call-site `var` must match
   the parameter. (Refined 2026-10-10 from "never passed twice".)
   Arguments run in source order, so one passed by pointer must not be
   changed by a call or assignment nested in a later argument; and while a
   `match` arm's binding reads from a place, nothing may change that place
   except through the binding. (Added 2026-10-10 after review.)
6. **Arena escape (milestone rules, deliberately conservative):**
   regions are `Program`, `Scoped(block)`, `Owned(path)`, and `Caller`
   (inside a function, the caller's current arena). An allocated value
   may be stored only where it cannot outlive its region:
   - `Program`: anywhere.
   - `Scoped(b)`: into locals declared inside `b`, or passed to calls.
   - `Owned(p)`: into places whose path starts with `p`, or locals
     inside the `in` block.
   - `Caller`: returned, or stored into locals; storing into a `var`
     parameter's place is an error with the hint "allocate it with
     `in <param>.mem { }`".
     (Refined 2026-10-25, generics Task 3c, plan ruling P38: such a store is
     no longer an error in the body. The function records which of its
     `var` parameters it stores caller-arena memory into, including through
     its own calls, worked out to a fixpoint after every body. Each direct
     call (a function, a method, a generic, or a call through an interface
     or a rule, which takes the union of the implementations) then checks
     that the caller's current arena and the arguments' memory may be stored
     into the place passed, as built-in `xs.add(v)` is checked where it is
     written. A function that stores into a `var` parameter can't be used as
     a value, and a closure's own `var` parameters keep the old error.)
   The error names both lifetimes and suggests `copy_to`.

### Diagnostics

`Diagnostic { severity, message, primary: Span, labels, help }`,
rendered as `error: ...`, `--> file:line:col`, the source line, a caret
underline spanning the full range, and `= help:` when present. Spans
are real start..end ranges. The parser recovers at statement
boundaries and keeps going.

## 4. Runtime and linking

`kestrel-runtime` is a Rust crate built as both `rlib` and `staticlib`.
It exports `extern "C"` functions: arena create/free/alloc/push/pop,
list grow, print for each scalar type and strings, string
interpolation builders, bounds failure, and `kestrel_rt_start(entry)`
which sets up the program arena and arena stack, calls the Kestrel
`main`, then frees the program arena and resets all thread-local
state — so the JIT can run many programs in one process (watch mode,
devtool) without leaking or leaking state between runs.

Printing uses Rust's `Display` formatting for every type, so JIT, exe
and the Rust benchmark port print floats identically.

- **JIT:** `kestrel-cli` links the runtime as an rlib; codegen
  registers each runtime function's address with `JITBuilder::symbol`.
  No files, no linker.
- **Exe:** codegen emits an object whose `main` calls
  `kestrel_rt_start(kestrel_main)`. `kestrel build` links it with
  `kestrel_runtime.lib` (found next to the `kestrel` executable) plus
  the native libraries Rust's std needs.
  - Windows: Microsoft `link.exe`, located through the `cc` crate's
    Visual Studio discovery. **Never invoke a bare `link.exe` from
    PATH** — on this machine that is Git's coreutils `link`.
  - Unix: `cc`.

## 5. Testing and benchmarks

- **Program tests:** `kestrel/tests/programs/*.kes` with `// expect:`
  lines. Each runs via JIT and via exe; both must match exactly.
- **Error tests:** `kestrel/tests/errors/*.kes` with
  `// error <line>: <message substring>` lines; the compiler must
  report exactly those.
- **Unit tests** inside crates (lexer, parser, arena runtime, layout).
- **Ported tests:** useful cases from `kestrelc/tests/integration.rs`
  rewritten in the new syntax. Tests of deleted features (memoization,
  `where` clauses, fusion) are dropped.
- **Benchmark:** `kestrel/bench/heightmap/` — Kestrel, plain Rust,
  optimised Rust (`bumpalo`); outputs checked equal; `kestrel run`
  wall time measured; one `results.md` table.

## 6. After the milestone (still sub-project 2)

1. Remaining syntax-spec features listed as "Out (after the
   milestone)" above.
2. Delete `kestrelc/`, `kestrel.js`, `test/*.js`, `package.json`,
   `benchmarks/` (superseded), stale `.worktrees/`.
3. Rewrite `README.md` and `CLAUDE.md` short and accurate; retire
   `kestrel-DESIGN.md` and `docs/SYNTAX.md` in favour of the two specs.
4. `kestrelc-devtool` re-pointed at the new compiler or removed
   (decided then; its native-GUI rebuild is separate future work).

## Risks

- **Scope.** The milestone needs every stage at once. Mitigation: the
  plan builds a thin vertical slice (`main() { print(1 + 2) }` through
  JIT and exe) first, then widens.
- **Escape rules too strict.** Conservative rules may reject valid
  code. Acceptable for the milestone; each false rejection found
  becomes a test and a rule refinement.
- **Rust staticlib link time / size.** std-backed staticlib links
  slower than a C object. Exe builds are not on the dev loop; measure
  and revisit (e.g. `no_std` runtime) only if it hurts.
- **Cranelift gap vs Rust.** Known 1.2-2x on scalar code; addressed in
  sub-project 5, not here.
