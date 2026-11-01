# Kestrel Syntax Redesign

Date: 2026-10-09. Status: approved.

Sub-project 1 of 5 in the Kestrel redesign. Defines what Kestrel source
looks like. Semantics beyond what syntax needs (type checker, IR,
codegen) belong to sub-project 2.

## Context

Decisions from the redesign brainstorm that shape this spec:

- **Target:** games, especially voxel/simulation games with heavy
  number crunching. The language thesis: purity proofs give safe,
  automatic parallelism (`par for`) and SIMD.
- **Memory model:** plain values + arenas. No garbage collector, no
  borrow checker.
- **No classes / no inheritance.** Structs and enums with methods,
  `match` for per-kind behaviour. Interfaces may come later.
- **One implementation.** `kestrel.js` and the separate
  `jit_codegen.rs` are deleted in sub-project 2; one Rust compiler with
  one Cranelift codegen serves JIT (dev) and AOT (exe).
- **Feel:** distinct from Rust; readable by someone coming from
  GDScript.

Sub-project order: 1 syntax (this) -> 2 core rebuild -> 3 parallelism
v2 (chunked `par for` + SIMD) -> 4 game shell (C FFI, window, input)
-> 5 release speed (LLVM backend, precompiled runtime).

## Example

```
use { noise2 from math }

let SIZE = 32

enum Block {
    Air
    Dirt
    Stone { hardness: u8 }

    pure is_solid(self) -> bool {
        match self {
            Air  { false }
            else { true }
        }
    }
}

struct Chunk {
    x: int
    z: int
    mem: Arena = Arena()
    heights: [int] = []

    generate(var self, seed: int) {
        in self.mem {
            self.heights = array[int](SIZE * SIZE)
        }
        for i from 0 to SIZE * SIZE {
            let wx = self.x * SIZE + i % SIZE
            let wz = self.z * SIZE + i / SIZE
            self.heights[i] = height(wx, wz, seed)
        }
    }
}

pure height(x: int, z: int, seed: int) -> int {
    let n = noise2(vec2(x, z) * 0.01, seed)
    if n > 0.5 { int(n * 64) } else { int(n * 32) }
}

pure block_at(y: int, ground: int) -> Block {
    if y > ground          { Block.Air }
    else if y > ground - 3 { Block.Dirt }
    else                   { Block.Stone(hardness: 3) }
}

main() {
    var world = list[Chunk]()
    for cx from 0 to 16 {
        for cz from 0 to 16 {
            world.add(Chunk(x: cx, z: cz))
        }
    }

    par for chunk in world {
        chunk.generate(seed: 1337)
    }

    arena {
        var tris = list[vec3]()
        for chunk in world { mesh(chunk, var tris) }
        print("triangles: {tris.len / 3}")
    }
}
```

## Lexical rules

- **Comments:** `//` to end of line. No block comments.
- **Statement ends:** a newline ends a statement. No semicolons
  required; `;` is allowed as a separator between statements on one
  line.
- **Line continuation:** a newline does *not* end a statement when
  inside an unclosed `(` or `[`, when the line ends with a binary
  operator or `,`, or when the next line starts with `else` or `.`.
  Outside `(` and `[`, two exceptions keep another language's spelling
  to its own line, for its error message: `++` or `--` written together
  at the end of a line ends it (`n--`), and so does a `>` or `>>` when
  the next line starts with `let`, `var`, `for`, `while`, `return`,
  `struct`, `enum`, `interface`, `use` or `pub` (as after
  `let xs: list<int>`). No expression goes on with those words.
- **Strings:** `"..."`, interpolation with `{expr}`, escapes `\n \t \"
  \\ \{ \}`.
- **Numbers:** `42`, `3.14`, `1_000_000`, `0xFF`. An integer literal is
  `int` unless context needs another integer type; a literal with `.`
  is `float` unless context needs `f64`.
- **Hex literals use their type's whole bit width** (decided 2026-10-22,
  with the SIMD vectors): `0x9E3779B1` is the `int` -1640531535 and
  `0xFFFFFFFFFFFFFFFF` is the `i64` -1, so hash constants can be written
  the way they are published. **A hex literal is read at the type it
  meets**, when it is itself an operand (either side of an operator),
  any argument of a built-in (`min`, `max`, `clamp`, `sqrt` ...) or a
  value that is widened: `let x: i64 = 0xFFFFFFFF` and `i64(0xFFFFFFFF)`
  are 4294967295; `let x = 0xFFFFFFFF` is an `int`, -1, and stays -1
  when it is widened later; `w + 0xFFFFFFFF`, `0xFFFFFFFF + w`,
  `max(0xFFFFFFFF, w)` and `max(w, 0xFFFFFFFF)` agree for `w: i64`
  (4294967295), since the literal meets `i64`; and next to a float it is
  the float it spells, so `0xFFFFFFFF * 1.5` is 6442451000 and
  `0xFFFFFFFF < d` with `d = 1.5` is false (it compares 4294967295.0,
  not -1). A *variable* that holds a wrapped hex stays an `int`
  (`x * 1.5` with `x = 0xFFFFFFFF` is -1.5). **A hex expression meeting
  a wider type needs a conversion** (decided 2026-10-24): a literal
  inside a larger expression (`-0xFFFFFFFF`, `0xFFFFFFFF >> 4`,
  `0xFFFFFFFF * 2`, an `if` branch) is read before that expression
  meets the other side, so when it was read as a wrapped `int` and the
  expression then meets an `i64` or a float, that is an error: "`0xFFFFFFFF`
  is read as the `int` -1 here, so this changes when it is widened to
  `i64`; write `i64(0xFFFFFFFF)` to mean 4294967295, or
  `int(0xFFFFFFFF)` to keep -1". The other order, `w + (0xFFFFFFFF >> 4)`,
  reads the literal at `i64` (268435455), as `w + -4294967295` is fine
  where `-4294967295 + w` doesn't fit: one order may be an error, but
  the two orders never give two values. The one asymmetry left is the
  decimal one: next to a `u8` `c`, `0xFFFFFFFF + c` is the `int` -1 plus
  `c` (the literal is read as an `int` and `c` widens) while
  `c + 0xFFFFFFFF` is "doesn't fit in u8", as `300 + c` and `c + 300`
  are. Decimal literals keep
  today's rule ("`2654435761` doesn't fit in int"), a hex literal wider
  than its type is still an error, and `-0x80000000` is an error that
  says to write `0x80000000` (the negation would overflow).

## Declarations

### Functions

```
[pub] [pure] name(params) [-> Type] { body }
```

- No `fn` keyword. Only declarations appear at top level and in
  struct/enum bodies, so a bare `name(` there is unambiguous.
- `pure` marks a function the compiler verifies has no effects (no I/O,
  no writes to anything but its own locals and its `var` parameters,
  calls only pure functions). `var` parameters, including `var self`,
  are allowed (decided 2026-10-09), so `par for` bodies can call
  `pure build(var self)` on their item.
- Parameters: `name: Type`. **Read-only by default.**
- `var name: Type` parameter: the function may change the caller's
  value, and the change is visible to the caller (by reference). A
  `var` parameter has no other meaning — there are no "mutable local
  copy" parameters.
- **Call sites mark `var` arguments:** `mesh(chunk, var tris)`, so a
  reader sees what a call changes. Passing a `var` argument without the
  marker (or marking a read-only one) is a compile error. The receiver
  of a method call is exempt: `chunk.generate()`, not
  `var chunk.generate()`.
- **A call can't see what it changes through another argument.** While
  a call changes a place (a `var` argument or a `var self` receiver),
  no other argument may be that place, a place inside it or one it is
  inside, whether passed as `var`, as the receiver, or read-only with
  an aggregate type (passed by pointer); number arguments are copied
  first, so `bump(var n, n)` is fine. Places are compared by their
  fields (2026-10-31): separate fields of one variable may be passed
  together (`step(var g.player, g.world)`, `swap(var g.a, var g.b)`),
  while two elements of one list (`f(var xs[i], xs[j])`: `i` may equal
  `j`) and two lanes of one `vec4` clash, and a loop item or `match`
  binding counts as the place it reads. A changed place that holds an
  `Arena` clashes with everything else from its variable, since
  replacing it may free memory a sibling field holds.
- Program entry point: `main()`, or the reloadable shape below.

### Reloadable programs

(The hot reload round, 2026-10-30; design `2026-10-30-hot-reload-design.md`.) A program may have, instead of `main`:

```
pub setup() -> Game            // runs once: opens the window, loads assets, makes the state
pub frame(var g: Game) -> bool // runs every frame; false ends the program
```

- Both are `pub` free functions of the program's own file; `frame`'s `var` parameter has the type `setup` returns. A program has `main`, or `setup` and `frame`, never both.
- The state type is anything `checksum` takes, **plus GPU objects** (a `Mesh` or `Texture` keeps working across a reload); it may not hold an `Arena`, a function value or an interface value ("`setup`'s state can't hold a function value; keep functions out of the state").
- Everywhere but the reloading host, the program behaves exactly as if it had `main() { let g = setup(); while frame(var g) {} }`: `kestrel build`, `kestrel check --report` and proofs see that.
- **`kestrel run` reloads it.** While the program runs, saving any of its files recompiles the whole program on another thread; between two frames the new code takes over. When the state's shape (its fields and their types) is unchanged, the new `frame` continues on the same state in place: nothing is copied, the window stays open, meshes stay uploaded (`reloaded pong.kes in 0.31 s` on stderr; `setup() changed but the state was kept; restart to apply it` when only `setup` changed). When the shape changed, the old state is dropped, `setup()` runs again and the program says so (`state reset: Game changed shape; setup() ran again`). A compile error prints its diagnostics and `still running the previous code; fix the error and save again`; the next save tries again. `--no-reload` turns it off; `--record` and `kestrel replay` never reload.
- A second `window.open` while the window is open applies the size and title instead of failing, so `setup` can run again.

### Native functions

```
[pub] [pure] native name(params) [-> Type]
```

- `native` declares a function whose body the runtime supplies, so it has no block (`pub native circle(center: vec2, radius: float, color: Color)`). It is a keyword, and it comes after `pub` and `pure`: `pub pure native`.
- Only Kestrel's built-in modules can declare natives (decided 2026-10-10). A native in a user file is an error: "only Kestrel's built-in modules can declare native functions". A native can't be a method.
- A native's parameters and result are limited to what Rust can read from memory. Details: `2026-10-10-window-design.md` ("Built-in modules and `native`").
- A native that isn't `pure` has effects, like `print`, so `pure` functions and `par for` bodies can't call it.

### Built-in modules

`window`, `draw` and `input` are modules that ship inside the compiler. `use draw` loads the embedded file, and a `draw.kes` next to the program is an error. They are ordinary Kestrel (structs, enums, constants, functions) plus `native` functions, so `use { Color, RED from draw }` and `draw.circle(...)` work like any module. They open a window, draw 2D shapes and text, and read the keyboard and mouse. They are only available in the standalone `kestrel` command, not in the Godot add-on. The full API and the design are in `2026-10-10-window-design.md`.

**`random`** (the determinism round, 2026-10-27) is a fifth built-in module, and unlike the others it is plain Kestrel with no `native`, so it needs no window library and works in a program that never opens one. `use { Rng from random }` gives `Rng(seed: 42)`, a seeded random number generator that lives in the game state (GDScript's `RandomNumberGenerator`, whose seed and state sit inside the object). Its one field, `seed`, is the generator's state: it starts as the seed and moves on with every draw, so it is copied and checksummed with the state around it. Every method is `pure` and takes `var self`: `next() -> i64` (SplitMix64, equal bit for bit to the reference algorithm for the same seed), `randf() -> float` in `[0, 1)`, `randf_range(lo, hi) -> float`, and `randi_range(lo, hi) -> int` with both ends included, the ends in either order, and no overflow over the whole `int` range. The global `randf()` family stays for toys; `@prove(deterministic)` refuses it, and `Rng` is what to use instead. A built-in module's name can't be a file next to the program, so the program test `tests/programs/random.kes` became `global_random.kes`.

**`file`, `time`, `args` and `env`** (the I/O round, 2026-10-29) are four more built-in modules of natives that need no window library, so they work in any program, under `kestrel run` and in a built exe. None of them is `pure`, so `pure` functions and `par for` bodies can't call them, and `@prove(deterministic)` names the call ("which reads a file", "which reads the clock", "which reads the command line", "which reads the environment"). Relative paths resolve next to the program under `run` and next to the exe when built, as `window.screenshot` does; absolute paths are used as given.

| Native | Meaning |
|---|---|
| `file.read(path) -> list[u8]?` | the whole file as bytes, `none` when it can't be read |
| `file.read_text(path) -> string?` | the whole file as text, `none` when it can't be read or isn't UTF-8 |
| `file.write(path, bytes: [u8]) -> bool` | replace the file's contents (a `list[u8]` passes as `[u8]`) |
| `file.write_text(path, text) -> bool` | the same with a string |
| `file.exists(path) -> bool` | whether a file or directory is there |
| `file.list_dir(path) -> list[string]?` | the names in a directory, sorted bytewise; `none` when it isn't one (`list` is a built-in name, so the native can't be called that) |
| `file.make_dir(path) -> bool` | make a directory and its parents |
| `file.remove(path) -> bool` | remove a file, never a directory |
| `file.error() -> string` | why the last call failed, naming the path ("could not open `save.bin`: the system cannot find the file specified"), or `""` after a success; read it before `exists`, which clears it |
| `time.now() -> f64` | seconds since the program started (monotonic) |
| `time.unix() -> i64` | seconds since 1970 UTC |
| `time.sleep(seconds: f64)` | wait; a negative or NaN argument returns at once |
| `args.all() -> list[string]` | the program's arguments: under `kestrel run game.kes a b` they are `["a", "b"]` (every word after the file that isn't a compiler flag, and everything after `--`); in a built exe, everything after the exe |
| `args.program() -> string` | the `.kes` path as typed under `run`, the exe's path when built |
| `env.get(name) -> string?` | an environment variable, `none` when unset or not UTF-8 |

Natives may return strings, lists of plain items or strings, and optionals of those since this round; such a native never runs in a call's scratch arena, so its result lives in the current arena.

**Recording and replaying a window program** needs no code: `kestrel run game.kes --record run.replay` logs every result of a `window`, `draw`, `draw3d`, `input` or `time` native and every `print` line, and `kestrel replay game.kes run.replay` runs the program again with no window, feeding the logged results back and comparing the output line by line: `replay matches (N frames, M lines)`, or `replay differs at frame F, line L` with both lines, or `replay differs at frame F: the program called input.mouse where the recording has window.delta` when the call order changed. A run that fails is recorded too, and must fail the same way. `args.all()` is recorded; `file` and `env` are read again. Details: `2026-10-29-io-round-design.md`, decision 7.

### Methods

Declared inside a `struct` or `enum` body. First parameter is `self`
(read-only) or `var self` (changes the original). Called with dot
syntax: `chunk.generate(seed: 1337)`.

### Structs

```
[pub] struct Name {
    field: Type [= default]
    ...
    methods...
}
```

Fields one per line, no commas. A field with a default may be omitted
at construction.

### Enums

```
[pub] enum Name {
    Variant
    Variant { field: Type, ... }
    ...
    methods...
}
```

Variants without data, or with named fields. Construct with the type
prefix: `Block.Air`, `Block.Stone(hardness: 3)`.

### Variables and constants

- `let x = value` — cannot change after creation.
- `var x = value` — can change.
- Optional type: `let x: float = 1`.
- **Block scoped:** visible from declaration to the end of the
  enclosing `{ }`.
- **No shadowing:** declaring a name already visible in an enclosing
  scope is an error. Visible means in the same file: the file's own items,
  its imports and the enclosing locals. Another file's items aren't
  visible, so a local may share their names ("`r` is already defined in
  this file; Kestrel doesn't allow shadowing" is only for a clash in one file).
- Top-level `let` is a module constant. Top-level `var` is a global;
  touching it is an effect, so `pure` functions can't.

### Imports and visibility

```
use geom                        // then geom.dist2(a, b), geom.Shape.Box(w: 2, h: 3)
use { Point, dist2 from geom }  // then Point(1, 2), dist2(a, b)
use {
    noise2, lerp from math
    colors
}
use net.packet                  // net/packet.kes: packet.make(), net.packet.make(), net.packet.Packet
```

- **Modules.** A module is one file, named after it, found next to the file that uses it. A file is loaded once; files may use each other.
- **Modules in folders.** A module name may be names joined by dots, which are folders: in a file whose folder is `D`, `use net.packet` loads `D/net/packet.kes`, else `D/net/packet/packet.kes` (a folder's own file), and `use net` loads `D/net.kes`, else `D/net/net.kes`, so a module can grow into a folder without changing its users. Both at once is an error: "`net` could be net.kes or net/net.kes; remove or rename one of them". Names never go up (no `..`): a module finds modules next to it or in folders below it, never above, so every file is in the program's folder or below it. A single name that is a built-in module is the built-in; a dotted name never is, and one that starts with a built-in's name is refused ("`draw` is a built-in module, so a folder can't be called draw; rename the folder"). A missing module says where it looked: "can't find module `net.packet`: there's no net/packet.kes next to this file", or "the folder net/ has no net.kes; add one, or use a module inside it, like `use net.packet`".
- **Full names.** A module's full name is its file's path from the program's folder, `.` for `/`, without `.kes` (`net/packet.kes` is `net.packet`; `net/net.kes` is `net`), however a file wrote its `use`; a file reached by two names (`use net.wire` from the program, `use wire` from `net/packet.kes`) is one module, loaded once. After `use net.packet` its items are reached by the last name (`packet.make()`) or the whole name as written (`net.packet.make()`), and the first name (`net`) is taken in that file. Two whole-module uses with one last name bind neither short name ("`packet` could be `net.packet` or `ui.packet`; write the full name, like `net.packet.Packet`"), except that a single name (`use draw`, `use packet`) keeps it. A program that loads both `net` and `net.packet` when `net.kes` has an item `packet` (`pub` or not) is an error: "`net.packet` is both the module net/packet.kes and `packet` in net.kes; rename one of them". In a type, a module the file uses may also be named by its full name (`net.packet.Packet` inside `net/` after `use packet`).
- **`pub`** marks a function, struct, enum or constant as importable. Anything else stays private to its file.
- **A module sees only its own items and its imports.** The program's own file is no exception: a module that uses `bump` from it without importing it gets "unknown function `bump`: it's in main.kes, which `util` can't see".
- **Names must be unique.** An imported name can't collide with the file's own items, another import, or a local variable.
- **Printing** shows a type's own name (`Point`), not `geom.Point`.
- **Types from another module** are imported by name (`use { Point from geom }`) or written with their module, in every type position and in interface lists and rules: `geom.Point`, `net.packet.Packet`, `geom.Stack[int]`, `struct Game: rollback.Rollback`, `[G: net.packet.Shape]`. A module the file doesn't use gets "`geo` isn't a module this file uses; add `use geo`, or import the type: `use { Point from geo }`".
- **Runtime errors** name the line and the file it is in, by its path from the program's folder: "index 5 out of bounds for length 2 on line 24 of geom.kes", "on line 12 of net/packet.kes" (implemented 2026-10-12). Contract chains and `kestrel check --report` name other files the same way (`it allocates on net/stack.kes:4`).

(Implemented 2026-10-10; folders and full names 2026-11-01.)

## Types

| Spelling | Meaning |
|---|---|
| `int` | 32-bit signed integer |
| `float` | 32-bit float |
| `bool` | `true` / `false` |
| `i64` | 64-bit signed integer |
| `u8` | 8-bit unsigned integer, 0 to 255 (a byte) |
| `f64` | 64-bit float |
| `string` | text, UTF-8, a heap value like an array: copying a string copies its bytes into the current arena (literals stay in read-only data and belong to no arena). `s.len` and `s[i]` count characters, not bytes. `+`, `join`, `split`, `slice`, `find`, `upper`, `trim`, `to_int` and the rest: see "Lists and strings" |
| `vec2 vec3 vec4` | float vectors, 2, 3 or 4 `float`s (Godot's `Vector2`, `Vector3`, `Vector4`). `vec4` is one 16-byte SIMD value, aligned to 16, that the CPU adds, multiplies and compares four lanes at a time; `vec2` and `vec3` are plain structs of floats (8 and 12 bytes) that scalar replacement splits into registers. See "Vectors" below |
| `ivec2 ivec3 ivec4` | int vectors, 2, 3 or 4 `int`s (`Vector2i`, `Vector3i`, `Vector4i`), laid out the same way (`ivec4` is one 16-byte SIMD value; `ivec2` and `ivec3` are structs of ints) |
| `[T]` | array of `T`, fixed length once created (fast, arena/SIMD friendly) |
| `list[T]` | growable list: `add`, `remove_at`, `pop`, `insert`, `clear`, `has`, `find`, `join`, `.len`; `remove b` in a `for b in xs` loop. See "Lists and strings" |
| `map[K, V]` | hash map: `m[k] = v`; `m[k]` is a `V?` (so `let v = m[k] else { ... }`); `m.has(k)`, `m.remove(k)`, `m.len`, `for k, v in m` (decided 2026-10-10). Literal `{"a": 1, "b": 2}`, empty `{}`; `m.get(k, default)`; `for k in m` gives keys (decided 2026-10-09). Insertion order. Details: `2026-10-09-map-design.md` |
| `(A, B) -> R` | a function value: `(A, B)` gives nothing, `pure (A) -> R` is pure, `var A` marks a parameter changed in place. See "Closures and function values" |
| `T?` | a `T` or `none` |
| `Arena` | a memory bucket |

- **Type parameters use square brackets:** `list[vec3]`, `map[string,
  int]`.
- **Creating generic values:** `list[vec3]()`, `array[int](n)`, or
  annotate and use a literal: `var tris: list[vec3] = []`.
- **Floats are plain IEEE 754** (the determinism round, 2026-10-27, spec
  `2026-10-27-determinism-design.md`): every operation rounds as written,
  with no fused multiply-add, no reassociation and no fast-math, and there
  is no "strict" flag because strict is the only mode. No optimization may
  change a float result. The transcendental functions on `float` (`sin cos
  tan asin acos atan atan2 exp log pow`) widen their operands to `f64`,
  compute, and narrow the result once, with Kestrel's own wide functions
  (the fast float math round, 2026-10-28, spec
  `2026-10-28-fast-math-design.md`): at most 0.51 ulp from the true value,
  inlined except `pow`. The others (`sqrt floor ceil round abs min max`)
  are exact `float` instructions. Pure math whose operands a loop never
  changes is computed once, before the loop, with the same bits.
- **Vectors** (`vec2 vec3 vec4 ivec2 ivec3 ivec4`; the SIMD round of
  2026-10-22, spec `2026-10-21-simd-vectors-design.md`). One set of rules
  for all six, written in GDScript terms:
  - **Making them.** `vec4(1, 2, 3, 4)` (ints convert to floats);
    `vec4(0.5)` fills every component with one value (GDScript's
    `Vector4(0.5, 0.5, 0.5, 0.5)`, shorter); `vec4()` is zeros;
    giving some components but not all is an error. **Mixed:** the
    values' components must add up, in order, as in a shader:
    `vec4(v3, w)`, `vec4(v2, z, w)`, `vec4(a.xy, b.zw)`, `vec3(v2, z)`,
    `ivec4(iv2, 0, 1)`. **Labelled**, when every value is a number:
    `vec4(x: 1, y: 2, z: 3, w: 4)`, `vec4(w: 4, z: 3, y: 2, x: 1)`,
    evaluated in the order written. One vector of the same type copies
    it; one vector of the other kind and the same size converts it
    (`vec4(iv)`, `ivec3(v)`): float to int truncates toward zero and
    saturates, NaN gives 0 (exactly `int(x)`), and int to float rounds to
    the nearest float. Neither is a checked narrowing. `ivec*` take
    `int`, `u8` and int vectors only: `ivec4(1.5)` and `ivec4(n)` with
    `n: i64` say to use `int(...)` first.
  - **Components and swizzles.** `.x .y .z .w` read and write one
    component (`.w` only on 4-component types): `v.x += 1`,
    `p.vel.w = 0`, `vs[i].y = 7`. Swizzles of 2 to 4 letters, repeats
    allowed, make a vector of that many components of the same kind:
    `v.xy` is a `vec2`, `iv.zyx` an `ivec3`, `v.wzyx` and `v3.xyzz` a
    `vec4`. Swizzles are read-only.
  - **Operators.** `+ - *` and unary `-` are component by component on
    every vector; `/` too, except on `ivec4`. A plain number on either
    side fills every component (`2 * v`, `h & 0xFFFF`), but an int vector
    with a `float` is an error that says to convert with `vec4(...)`.
    Compound forms follow (`v *= 2.0`, `h >>= 1`).
    - **Int vectors** also have `& | ^`, `~v`, `<< >>` and `%` (not on `ivec4`). `<<` and
      `>>` shift every component by one plain `int` amount (checked like
      an `int` shift; `>>` is arithmetic): for different amounts shift each
      component on its own. `ivec4` has no `/` and no `%`, because the CPU
      can't divide integer lanes at once (use `>>` and `&` for a power of
      two, or divide a component); `ivec2` and `ivec3` have both, each
      component divided as `int` is. Float vectors have no `%`, `&`, `|`,
      `^`, `~` or shifts.
    - **`==` and `!=`** give one `bool`: `==` is true when every component
      is equal (a NaN is unequal, as for floats), `!=` is its opposite.
      `< > <= >=` don't exist on vectors: "`<` doesn't compare vectors;
      compare components, like `a.x < b.x`" (GDScript orders `Vector2`
      lexicographically, which almost never means what it looks like).
  - **Overflow on int vectors follows `int`.** In a checked build, any
    component of `+ - *`, `-v` or `abs` that overflows stops the program
    with "integer overflow on line N", and a shift amount out of range
    with "shift amount out of range"; `--release` wraps. `ivec2` and
    `ivec3` are structs of ints, so each component gets an ordinary
    `int` check; `ivec4` is checked with a few lane instructions (about 5
    more than the bare `+`, about 17 for `*` of two vectors, about 30 for
    `*` by a filled constant, about 3 for `-v`), and range analysis
    removes them where it can. **Range analysis tracks an `ivec4` as one
    range for its four components** (the widest), and a condition on one
    component (`if v.x < 10`, `@assume(v.x < 10)`) teaches it nothing
    about the vector; on an `ivec2` or `ivec3` component it does, as on
    any `int` field.
  - **Where they work.** In locals, parameters, results, struct fields,
    `[T]`, `list[T]`, `T?`, map values, constants and field defaults; not
    as map keys. They print as `vec4(x: 1.5, y: 3, z: 4.5, w: 6)`,
    interpolation included. Natives can't take or return `vec4` or
    `ivec4`, and exporting a function that takes or returns `vec4`,
    `ivec4`, `ivec2` or `ivec3` to Godot is today's "can't export"
    error. A `vec4` or `ivec4` is 16 bytes aligned to 16 in every layout
    (a struct field after a `u8` pads to 16). A program that uses
    `vec4` or `ivec4` needs a CPU with SSE4.1 (every x64 CPU since about
    2011; the compiler says so at build time if it is missing).
- **Conversions:** integer -> float and narrower -> wider integer are
  implicit. Float -> integer and wider -> narrower require an explicit
  call: `int(x)`, `u8(n)`.
- **Overflow:** debug builds trap on integer overflow; release builds
  wrap. A check the compiler proves can't fail is left out (see "Checks
  the compiler leaves out"), so the trap is the same wherever it could
  fire. **Int vectors are checked per component** the same way (see
  "Vectors"). **The wrapping operators `*% +% -%`** never check, in any
  build: see "Operators".

## Expressions

- **Operators**, highest to lowest precedence: unary `-`, `not` and
  `~`; `* / % *%`; `+ - +% -%`; `<< >>`; `&`; `^`; `|`; `== != < > <= >=`;
  `and`; `or`. Comparisons don't chain. `< <= > >=` take two numbers, two
  strings (by code point, the order `sort()` uses) or two bools (`false <
  true`), decided 2026-10-25 for generics; a mix is "`<` needs two numbers,
  two strings or two bools, found string and int". Logic is words only: `&&`,
  `||` and `!` don't exist (`!=` stays). Bit operators are symbols, as
  in GDScript, and work on integers (decided 2026-10-10): `& | ^` and
  `~x`, shifts `<<` and `>>` (arithmetic for signed types). A shift by
  a negative amount or by the type's width or more panics when checks
  are on. Compound forms: `&= |= ^= <<= >>=`.
  **Wrapping operators** (decided 2026-10-22): `a *% b`, `a +% b`,
  `a -% b` wrap around the type's width and are **never checked, in any
  build**; `*%` binds like `*` and `+% -%` like `+ -`, all left to right,
  and the compound forms are `*%= +%= -%=`. They work on `int`, `i64`,
  `u8` (`a +% 100` wraps at 256) and every int vector; a float is an error
  ("`*%` wraps integers; for floats use `*`"). Both sides meet at one type
  as `*` does (`int *% i64` is `i64`). In GDScript terms: GDScript's `int`
  is 64-bit and wraps silently, so `h * 31` just wraps there; Kestrel's
  `*` stops on overflow in a checked build, so a hash writes
  `h = (h ^ (h >> 13)) *% 0x5BD1E995`. Range analysis gives a wrapping
  result its wrapped range (exact when the true result fits, else the
  whole type).
- **Calls:** `f(a, b)`. Any argument may be labelled with its
  parameter name: `f(a, seed: 1337)`. Labelled arguments come after
  positional ones.
- **Construction:** `Chunk(x: 1, z: 2)` or positional in field order
  `vec3(1, 2, 3)`. Labels are required once any field is skipped.
- **Blocks are expressions:** the last expression in a block is its
  value. `return` is only for early exit.
- **`if` is an expression:** `let n = if a { 1 } else { 2 }`.
- **Indexing:** `a[i]`. **Field access:** `p.x`.
- **Array literal:** `[1, 2, 3]`. Empty: `[]`.
- **Interpolation:** `"hp = {hp}"` works anywhere, not only in `print`
  (`let label = "hp = {hp}"`). It formats as `print` does, and the string
  is built in the current arena. Because strings are heap values, storing
  a non-literal string into a longer-lived place needs `in x.mem { }`, the
  same rule arrays have.

## Built-in functions

Built into the compiler, with no import, like `print` and `int()`. They
can take several number types, which user functions can't. Their names
are reserved: a function, type, constant, variable or parameter with one of
them is an error ("`min` is a built-in name; pick another").

- **Math:** `sqrt sin cos tan asin acos atan atan2 exp log pow floor ceil
  round abs sign min max clamp lerp`, and the constants `PI` and `TAU`.
  All are `pure`. **On vectors, `min max clamp abs` work component by
  component on every vector, `floor ceil round sqrt` on float vectors,
  and `lerp(a, b, t)` takes two float vectors and a number `t`;** a plain
  number in any argument fills every component (`clamp(v, 0.0, 1.0)`),
  `round` rounds halves away from zero as the scalar one does, and `abs`
  of an int vector is checked like `-v`. `sign` takes numbers only, and
  the rest (`sin`, `pow`, ...) take numbers only. The transcendental
  functions (`sin cos tan asin acos atan atan2 exp log pow`) and the float
  remainder `%` are Kestrel's own, so they give the same bits on every
  x86-64 CPU. On `float` they are Kestrel's wide implementations: at most
  0.51 ulp, computed in `f64` from exact operations and rounded once, and
  inlined except `pow` (whose exponents 1, -1, 2 and 0.5 give `x`, `1/x`,
  `x*x` and `sqrt(x)` exactly for `x > 0`); special inputs (NaN,
  infinities, zeros, arguments outside a function's domain and huge
  arguments) take the `libm` path and give today's results, and `%` is
  exact. On `f64` they call `libm`, a port of
  musl's libm, instead of the C runtime's. Pure math whose operands a loop
  never changes is computed once before the loop, with the same bits.
  `sqrt floor ceil round abs min max` are exact and compiled to CPU
  instructions.
- **Vector methods:** `v.sum()` on every vector (components add left to
  right, `((x + y) + z) + w`, so float results equal the written-out sum;
  an int vector's sum is checked like `+`); on float vectors `v.length()`,
  `v.normalized()` (a zero vector stays zero), `a.dot(b)`,
  `a.distance_to(b)`, and `a.cross(b)` on `vec3` only. These names are not
  reserved.
- **Random numbers:** `randf()`, `randf_range(lo, hi)`,
  `randi_range(lo, hi)`, `rand_seed(n)`. Not `pure`: each call changes the
  generator.
- **`checksum(x) -> i64`** (the determinism round, 2026-10-27): a content
  hash of any value, to find where two runs of a game stopped agreeing.
  `pure`; it allocates nothing and never fails, so `no_alloc`, `no_panic`
  and `deterministic` hold through it. It returns `i64` because Kestrel has
  no `u64` yet: the 64 bits are the same, printed signed. Keep it in an
  `i64`: in a normal build `int(checksum(x))` stops the program for most
  values (a `--release` build wraps). It reads a value's
  contents in a fixed order, never its padding: a bool is 0 or 1; `u8`,
  `int` and `i64` are their value as 64 bits (`checksum(1) ==
  checksum(i64(1))`); a `float` is its 32 bits and an `f64` its 64 bits,
  every NaN made the one canonical quiet NaN (`-0.0` and `0.0` differ); a
  vector is its components in order; a string is its byte length, then its
  bytes; an array or list is its length, then its items (only the first
  `len`: never its capacity or what a shrunk list keeps past its length); a
  map is its live count, then each live key and value in insertion order;
  an optional is 0, or 1 then its value; an enum is its variant number, then
  that variant's fields; a struct is its fields in order. The exact fold
  (start `0x243F6A8885A308D3`, each word mixed as `x = (h ^ w) *
  0x9E3779B97F4A7C15; h = x ^ (x >> 32)`, SplitMix64's finaliser at the
  end) is in `2026-10-27-determinism-design.md`, decision 8; it may change
  between compiler versions until replay files pin it. It refuses, at
  compile time, exactly what a snapshot can't copy: a value holding an
  `Arena` or GPU object ("`checksum` can't take a `World`, which holds an
  `Arena`; checksum the fields that hold the game's state instead"), a
  function value, `pure` or not ("..., which holds a function value, whose
  address differs between builds; checksum the other fields instead") or an
  interface value ("..., which holds an interface value; ..."). In a
  generic, each copy is judged with its own types: `checksum(xs)` with
  `xs: list[T]` is fine for `T = int` and refused for `T` an interface.
- **`to_bytes(x) -> list[u8]`** (the I/O round, 2026-10-29): the value as
  bytes, for save files, replays and bug reports (Godot's `var_to_bytes`).
  It takes exactly what `checksum` takes and refuses the rest in its own
  words ("`to_bytes` can't take a `World`, which holds an `Arena`; save the
  fields that hold the game's state instead"). `pure`; it allocates its
  result once (under `@prove(no_alloc)`: "`to_bytes` allocates its
  result") and never fails. The bytes are a 13-byte header (`KESB`, a
  format version byte, then an `i64` fingerprint of the type's shape: its
  name, field names and types, so a save of one shape is never read as
  another), then the value little-endian with no padding in `checksum`'s
  order: a bool is one byte, `u8` one, `int` four, `i64` eight, `float`
  its four raw bits (NaN payloads kept) and `f64` its eight, a vector its
  lanes; a string is a `u32` byte length then its UTF-8; an array or list a
  `u32` length then its items; a map a `u32` live count then each live key
  and value in insertion order; an optional one byte then the value when
  1; an enum a `u32` variant number then that variant's fields; a struct
  its fields in order. The exact layout is in `2026-10-29-io-round-design.md`,
  decision 2; renaming or reordering a field changes the fingerprint.
- **`from_bytes[T](b: list[u8]) -> T?`**: reads a value back. The type is
  written in brackets, as a generic call (`from_bytes[Game](bytes)`; a
  bare `from_bytes(b)` says what to write). `pure`; it builds the value in
  the current arena. It gives `none`, and never stops the program or
  allocates by a length the bytes claim, on: a wrong header, version or
  fingerprint; bytes that end early; a bool or optional byte that isn't 0
  or 1; a variant number past the enum; a string that isn't UTF-8; a
  length past `i32::MAX` or past the bytes left; a repeated map key; or
  bytes left over. After a round trip `checksum(x) == checksum(y)`. Map
  keys use the unseeded deterministic hash, so a hostile file can pick
  colliding keys and make a large map slow to build (never wrong).
- **`first_difference(a, b) -> string?`**: where two values of one type
  first differ, as a path with both values (`ball.x: 1.5 and 1.25`,
  `inputs.len: 3 and 4`, `inputs[2].p1.up: true and false`,
  `scores["ana"]: 3 and missing`, `state: Alive and Dead`, `target: none
  and a value`; a bare scalar gives `1.5 and 1.25`), or `none` when they
  are equal. Floats compare by their bits: `-0.0` and `0.0` differ (`x: -0
  and 0`), and two NaNs that print alike get their bits appended (`nan and
  nan (bits 0x7FC00000 and 0x7FC00001)`). Two maps with the same entries in
  another order differ (`scores: the same keys in another order`), because
  insertion order is the value's order for `to_bytes` and `checksum` too.
  `pure`; equal values allocate nothing, a difference allocates its text.
  It takes what `checksum` takes ("`first_difference` can't compare a
  `World`, which holds an `Arena`; compare the fields that hold the game's
  state instead") and both arguments must have one type.

The full table of argument and result types is in
`2026-10-10-window-design.md` ("Global math and random numbers").

## Statements

- Assignment: `x = v`, `p.x = v`, `a[i] = v`, compound `+= -= *= /=
  %=` (and `*%= +%= -%=`). Target must be a `var`, a `var` parameter, or reached through
  `var self`.
- `print(...)` is an ordinary (effectful) function, usable anywhere.
- **No-effect statements are errors.** An expression statement whose
  value is unused and that has no effect (`+bonus`, `x == 1`) is a
  compile error: "this line has no effect; did you mean to continue
  the line above?" This closes the line-continuation trap where a line
  starting with an operator silently becomes its own statement.

## Control flow

```
if cond { ... } else if cond { ... } else { ... }
while cond { ... }
for i from 0 to n { ... }      // n exclusive, step +1
for v in xs { ... }            // each element
for i, v in xs { ... }         // index and element
par for v in xs { ... }        // parallel, see below
break
continue
return [value]
```

No parentheses around conditions.

### `match`

```
match value {
    Variant          { ... }
    Variant binding  { ... }   // binding names the variant's fields: binding.hp
    else             { ... }   // catch-all
}
```

- `match` is an expression.
- Every variant must be handled or `else` present — adding a variant
  makes the compiler list every `match` that misses it.
- Inside `match`, variants are written bare (`Air`), since the type is
  known.
- A binding refers to the payload in place; it is changeable when the
  matched value is (`var self`, a `var`).

### `par for`

```
par for chunk in world {
    chunk.generate(seed: 1337)
}
```

Runs iterations in parallel across CPU cores. The body may only:

- call `pure` functions,
- call `var self` methods on the loop's own item, when those methods
  change nothing but that item,
- change the loop's own item and its own locals.

Anything else is a compile error naming the offending line, e.g.
"line 3 writes to `world.score`, which is shared." Work splitting
(chunking) and SIMD are sub-project 3. Details and the remaining
rules: `2026-10-09-par-for-design.md`.

## Lists and strings

Design and rulings: `2026-10-17-lists-strings-design.md`. Names follow GDScript where a GDScript name fits.

**List methods.** `has` and `find` also work on arrays; the rest need a `list[T]` held in a `var`.

| Kestrel | Meaning |
|---|---|
| `xs.add(v)` | add at the end |
| `xs.remove_at(i)` | remove item `i`; the rest keep their order |
| `xs.pop()` | remove the last item and return it as a `T?` (`none` when the list is empty); GDScript's `pop_back` |
| `xs.insert(i, v)` | put `v` at `i`, shifting the rest up; `i == xs.len` adds at the end |
| `xs.clear()` | remove every item |
| `xs.has(v)` | `true` when an item `== v` (numbers, bools and strings) |
| `xs.find(v)` | the index of the first item `== v`, as an `int?` (GDScript returns -1) |
| `parts.join(sep)` | one string from a list or array of strings |

`filter`, `map`, `any`, `all`, `sort` and `sort_by` are under "Closures and function values".

An index outside `0..xs.len` panics. **A list or an array holds at most 2,147,483,647 items** (`i32::MAX`, like C# arrays), so `xs.len` always fits an `int`: growing past it (`add`, `insert`, a built-in that builds a list, `array[T](n)`, or a Godot array passed in) is a run-time error, "a list can't hold more than 2,147,483,647 items" ("an array can't hold ..." for an array). Strings have no limit. `remove_at`, `pop`, `insert` and `clear` are refused inside a `for` loop over the same list, because the loop read the length once. `add` is allowed there; the loop doesn't see the new items.

**`remove b`** removes the current item of a `for b in xs` loop over a list. The loop compacts the list in one pass, so items keep their order and none is skipped (in GDScript, `xs.erase(b)` inside `for b in xs` skips the next item).

```
for b in bricks {
    if hits(ball, b) {
        remove b
    }
}
```

- `remove` is not a keyword: `remove` followed by a name, as a statement of its own (`if hit { remove b; continue }` too), is this statement.
- `b` is gone after `remove b`, so using it later is an error. The rest of the body still runs, so follow it with `continue` to skip that.
- While a loop that uses `remove` runs, its body can't use the list except through `b` (not `xs`, `xs.len` or `xs[j]`), because the items are moving. Other fields of the same variable are fine.
- Not allowed in `par for`, in a loop over an array, map, string or a temporary, or on a list declared with `let`.

**Strings** are UTF-8. Lengths and indexes count characters (code points, as in GDScript), not bytes: `"héllo".len` is 5.

| Kestrel | Result |
|---|---|
| `s.len` | `int`, the number of characters |
| `s[i]` | the character at `i`, as a one-character `string` |
| `for c in s`, `for i, c in s` | each character as a `string`, and its index |
| `a + b`, `s += t` | a new string (both sides must be strings; use `"{n}"` for numbers) |
| `s.split(sep)` | `list[string]`; `"".split(",")` is `[""]`; `sep` can't be empty |
| `s.has(t)`, `s.starts_with(t)`, `s.ends_with(t)` | `bool` |
| `s.find(t)` | the character index of the first match, as an `int?` |
| `s.slice(from, to)` | the characters from `from` up to, not including, `to` (GDScript's `substr` takes a length instead) |
| `s.upper()`, `s.lower()`, `s.trim()` | a new string (`trim` is `strip_edges`) |
| `s.to_int()`, `s.to_float()` | `int?` / `float?`, `none` when the text isn't a number (GDScript returns 0) |

- Strings can't be changed in place: `s[i] = x` is an error. Build a new string.
- Every string these produce is new and lives in the current arena, like interpolation. Storing one in a longer-lived place needs `in x.mem { }`. In a frame loop, wrap the body in `arena { }` to free them each frame.
- `.len`, `s[i]`, `slice` and `find` walk the string from its start, so `s[i]` inside a loop over `i` is slow on long text. `for c in s` is the linear way.
- A bad `s[i]` or `slice` panics with the usual line and file.

```
var words: list[string] = []
for part in "go, stop ,  jump".split(",") {
    words.add(part.trim().upper())
}
print(words.join("-"))      // GO-STOP-JUMP
```

## Closures and function values

Design and rulings: `2026-10-18-closures-design.md` (its "As built" section lists what differs). A closure is a function with its name cut off, as GDScript's `func(e): ...` is `func name(e): ...` without the name.

```
square(x: int) -> int { x * x }                    // named function
let sq = (x: int) -> int { x * x }                 // the same, as a closure
let alive = enemies.filter((e) { e.hp > 0 })       // e's type comes from context
let hurt = (amount: int) { print("ouch {amount}") }
hurt(3)
```

**Literal.** `(params) [-> Type] { body }` is an expression. A parameter is `name: Type` or `var name: Type`; the type can be left out where a function type is expected (a list method's argument, a function-typed parameter, an annotated `let`, a field, a default, a `return`). The body is a block whose last expression is the result, and `return v` leaves the closure. The result type is inferred from the body unless written with `->`. A closure is `pure` when its body is. A closure parameter can't share a name with a local that is already visible, like any other local.

**Parentheses.** In expression position, a `(` whose `)` is followed by `{` or `->` starts a closure. In the head of an `if`, `while`, `for`, `match` or `in`, `(cond) {` stays a parenthesised condition, so `if (ready) { go() }` still works. A closure in a head goes inside a call's parentheses: `if xs.any((e) { e.dead }) { ... }`.

**Function types.** `(A, B) -> R`, `(A, B)` (gives nothing), `()`, `pure (A) -> R`, and `var A` for a parameter changed in place. They work in every type position: `list[(int) -> int]`, fields, parameters, results. `T?` of a function type is refused; give a callback a do-nothing default instead (`on_hit: (int) = (n) { }`). A function value of a `pure` type goes where a non-pure type is expected, not the other way round.

**Named functions are values:** `let f = square`, `xs.sort_by(score)`, `util.score`. Methods, built-in functions (`min`, `sqrt`, `print`) and natives (`draw.rect`) are not; wrap them in a closure. Function values can't be compared with `==` or used as map keys, and `print` writes `<function>`.

**Calling.** `f(x)` for a local, parameter or constant of function type, `self.on_hit(3)` for a field (a struct can't have a function field and a method with the same name), and `handlers[i](3)` or `make_adder(2)(5)` for any other expression. Arguments are positional (no labels), and a `var` parameter is marked as usual: `f(var p, 0.5)`.

```
struct Button {
    label: string
    on_click: (int) = (n) { }

    press(self) { self.on_click(1) }
}

make_adder(k: int) -> (int) -> int {
    (x) { x + k }
}

main() {
    let add2 = make_adder(2)
    print(add2(5))              // 7
    let b = Button(label: "ok", on_click: (n) { print("clicked {n}") })
    b.press()
}
```

**Captures.** A closure captures the locals and parameters it names, **by value**, as GDScript does: it sees what they held when it was made. What is captured is a path (`self.name`, `w.score`), not the whole variable. A closure can't change what it captured (assigning, `+=`, `var` arguments, changing methods and moves are errors; use a `for` loop to accumulate), and it can't capture a value that owns an `Arena` or a GPU object. A closure that may be stored can't capture a whole struct that holds a function value (`self.cb = () { self.fire() }`, or `make_timer(3, () { self.fire() })`, since a function may store its argument); copy what it needs into a local first. A literal passed to `filter`, `map`, `any`, `all` or `sort_by` is never stored, so it may. A function value, or a list or map of them, can be captured. `break`, `continue` and `remove` can't cross a closure's boundary.

**Memory.** A stored closure with captures lives in the arena that was current when it was made, like a string, so storing it somewhere longer-lived needs `in x.mem { }`. A closure without captures, and a named function, allocate nothing and can go anywhere. A closure literal written directly as an argument to a call of a user function, a method or a function value allocates nothing either, so the call must not change anything the closure reads: "`xs` is changed by this call, so the closure passed to it can't read it".

**`pure` and `par for`.** A `pure` function or a `par for` body may call a function value only if its type is `pure`, and a closure literal written there must be pure.

### List methods that take functions

`filter`, `map`, `any`, `all`, `sort` and `sort_by` work on lists and arrays (`sort` and `sort_by` need a `var` list).

| Kestrel | Meaning |
|---|---|
| `xs.filter(f)` | a new list of the items for which `f(item)` is `true` |
| `xs.map(f)` | a new list of `f(item)` for each item |
| `xs.any(f)` | `true` when `f` is `true` for some item; stops at the first |
| `xs.all(f)` | `true` when `f` is `true` for every item; stops at the first `false` |
| `xs.sort()` | sorts in place, smallest first; items are numbers, strings or bools |
| `xs.sort_by(key)` | sorts in place by `key(item)`, a number, string or bool; `key` must be `pure` |

```
let alive = enemies.filter((e) { e.hp > 0 })
let spots = enemies.map((e) { e.pos })
if enemies.any((e) { e.boss }) { alarm() }
enemies.sort_by((e) { e.pos.distance_to(player) })
```

- `f` takes the item read-only. Any function value of the right type works, not only a literal.
- `filter`, `map`, `any` and `all` call `f` once per item, first to last. The `sort_by` key runs twice per comparison, so its order is not promised, and it runs in a scratch arena freed after each comparison.
- Sorting is stable. Strings compare by code point, `false` comes before `true`, and a float NaN sorts last. For largest first, negate a number key.
- `filter` copies the kept items, so it refuses items that own memory (use a `for` loop with `remove`). `map` results may own memory.
- `sort` and `sort_by` are refused inside a loop over the same list (or over a map held in it), and a key can't read the list it sorts. `remove_at`, `insert`, `pop` and `clear` are refused inside a loop over a map held in the list as well. So is a call given the list, or what holds it, as `var`.
- Not built: `reduce`, a comparator sort, `reverse`, `min_by` and `max_by`.

## Interfaces

Design and rulings: `2026-10-19-interfaces-design.md` (its "As built" section lists what differs). An interface is a named list of methods. Any struct or enum that says it implements the interface can stand in for it, so one `list[Damageable]` can hold goblins, slimes and barrels, and the compiler checks the promise. GDScript does this with duck typing (`if body.has_method("take_damage")`), which is unchecked until the line runs.

```
interface Damageable {
    take_damage(var self, amount: int)
    pure hp(self) -> int
}

struct Goblin: Damageable {
    health: int = 10
    take_damage(var self, amount: int) { self.health -= amount }
    pure hp(self) -> int { self.health }
}

enum Barrel: Damageable {
    Full { oil: int }
    Empty
    take_damage(var self, amount: int) { ... }
    pure hp(self) -> int { ... }
}

hit_all(var foes: list[Damageable]) {
    for f in foes { f.take_damage(2) }
}

main() {
    var foes: list[Damageable] = []
    foes.add(Goblin())
    foes.add(Barrel.Full(oil: 5))
    hit_all(var foes)
}
```

**Declaring.** `[pub] interface Name { ... }` holds method signatures with no body, one per line: `[pure] name(self | var self, params) [-> Type]`. There must be at least one, and no fields or default bodies. `interface` is a keyword.

**Implementing.** `struct Name: A, B { ... }` and `enum Name: A { ... }`, where the type is declared. For each method of each listed interface the type needs a method with the same name, the same `self` or `var self`, the same parameter types (and `var`s) and the same result. A `pure` method may implement a method declared without `pure`, not the other way round. Extra methods are fine. Interface names are found like type names (`use { Shape from shapes }`).

**Using one as a type.** An interface works in every type position: parameters, results, `let`/`var`, fields, `list[Damageable]`, `[Damageable]`, map values and `Damageable?`. A struct or enum that implements it goes wherever the interface is expected (an annotated `let`, an argument, `add`, a field, a constructor argument, a `return`, each element of an annotated list literal). The conversion is one level deep: a `list[Goblin]` is not a `list[Damageable]`. An interface value doesn't convert to another interface or back to its struct (there is no type test yet). A list literal that mixes types needs its type written: `var foes: list[Damageable] = [...]`. Calling `d.take_damage(3)` runs the method of whatever `d` holds. A `var self` method needs a changeable place, as for structs. `print` writes the value inside. Interface values can't be compared with `==`, used as map keys, or searched with `has` and `find`.

**Borrowed, boxed, copied.** An interface value is two words: the data and the type's method table.
- **Borrowed.** A struct or enum value passed straight to a parameter of interface type (`show(g)`) is not copied: the callee sees `g` where it is, so `heal(var g)` with `heal(var d: Damageable)` changes `g`. Only a place can be lent this way: a variable or parameter (a loop element, a `match` binding and a closure's capture included), a field of one, an item of a list or an array, and a constant that holds a list, array, map or interface value (such a constant is built once and read in place). Anything else (a call's result, a map lookup `m[k]`, a constant of plain values) is boxed as a copy. The callee may not replace a `var` interface parameter as a whole (`d = Slime()`), because the caller's place may be a `Goblin`.
- **Boxed.** Everywhere else (a `let`, a field, a list item, a map value, a `return`) the struct is copied into the current arena and the interface value points at the copy, as for a string. So storing one somewhere longer-lived needs `in x.mem { }`, and the escape check treats it like a string.
- **Copied.** Copying an interface value copies the struct it holds, so copies change independently, as for every Kestrel value. `foes[0] = Slime()` replaces a whole item.

**The owning rule.** A struct that owns an `Arena` or a GPU object may implement an interface and be lent to it, `var` included, but it can't be stored as one: "`Sprite` owns a GPU object, so it can't be stored as a `Drawable`; keep it in a `list[Sprite]`". Where the compiler can't see the copy coming (a callee that stores its borrowed `Drawable` parameter) it is a run-time error naming the type and the interface.

**`pure` and `par for`.** A `pure` function or a `par for` body may call only methods declared `pure` in the interface.

**Cost.** A call through an interface is an indirect call through the method table, and is not inlined; the kernels in `kestrel/bench/kernels/results.md` measure it. Storing allocates a copy; lending does not.

## Generics

Design, rulings and what was built: `2026-10-24-generics-design.md` (its "As built" section lists what differs). A generic is a struct, enum or function written once with placeholders, and used for many types. In GDScript terms: `Array[Enemy]` is a typed array, and `list[Enemy]` is the same idea; a generic lets you write your own `Stack[T]`, which GDScript can't. Untyped GDScript gets the same reuse from `Variant`, which is boxed and only checked when the line runs. A Kestrel generic is checked once, where it is written, against what its placeholder is promised to be; then the compiler makes a separate compiled copy for each type you use it with (`Stack[int]` and `Stack[string]` are two types with two sets of methods). There is no boxing and no type tag at run time, and no `Variant`.

```
struct Stack[T] {
    items: list[T] = []

    push(var self, v: T) { self.items.add(v) }
    pop(var self) -> T? { self.items.pop() }
    pure size(self) -> int { self.items.len }
}

pure total_hp[T: Damageable](xs: list[T]) -> int {
    var sum = 0
    for x in xs { sum += x.hp() }
    sum
}

main() {
    var names = Stack[string]()            // the types written
    names.push("a")
    var ints: Stack[int] = Stack()         // or worked out from the annotation
    ints.push(3)
    print(ints, names.size())              // Stack[int](items: [3]) 1
}
```

**Declaring.**
- **Placeholders** go in square brackets after the name of a struct, an enum or a function, separated by commas: `struct Stack[T]`, `enum Result[T, E]`, `best[T](...)`, `pure best[T](...)`, `pub best[T](...)`. A struct's or enum's interface list comes after them: `struct Stack[T]: Named { ... }`.
- **Names:** a placeholder's name starts with a capital letter (`T`, `Item`), and it can't be the name of a type that exists (a struct, enum, interface or built-in type anywhere in the program): Kestrel never lets one name hide another. A function or constant with the same name is fine. Each name appears once in a list.
- **Methods** use their struct's or enum's placeholders and have none of their own ("methods can't have placeholders of their own yet; use a generic function that takes the struct instead"). Interfaces, natives and `main` have none either.
- **Inside its own declaration** a generic type's bare name means itself with its own placeholders: in `struct Stack[T]`, `Stack` is `Stack[T]`. Everywhere else it is written with its types: "`Stack` needs its types, like `Stack[int]`".

**Rules.** What a placeholder can do comes only from the rules after it, joined with `+`: `[T: Damageable + Named, K: key]`. A rule is the name of an interface, or one of three built-in words that are words only in this position (`key` stays usable as a name):
- `equatable`: `==`, `!=`, `xs.has(x)`, `xs.find(x)`. The types that allow it are the numbers, `bool`, `string` and the vectors.
- `ordered`: `<`, `<=`, `>`, `>=`, `xs.sort()` and a `sort_by` key. The numbers, `bool` and `string`. It includes `equatable`.
- `key`: a `map[K, V]` key. `int`, `i64`, `u8`, `bool` and `string`. It includes `equatable`.

An interface can't be named `equatable`, `ordered` or `key`. Two interface rules whose methods share a name must have the same signature (a call means either), else "no type can be both `A` and `B`: their `name` methods differ". In GDScript the nearest thing is checking `has_method` before a call, which is unchecked until the line runs.

**Using.**
- **In types:** `Stack[int]`, `Pair[string, vec2]`, `list[Stack[int]]`, `map[string, Result[Level, string]]`. A generic type from another module is imported by name (`use { Stack from geom }`) or written with its module (`geom.Stack[int]`), with its types.
- **In expressions** brackets after a name mean types when the name is a generic struct, enum or function, and an index otherwise: `best[Enemy](foes, score)`, `Stack[int]()`, `Result[int, string].Ok(value: 3)`, `Maybe[int].Nothing`, `geom.best[Enemy](...)`, `geom.Stack[int]()`; but `handlers[i](3)` indexes (a local is never hidden by an item of the same name). Written types give all of the placeholders, never some ("`Result` takes 2 types, found 1").
- **Methods** are called on a value as usual (`ints.push(3)`); the value's type fixes the placeholders.
- **A generic function is a value once its types are fixed:** `let f = best[Enemy]`, `let RULE = best[int]` (a constant). `let f = best` is "`best` is generic; give its types to use it as a value, like `best[int]`".
- **`match`** on a generic enum names variants as for any enum; `r.value` has the copy's type. `print` shows `Stack[int](items: [3])` and `Result[int, string].Ok(value: 3)`.

**Working out the types.** When no types are written, a call, a construction or a variant works them out from what you pass, in this order:
1. **The values**, in source order. A placeholder met by an `int` and then a `float` becomes `float` (numbers widen, as mixed arithmetic does), **except** through a `var` argument, inside a type (`list[T]`, `T?`, a function type) or by a closure's result, which must match exactly.
2. **The expected type** (a `let` annotation, a parameter, a field, a `return`) fills only what the values left open. `let x: float = first(ints)` works; `let s: Stack[float] = Stack(items: [1, 2])` is an error, because `T` is `int` from the list: write `Stack[float](items: [1, 2])` or `[1.0, 2.0]`.
3. **Deferred arguments:** closures whose parameter types aren't written, `none`, `[]` and `{}`, each checked with the parameter type filled in. A closure's result decides an open placeholder exactly (`make(() { 3 })` gives `int`).
4. **Every placeholder must be decided:** "can't tell what `T` is in `best`; write it, like `best[int](...)`". A conflict says where each type came from: "`T` in `pick` can't be both `int` (from `a`) and `string` (from `b`)".

A non-empty array literal given for a `list[T]` parameter decides `T` through its items (`count([1, 2])`). A placeholder no parameter mentions can only be written (`make_empty[int]()`).

**What `T` can do inside generic code.** The body is checked once, with `T` unknown. A `T` value can be stored, copied, passed, returned, printed (`print(x)`, `"{x}"`), held in a `list[T]`, a `[T]` literal, a `T?`, a map value or another generic type, captured by a closure and passed where a `T?` is expected. Everything else needs a rule:

| Written | Needs | Otherwise |
|---|---|---|
| `x.hp()` | an interface rule with a method `hp` | "`T` has no method `hp`: its rules (`Named`) don't have one"; with no rules, "`T` has no rules, so it has no methods; add one, like `[T: Damageable]`" |
| `x` where a `Damageable` is expected | the rule `Damageable` | "expected Damageable, found T" |
| `==`, `!=`, `has`, `find` | `equatable` | "`==` on `T` needs the rule `equatable`: write `[T: equatable]`" |
| `<`, `<=`, `>`, `>=`, `sort()`, a `sort_by` key | `ordered` | "`<` on `T` needs the rule `ordered`: write `[T: ordered]`" |
| `map[K, V]` | `K: key` | "`K` can't be a map key without the rule `key`: write `[K: key]`" |

- **Fields of `T`** aren't reachable: "`x` is a `T`, which can be any type, so it has no field `hp`", help "give `T` a rule with a method instead, like `[T: HasHp]`".
- **Arithmetic** isn't available (there is no `number` rule this round): "`+` doesn't work on `T`, which can be any type; take a function that does it, like `add: (T, T) -> T`".
- **`T` names a type:** `T(...)` is "`T` is a placeholder, so it can't be built here; take a value of it as a parameter instead", and `array[T](n)` is refused because the zeros it fills may not be a `T` ("build a `list[T]` with `add` instead"). `list[T]()`, `map[K, V]()` and `[a, b]` are fine.
- **`pure` and `par for`:** a call through a rule follows the interface's method, which must be `pure` there.
- **Closures** in generic code may take, return and capture `T` values.
- **Field defaults** of a generic struct are checked once and built per copy (`items: list[T] = []`); a field of type `T` can't have a default.

**Which types can be `T`.** Checked wherever a placeholder gets a type, written or worked out:
- **Not an owning type:** "`Sprite` owns a GPU object, so it can't be `T` yet" (an `Arena` likewise).
- **Not an optional:** "`int?` is an optional, so it can't be `T` yet; hold it in a struct field instead".
- **Not a function type, and not a type that holds a function anywhere** (`list[(int) -> int]`, a struct with a callback field): "`(int) -> int` is a function type, so it can't be `T` yet".
- **It meets every rule:** "`Rock` isn't `Damageable`, so it can't be `T` in `total_hp`"; "`Enemy` isn't `equatable` (numbers, bools, strings and vectors are), so it can't be `T` in `count`". An interface type meets its own rule only, so `total_hp` over a `list[Damageable]` works (that copy calls through the interface, an indirect call). Inside another generic, a placeholder meets a rule it carries, else the message adds "; add the rule: `[U: Damageable]`".
- Interface types, lists, arrays, maps and other generic types are fine (`Stack[Damageable]`, `Stack[list[int]]`, `Stack[Stack[int]]`).

**Endless copies.** Each copy is compiled code, so a generic can't need endless copies. The checker refuses it at the declaration, even if nothing uses it yet: "`Tree` would need endless copies: `Tree[T]` holds a `Tree[list[T]]`, which holds a `Tree[list[list[T]]]`, and so on" (help "use the same placeholders (`Tree[T]`), or types that don't grow"), and the same for a function that calls itself with a growing type. A few that go through a rule's method can only be found when copies are made, and stop there at 32 levels of nesting, 1,000 parts in one type, or 10,000 copies of one generic ("making copies of `walk` doesn't end ..."). A type that holds itself through a generic is today's "struct `Bad` contains itself".

**Modules.** `pub struct Stack[T]`, `pub enum Result[T, E]` and `pub best[T]` are imported like any `pub` item (`use { Stack, best from geom }`, or `use geom` and `geom.best(...)`). The copies are made in the program that uses them, once per generic and set of types, whichever modules use it.

**Contracts and the report.** `@prove` on a generic is checked on every copy; an error names the copy and the line that first led to it: "`Stack[string].put` (made by `Stack[string]` on line 12) can't be proven `no_alloc`: ...". A generic nothing uses has nothing to check, and `kestrel check --report` lists it as "`best`: not checked, no copies". Generic functions and their copies aren't exported to Godot; write a non-generic wrapper.

**Cost.** One compiled copy per generic and set of types: code size and compile time grow with the number of distinct types you use. A copy is meant to run like the function you would have written by hand: a call on a `T` with a rule is a direct call the compiler can inline, unless `T` is an interface type, which is an indirect call as always (`kestrel/bench/kernels/results.md`, "Generics").

**What this round changed in the rest of the language** (none of it new syntax):
- **`<`, `<=`, `>`, `>=` work on two strings and two bools everywhere**, not only on numbers (strings by code point, the order `sort()` uses; `false < true`), so a copy of `[T: ordered]` code made for `string` compares strings. A mix is "`<` needs two numbers, two strings or two bools, found string and int".
- **A function may store memory from the caller's arena into a `var` parameter, and the check moves to the call** (before, it was an error in the body). `push(var self, v: string) { self.items.add(v) }` works, and so does a generic `Stack[T].push`. Each function records which of its `var` parameters it stores such memory into, each direct call checks the place passed (as built-in `add` does), and a call through an interface or a rule takes the union of the implementations'. Such a function can't be used as a value. `Stack[int].push` inside an `arena { }` on an outer stack is still refused, as a `Stack[string]`'s is: the check assumes any `T` holds memory.
- **Replacing a whole `var` placeholder** (`a = b`) is refused only when the placeholder may be an interface type (no built-in rule and at most one interface rule): `swap[T: equatable](var a: T, var b: T)` works, `swap[T]` doesn't ("if `T` is never an interface type, give it a built-in rule, like `[T: equatable]`").
- **A `var` argument conflicts with an `if` or `match` argument that gives part of it** (`f(var w, if c { w.s } else { "" })`), as `f(var w, w.s)` already did; copy it into a `let` first.

**Not built:** the `number` rule (arithmetic on `T`); owning types, optionals, function types and types holding functions as `T`; placeholders on methods and interfaces; default types (`[T = int]`); constant placeholders (`Grid[W, H]`); `is` type tests; specialising a generic for one type; Godot exports of generics; some but not all types written (`count_by[_, string]`); working out a generic function value's types from an expected function type (`let f: (list[Enemy]) -> Enemy? = best`); devirtualising a call on an interface `T`.

## Memory

```
arena {                      // temporary
    var tris = list[vec3]()
    ...
}                            // everything made inside is freed here

in chunk.mem {               // owned: lives as long as chunk.mem
    chunk.voxels = array[u8](32768)
}
```

- Allocations go to the **current arena**: the innermost `arena { }`
  or `in x { }` block, including inside functions called from it.
- Outside any block, allocations go to the **program arena**, freed at
  exit.
- **Escape check:** a value allocated in an arena may not be stored
  anywhere that outlives that arena. Error names both lifetimes and
  suggests a copy: `chunk.mesh = tris.copy_to(chunk.mem)`.
- **Assigning over an existing value reuses its storage** when it has
  room: `ring[f % 16] = game` copies the game's lists into the slot's
  own lists when they have the capacity. There is no syntax for it, and
  it is still a deep copy: the two values share nothing afterwards, and
  the program prints exactly what a fresh copy would. What is reused:
  - lists and arrays of plain items (numbers, vectors, plain structs),
    and structs, enums and optionals made of those (an enum or optional
    reuses its payload when both sides hold the same variant);
  - lists whose items hold lists, maps or strings, item by item, but
    only up to the target's old length: a list that grows back into a
    length it had before copies the items past its old length fresh;
  - strings, only when their bytes are equal: a string is kept then, and
    otherwise copied fresh (room has nothing to do with it; a string is
    never written into);
  - maps with plain keys and values: a map with room takes the new keys
    into its own entries and index. String keys and heap values are
    copied fresh each time, and a map being looped over is copied fresh,
    so the loop sees the replacement as before.

  Storage never grows in place: a list or map without room gets a fresh
  copy in the current arena, as before. A reused buffer stays in the
  arena it was made in, and later growth of it (an `add`) goes there
  too; only a copy that doesn't fit goes to the current arena.
  `@prove(no_alloc)` holds for saving and restoring a snapshot with no
  list, map or string; one that holds them keeps the fresh copy for a
  target without room, so the contract names that assignment.
  A target that may share memory with the value (`a = a.child`,
  `ring[i] = ring[j]`), or whose index has a call in it, is copied
  fresh. So a ring of snapshots of plain lists, structs and maps stops
  allocating once each slot has held the largest state; one whose lists
  hold strings, lists or maps and change length, or whose maps hold
  strings or heap values, keeps allocating for those.

## Optional values

```
let hp = scores["ana"] else { return }
```

- `T?` holds a `T` or `none`.
- A map lookup is the usual source of one: `scores["ana"]` on a `map[string, int]` is an `int?`, `none` when the key is missing.
- `let x = expr else { ... }` unwraps; the `else` block must exit
  (`return`, `break`, `continue`).
- Using a `T?` as a `T` without unwrapping is a compile error.

## Grammar sketch

```
file      := (use | decl)*
decl      := 'pub'? (func | struct | enum | interface | 'let' binding | 'var' binding)
func      := 'pure'? 'native'? IDENT tparams? '(' params? ')' ('->' type)? block?
tparams   := '[' tparam (',' tparam)* ']'
tparam    := IDENT (':' rule ('+' rule)*)?    // IDENT starts with a capital; a rule is an interface, equatable, ordered or key
params    := param (',' param)*
param     := 'var'? (IDENT ':' type | 'self')
struct    := 'struct' IDENT tparams? impls? '{' (field | func)* '}'
field     := IDENT ':' type ('=' expr)?
enum      := 'enum' IDENT tparams? impls? '{' (variant | func)* '}'
impls     := ':' IDENT (',' IDENT)*
interface := 'interface' IDENT '{' sig+ '}'
sig       := 'pure'? IDENT '(' params? ')' ('->' type)?
variant   := IDENT ('{' IDENT ':' type (',' IDENT ':' type)* '}')?
type      := IDENT ('[' type (',' type)* ']')? '?'? | '[' type ']' '?'?
           | 'pure'? '(' ('var'? type (',' 'var'? type)*)? ')' ('->' type)?
block     := '{' stmt* '}'
stmt      := 'let' binding | 'var' binding | assign | if | while | for
           | 'par' for | match | arena | in | 'break' | 'continue'
           | 'return' expr? | 'remove' IDENT | expr
closure   := '(' (cparam (',' cparam)*)? ')' ('->' type)? block
cparam    := 'var'? IDENT (':' type)?
binding   := IDENT (':' type)? '=' expr ('else' block)?
for       := 'for' IDENT ('from' expr 'to' expr | (',' IDENT)? 'in' expr) block
arena     := 'arena' block
in        := 'in' expr block
match     := 'match' expr '{' (pattern block)* '}'
pattern   := IDENT IDENT? | 'else'
```

A `native` function has no block, and every other function has one. `native` is only allowed at the top level of a built-in module.

## Checks the compiler leaves out, and `kestrel check --report`

Design: `2026-10-19-range-analysis-design.md` (with "As built"). Built 2026-10-20.

- **Every integer `+ - *`, every `xs[i]`, `/`, `%`, shift and narrowing conversion (`int(x)` of an `i64`) is checked** in a normal build, and the compiler **leaves out each check it proves can't fail** from the ranges of the values and from what earlier checks, loop bounds and conditions showed: `for i from 0 to xs.len` proves `xs[i]`, `h & 0x7FFFFFFF` times a 31-bit constant proves no overflow, a divisor guarded `> 0` proves the division. Nothing a program does changes: the same output, and the same error on the same line wherever a check could fire. When the compiler isn't sure, the check stays.
- **`--release`** turns off the arithmetic checks as before (they wrap), and keeps the bounds and division-by-zero checks; the analysis proves those too.
- **What it doesn't prove yet:** `xs[y * w + x]` from `y < h`, `x < w` and `xs.len == w * h` (a product), a length a function returns, an integer field read from memory, and a check inside a loop that doesn't depend on the loop. `@assume` (next section) lets a program state a fact the analysis can use for the first two, though it doesn't use a product as a term of a fact yet; the report and the warning show whether it removed a check.
- **`kestrel check --report file.kes`** (and `--report --release`) prints, to standard output, every check, allocation, other way to fail (a list index in `remove_at`, `insert` or `s[i]`, a list growing past its limit, a native function, a call through a function value) and call that wasn't inlined, grouped by file, function and loop, with its line and why it stayed ("overflow check: `total + xs[i]` may exceed `int` (`total` can be any `int`)"), and a total. A call to a function that allocates or can fail is an item of its caller, with the chain to the reason. `--report` goes only with `check`, and a program with errors gets no report. `KESTREL_CHECK_PROOFS=1` in the environment of `kestrel run` or `build` keeps every removed check as an internal-error check (for testing the compiler).

## `@` annotations: `@prove` and `@assume`

Design: `2026-10-18-at-annotations-design.md` (with its Round B addendum and "As built"). Built 2026-10-21. Keywords are for structure (`pure`, `pub`); `@` is only for promises the compiler must prove and facts it may use.

```
@prove(no_alloc, no_checks)
pure step(var p: Particle, dt: float) { ... }

@prove(no_checks)
for i from 0 to n { grid[i] += 1 }

pure at(xs: [int], i: int) -> int {
    @assume(i >= 0 and i < xs.len)
    xs[i]
}
```

- **`@prove(a, b)`** takes one or more of `no_alloc`, `no_checks`, `no_panic`, `inline` and `deterministic`, and goes before a function (before `pub` and `pure`), a method of a struct or enum, an interface method, or a `for`, `par for` or `while` loop. Several `@prove`s before one target merge. Nothing else takes one: not a `let`, an `if`, a struct, a closure literal or a native function. The annotated thing may follow on the same line or on the next; a blank line or a comment line between is an error, so an annotation never attaches to something further down. **(An open syntax pick: the user hasn't ruled on whether the same line is allowed, or on comment lines.)**
- **A contract fails the build** (`kestrel check`, `run` and `build` exit 1, nothing runs) when it doesn't hold. It is decided on the compiled code of the build you asked for (`--release` or not), after inlining and the range analysis, and the error shows the chain: "`step` can't be proven `no_alloc`: it calls `spawn` (line 40), which adds to `ps` (line 12) and may grow the list". `kestrel check --report` still prints when only a contract fails: it is the way to see what is left.
  - `no_alloc`: nothing allocates, the runtime's own allocations included (`sort`, `sort_by`, list growth, copies of heap values, `arena { }`, stored closures, boxed interface values). A `par for` always fails it, because it makes an arena for each thread; put `@prove(no_alloc)` on the function its body calls. **`print` isn't counted** (a ruling, 2026-10-21): the runtime makes a thread's output buffer on its first print, and the Godot add-on's captured output grows on every call, so a `no_alloc` function that prints can allocate.
  - `no_checks`: no overflow, shift, division, narrowing or bounds check is left. Under `--release` that means no bounds check or division check, since those are the checks `--release` keeps.
  - `no_panic`: `no_checks`, and no other way to fail the report lists (`remove_at`, a native, a map changed in its loop, `array[T](n)` with a bad `n`, an `@assume` that is checked).
  - `inline`: every call in it was inlined, so each call left is named with the inliner's reason.
  - `deterministic` (the determinism round, 2026-10-27): its result, and every change to its `var` parameters, depends only on its arguments. Three things break it: any native, in words chosen by the native ("it calls `window.delta` (line 14), which reads the clock"; `input.held`, `pressed`, `released` read the keyboard; `input.mouse`, `wheel`, `mouse_delta` read the mouse; any other native "talks to the window, which a deterministic function can't depend on"); the program's shared random generator (`randf`, `randf_range`, `randi_range`, `rand_seed`: "which uses the program's shared random generator"); and a call through a function value whose type isn't `pure` ("which isn't `pure`") or through an interface whose method is neither `pure` nor promises `deterministic`. Each error has a help line that says what to do instead (pass the time step in, read input outside and pass it in an `Inputs` struct, keep an `Rng` from the `random` module in the game state, give the function value a `pure` type). **`print` is allowed** (a rollback re-runs it, so lines repeat), and so are math, maps, sorting and `par for`. **Every `pure` function holds it** (no `pure` natives exist; a future one would need marking): a written rule from now on. Range analysis giving up on a function doesn't break it; only a function whose code nests too deeply to walk does.
  - **It covers what it calls** (through the call graph, with the chain in the error) **and every copy the inliner made of it**: a function's contract holds in its own compiled code and in each place it was inlined, and an error in a copy says where ("where it is inlined into `step` (line 40)"). A loop's contract covers its header (`for x in xs.filter(...)` builds a list) and every iteration. A function value (a parameter, a field, a stored closure) breaks the first four contracts, because its body isn't known there; contracts on function types wait. A call through an interface keeps exactly what its interface method promises, and every implementation is checked against the interface method's contract ("`Goblin.area` can't be proven `no_alloc`, which `Shape.area` requires: ..."). Natives break `no_alloc`, `no_panic` and `deterministic`. A function range analysis gave up on, or ran out of depth in, fails every contract that reaches it (`deterministic` only when its code nests too deeply).
- **What `no_panic` doesn't count:** running out of memory; a list or map growing past its item limit (2,147,483,647: `add`, `insert`, a map key set; use `no_alloc` to rule growth out); and, under `--release`, a false trusted assumption's divide fault (a trusted `@assume` that is false can remove a division's zero check, and the division then crashes the process instead of ending with a clean error; it can never make a program read outside a list). **Recursion fails `no_panic`** (the stack may run out; but printing or copying a value of a recursive type, such as `struct Node { kids: list[Node] }`, recurses in code the compiler generates, which no contract sees, so a deep enough value can overflow the stack in a `no_panic` function), and so does an `@assume` that is checked: a function-level `@prove(no_panic)` and an `@assume` in that function can't both hold in a checked build. Put the contract on a loop and the assumption before it. Moving the assumption to the caller doesn't help: facts don't cross calls, so the function's own copy is still checked with its parameters anywhere in their types' ranges.
- **`@assume(cond)`** is a statement, anywhere a statement may go inside a function, a method or a closure. It covers the rest of its block, up to the `}` (like a `let`); the code outside the block is analysed as if it weren't there. Several in a row, with no statement between them, are one assumption (`@assume(a) @assume(b)` is `@assume(a and b)`).
  - **The condition** is comparisons (`<`, `<=`, `>`, `>=`, `==`, `!=`) of integers, joined with `and`. Each side is made of names (locals, parameters, constants), fields of them, integer literals, `.len`, `+`, `-`, `*`, unary `-` and parentheses; no calls and no indexing. **(An open syntax pick: the user hasn't ruled on how narrow the sides are. The cost: `grid[y].len` can't be assumed.)** Anything else is an error that says so. **A component of an `ivec4` is allowed but teaches nothing:** `@assume(v.x < 10)` on an `ivec4` gets the "removes no check" warning below, because range analysis keeps one range for all four components; on an `ivec2` or `ivec3` component it teaches as on any `int`.
  - **In a checked build** each condition is checked where it is written, every time it runs; a false one ends the program: "assumption `n <= xs.len` is false on line 9 of f.kes" (exit 101).
  - **Under `--release`** an assumption that only removes overflow, shift or division checks is trusted and not evaluated. One that removes a bounds check stays one check where it is written (one check instead of one per iteration), so `@assume` can't make a program read outside a list.
  - **`kestrel check` warns** when an assumption removes nothing in either build: "`@assume(n <= grid.len)` removes no check; delete it". The warning never fails anything.
  - **What the analysis uses** is what `if cond` would teach it on its true branch: nothing more, so a condition that contradicts what the compiler already knows proves nothing. Facts die with the values in them, as everywhere (an assignment, a `var` argument or a change to a list's length inside the block). An assumption about a caller's value doesn't reach into a function it calls.
- **Errors you can get:** "unknown annotation `@fast`; Kestrel has `@prove` and `@assume`"; "unknown contract `fast`; `@prove` takes `no_alloc`, `no_checks`, `no_panic`, `inline` and `deterministic`"; "`@prove` goes before a function, a method or a loop (`for`, `par for` or `while`)"; "`@prove` goes on the line right before what it applies to, or on the same line"; "`@assume` goes inside a function, as a statement of its own"; "`@assume` takes comparisons of integers joined with `and`, like `@assume(i < xs.len and n >= 0)`"; "`@assume` can't call a function or index a list (a `--release` build may not run it); compare names, fields, numbers and `.len`".
- **Grammar:** `annot := '@prove' '(' IDENT (',' IDENT)* ')' | '@assume' '(' expr ')'`; an annotation goes before an item, a method, an interface `sig`, or a `for`, `par for` or `while` statement, and `@assume` is a statement of its own.
- **Not built yet:** contracts on function types, natives marked safe, `@prove` on a native in a built-in module, contract warnings in `kestrel run` and the Godot add-on, facts across calls (an assumption at a call site doesn't reach the callee), and indexes in an assumption (`grid[y].len`). The follow-ups file lists them.

## Out of scope (later specs)

User-defined generics are built (see "Generics"; what they leave out is under "Not built" there, and in the generics spec's "Not this round"). Still out of scope: `is` type tests on interface values (and default method bodies, fields and inheritance between interfaces), error types beyond `?`, fixed-size arrays,
C FFI declarations, GPU/shader code. Also not yet: the other sized integers `i8`, `i16`,
`u16`, `u32` and `u64` (the integers are `int`, `i64` and `u8`), optional function types,
methods and built-ins as values, `reduce`, `reverse`, a comparator sort.

## Resolved in review

- Call-site `var` marker: yes (receiver exempt).
- Logic operators: `and` / `or` / `not` only.
- Index + value loop `for i, v in xs`: included.
