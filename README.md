# Kestrel

A compiled language for games: plain values, arenas instead of a garbage
collector, and parallel loops (`par for`) over `pure` code.
It compiles through Cranelift to native code, either JIT-run in place or
built into an executable.

```kestrel
struct Chunk {
    x: int
    mem: Arena = Arena()
    heights: [int] = []

    generate(var self) {
        in self.mem {
            self.heights = array[int](32 * 32)
        }
        for i, h in self.heights {
            self.heights[i] = self.x + i % 32
        }
    }
}

main() {
    var world = list[Chunk]()
    for x from 0 to 4 { world.add(Chunk(x: x)) }
    for chunk in world { chunk.generate() }
    print("chunks: {world.len}, first: {world[0].heights[1]}")
}
```

Lists and strings have the everyday methods, named after GDScript's (`remove_at`, `pop`, `insert`, `clear`, `has`, `find`, `join`, `split`, `slice`, `upper`, `trim`, `to_int`). `remove b` inside a `for b in xs` loop removes the current item without skipping the next one, which is where `erase` in a GDScript loop goes wrong:

```kestrel
main() {
    var bricks: list[int] = [1, 2, 3, 4, 5, 6]
    var score = 0
    for b in bricks {
        if b % 2 == 0 {
            score += b
            remove b
        }
    }
    print(bricks, score)    // [1, 3, 5] 12
}
```

Strings are UTF-8, and lengths and indexes count characters, so `"héllo".len` is 5. Methods that can fail, like `find` and `to_int`, return an optional (`none`) where GDScript returns -1 or 0:

```kestrel
main() {
    var words: list[string] = []
    for part in "go, stop ,  jump".split(",") {
        words.add(part.trim().upper())
    }
    print(words.join("-"), words.len)       // GO-STOP-JUMP 3
    let name = "héllo"
    print(name.len, name[1], name.slice(1, 4), name.find("l"))    // 5 é éll 2
    let n = " 42 ".to_int() else { return }
    print(n + 1)                            // 43
}
```

The full list is in the "Lists and strings" section of `docs/superpowers/specs/2026-10-09-kestrel-syntax-design.md`.

Functions are values, and closures capture by value, as in GDScript. Lists have `filter`, `map`, `any`, `all`, `sort` and `sort_by`. A closure literal passed straight to one of these (or to a function) allocates nothing, and the compiler inlines it into the loop. A closure can't change what it captured, so `for` is still how you accumulate:

```kestrel
make_adder(k: int) -> (int) -> int {
    (x) { x + k }
}

main() {
    let add2 = make_adder(2)
    var hp: list[int] = [30, 0, 12, 7]
    let alive = hp.filter((h) { h > 0 })
    print(alive.map(add2))                  // [32, 14, 9]
    hp.sort_by((h) { -h })                  // largest first
    print(hp, hp.any((h) { h == 0 }))       // [30, 12, 7, 0] true
}
```

Sorting is stable, strings sort by code point, and `sort_by` keys must be `pure`. Methods, built-ins and natives aren't values yet; wrap them in a closure. The details are in the "Closures and function values" section of the same file.

Interfaces name a set of methods that several structs or enums can promise to have, so one list can hold goblins and barrels and a call runs the right method for each (a method table, like a vtable). The compiler checks the promise, where GDScript's duck typing (`has_method`) checks it when the line runs. A struct passed straight to an interface parameter is lent, not copied; storing one makes a copy in the current arena, like a string:

```kestrel
interface Damageable {
    take_damage(var self, amount: int)
    pure hp(self) -> int
}

struct Goblin: Damageable {
    health: int = 10
    take_damage(var self, amount: int) { self.health -= amount }
    pure hp(self) -> int { self.health }
}

struct Barrel: Damageable {
    oil: int
    take_damage(var self, amount: int) { self.oil -= amount * 2 }
    pure hp(self) -> int { self.oil }
}

show(d: Damageable) { print(d.hp()) }       // d is the caller's Goblin, not a copy

main() {
    var foes: list[Damageable] = [Goblin(), Barrel(oil: 4)]
    for f in foes { f.take_damage(2) }
    print(foes.map((f) { f.hp() }))         // [8, 0]
    print(foes)                             // [Goblin(health: 8), Barrel(oil: 0)]
    let g = Goblin()
    show(g)                                 // 10
}
```

A struct that owns an `Arena` or a GPU object can be lent to an interface but not stored as one. There is no `is` test to get the struct back out yet. A call through an interface is an indirect call that isn't inlined: about 5 ns where a direct call is under 1, and about 15% behind Rust's `Box<dyn Trait>` (`kestrel/bench/kernels/results.md`). The details are in the "Interfaces" section of the syntax file.

Structs, enums and functions can take types in square brackets, as `list[T]` does. In GDScript terms: `Array[Enemy]` is a typed array, and this lets you write your own `Stack[T]`, which GDScript can't; untyped GDScript gets the same reuse from `Variant`, which is boxed and checked when the line runs. A Kestrel generic is checked once, where it is written, against rules you give its placeholder (an interface, or the built-in `equatable`, `ordered` and `key`), and a call on a `T` can only use what its rules promise. The compiler then makes one compiled copy for each type you use, so there is no boxing and no type tag:

```kestrel
struct Stack[T] {
    items: list[T] = []
    push(var self, v: T) { self.items.add(v) }
    pop(var self) -> T? { self.items.pop() }
}

pure total_hp[T: Damageable](xs: list[T]) -> int {
    var sum = 0
    for x in xs { sum += x.hp() }
    sum
}

main() {
    var names = Stack[string]()             // types written
    names.push("a")
    var ints: Stack[int] = Stack()          // or worked out
    ints.push(3)
    print(ints)                             // Stack[int](items: [3])
    print(total_hp([Goblin(), Goblin()]))   // T is Goblin: direct calls, inlinable
}
```

**What it costs.** One compiled copy per type: the code size and the compile time grow with the number of distinct types you use a generic with (`Stack[int]` and `Stack[string]` are two sets of methods), and `kestrel check` makes the copies too (a program with no generics pays nothing). A copy runs like the function you would have written by hand: in the benchmark the generic loop takes 0.56 ns per item against 0.55 for the hand-written one (identical machine code), about 9 times faster than the same loop over an interface list (`kestrel/bench/kernels/results.md`, "Generics"; measured on a busy machine, taking turns). The full test suite passes (1,453 tests). A `T` that is an interface type (`total_hp` over a `list[Damageable]`) makes an indirect call, as before. **What it doesn't do yet:** arithmetic on a `T` (there is no `number` rule; take a function instead), owning types, optionals and function types as `T`, placeholders on methods and interfaces, default types, writing only some of the types, and Godot exports of generics. The details are in the "Generics" section of the syntax file.

Two things changed elsewhere with it: `<` now works on strings and bools (so `[T: ordered]` code works for them), and a function can store a string into its `var` parameter, with the check made at the call (so `push(var self, v: string)` works).

## Use

Needs Rust (stable) and, on Windows, Visual Studio's C++ build tools (for `link.exe`).

```bash
cd kestrel
cargo build --release
target/release/kestrel run program.kes        # compile in memory and run
target/release/kestrel build program.kes -o program.exe
target/release/kestrel check program.kes      # errors only
target/release/kestrel check --report program.kes   # what is left in each function and loop
```

On Linux x64 the same commands build the compiler and `kestrel build` exes (linked with `cc`), without the window: a program that uses `window`, `draw`, `draw3d` or `input` is refused there with a sentence saying so. CI (`.github/workflows/ci.yml`) runs clippy and the tests on Linux and Windows.

`kestrel build --release` turns off overflow and narrowing checks (they wrap instead). `kestrel run --release` does the same.

**The compiler leaves out each check it can prove won't fail.** `for i from 0 to xs.len { total += xs[i] }` has no bounds check on `xs[i]`, and `h & 0x7FFFFFFF` times a 31-bit constant has no overflow check, in every build. `kestrel check --report` shows what stayed, per function and loop, with the line and the reason, and the allocations and other ways to fail (`--report --release` for a `--release` build):

```
sum.kes
  function `sum_all` (line 1): its own copy is never called; its checks are listed where it is inlined
  function `main` (line 9): 1 check left (2 removed), 1 allocation
    loop at line 3 (in `sum_all`)
      line 4 (in `sum_all`): overflow check: `total + xs[i]` may exceed `int` (`total` can be any `int`; `xs[i]` can be any `int`)
    line 10: allocation: `[...]` makes a new list
total: 3 checks, 2 removed, 1 left; 1 allocation; no other ways to fail
```

A list or an array holds at most 2,147,483,647 items.

**Modules in folders.** `use geom` loads `geom.kes` next to the file, and `use net.packet` loads `net/packet.kes` from the folder `net/` beside it, whose items are then `packet.make()` or `net.packet.Packet` (the rules are in "Imports and visibility" in the syntax file).

**`@prove` and `@assume`.** `@prove` is a promise the compiler checks, and a build that breaks it fails with the chain of calls that broke it. `@assume` tells the compiler a fact it can use, and checks it where it is written.

```kestrel
@prove(no_alloc, no_checks)
pure step(var p: Particle, dt: float) { ... }

pure at(xs: [int], i: int) -> int {
    @assume(i >= 0 and i < xs.len)
    xs[i]
}
```

- `@prove` takes `no_alloc` (nothing allocates), `no_checks` (no overflow, shift, division, narrowing or bounds check is left), `no_panic` (no check and no other way to fail) and `inline` (every call was inlined). `print` isn't counted by `no_alloc` (the runtime makes a thread's output buffer on its first print, and in the Godot add-on output is captured and grows on every call). It goes before a function, a method, an interface method, or a `for`, `par for` or `while` loop, and covers what the code calls and every place it was inlined. `kestrel check --report` lists what is left in it.
- `@assume(cond)` holds to the end of its block. A checked build checks it and a false one stops the program ("assumption `i < xs.len` is false on line 3 of f.kes"); under `--release` one that only removes arithmetic checks is trusted, and one that removes a bounds check stays one check. `kestrel check` warns when an assumption removes nothing.
- `@prove` and `@assume` make `kestrel check` run the whole analysis, so it takes about 50-80 ms longer on the voxel world, and about 250 ms longer with an `@assume` around a big block (the inlined noise code: `check` then analyses both builds, running the block twice in each); that assumption makes the build itself 65-70% slower there, so put one near what it helps. `kestrel/bench/voxels3d/results.md` has the numbers.

**Vectors and wrapping operators.** `vec2`, `vec3`, `vec4` and `ivec2`, `ivec3`, `ivec4` are Godot's `Vector2`/`Vector3`/`Vector4` and `Vector2i`/`Vector3i`/`Vector4i`, with one set of rules. `vec4` and `ivec4` are real SIMD values: one 16-byte register, and `+`, `*`, `^`, `>>` and the rest do all four lanes in one instruction (the CPU needs SSE4.1, which every x64 CPU since about 2011 has). `vec4(0.5)` fills all four components, `vec4(v3, w)` and `vec4(a.xy, b.zw)` join parts, `v.wzyx` swizzles, `v.sum()` adds the lanes, and `min`, `max`, `clamp`, `abs` and `sqrt` work lane by lane. `==` gives one `bool` (every lane equal), and `<` doesn't exist on vectors. An `ivec4` has no `/` or `%` (the CPU can't divide integer lanes at once: use `>>` for a power of two, or divide each lane). An `ivec4` is checked per lane like an `int`: a checked build stops on an overflowing lane, `--release` wraps, and range analysis removes the checks it can prove. **`*% +% -%` wrap and are never checked**, in any build, which is what a hash wants (GDScript's `int` is 64-bit and wraps silently; Kestrel's `*` stops on overflow, so `*%` is how you ask for the wrap). Hex literals may use their type's whole width, so `0x9E3779B1` is an `int`:

```kestrel
hash(h: ivec4) -> ivec4 {
    let a = (h ^ (h >> 15)) *% 0x2C1B3C6D
    (a ^ (a >> 12)) *% 0x297A2D39
}

main() {
    let pos = vec4(1, 2, 3, 4)
    let moved = pos + vec4(0.5) * 2.0
    print(moved, moved.sum(), moved.wzyx)   // vec4(x: 2, y: 3, z: 4, w: 5) 14 vec4(x: 5, y: 4, z: 3, w: 2)
    let h = hash(ivec4(10, 20, 30, 40))     // four hashes at once
    print(h.x != 0)                         // true
    var n = 1_000_003
    n *%= 0x9E3779B1                        // wraps, never checked
    print(n)                                // -683443373
}
```

On a 2D value noise, four samples at once in `vec4`/`ivec4` run 2.5 times faster per sample than the scalar version, and take 1.08 times as long as a hand-written Rust SSE4.1 version (SSE, not AVX; Rust's own four-at-once version is 2.6 times its scalar one), and a checked build costs nothing measurable because the analysis proves every lane check (`kestrel/bench/kernels/results.md`, "SIMD vectors"). Four corners of one sample at once gain only about 1.25 times. Natives can't take `vec4` or `ivec4` yet, and Godot exports of them aren't built. The rules are in the "Vectors" bullet of the syntax file.

## Editor

VS Code (or any editor that speaks the Language Server Protocol) gets Kestrel's real errors as you type, the type of what is under the cursor, go to definition (into module files and the built-in modules too), and the **cost lens**: every check, allocation and other way to fail that the compiler leaves in a line, shown on that line with the reason, and the `@assume` or wrapping operator that removes it when there is a simple one. It is `kestrel check --report` in the editor, and runs when a file is opened or saved.

```text
pub get(xs: list[int], k: int) -> int {
    xs[k]    bounds check: `xs[k]` may be outside `xs` (`k` can be any `int`)
}
```

To install it locally (it isn't on the marketplace), from `kestrel/`:

```bash
cargo build --release -p kestrel-lsp
cd editors/vscode
npm install
npx @vscode/vsce package --skip-license --allow-missing-repository
code --install-extension kestrel-0.1.0.vsix
```

Then set `kestrel.server.path` to the built `kestrel/target/release/kestrel-lsp.exe` (or put that folder on `PATH`) and reload VS Code. The settings `kestrel.costLens.enabled`, `kestrel.costLens.build` (`checked` or `release`) and `kestrel.costLens.show` (`hints`, `diagnostics` or `both`) control the lens. A file with `pub` items and no `main` is checked as a module. [`kestrel/editors/vscode/README.md`](kestrel/editors/vscode/README.md) has the details and what isn't built yet.

## A window

Four modules are built into the compiler (raylib underneath), and work under both `kestrel run` and `kestrel build`. Windows only for now.

- `use window`: open the window, run the frame loop, time, screenshots.
- `use draw`: shapes (`draw.rect`, or `draw.fill` for a `Rect`), text, textures and sprites, and `draw.rects` to draw thousands of rectangles in one call.
- `use draw3d`: a camera, meshes you build in Kestrel (`MeshData`, like filling an `ArrayMesh`) and upload to the GPU once, and a few shapes.
- `use input`: keyboard and mouse.

A game written as two functions, `pub setup() -> Game` and `pub frame(var g: Game) -> bool`, reloads while it runs: save a file and `kestrel run` recompiles it on the side and swaps the new code in between two frames, keeping the state (score, world, meshes) when its shape did not change, or restarting with one sentence when it did. A compile error keeps the old code running. `main` programs are untouched; `--no-reload` turns it off.

```kestrel
use window
use draw

main() {
    window.open(640, 360, "Hello")
    while window.frame() {
        draw.clear(draw.BLACK)
        draw.circle(vec2(320, 180), 40, draw.ORANGE)
        draw.text("fps {window.fps()}", vec2(10, 10), 20, draw.WHITE)
    }
}
```

The showcase is a particle demo: tens of thousands of dots pulled toward the mouse, moved by a `par for`. Run it from `kestrel/`:

```bash
target/release/kestrel run --release demos/particles/particles.kes
```

Move the mouse to pull the particles. Space adds 1,000 more. The window shows the update and draw times, and the demo prints them once a second. It draws all the dots with one `draw.rects` call: 200,000 dots draw in about 4.6 ms (a frame takes 5.4 ms), against about 29 ms when each dot was its own `draw.rect` call. `kestrel/demos/particles/results.md` has the numbers, and `kestrel/bench/particles/results.md` compares the demo with a Rust port on the same raylib (Kestrel is 2-7% behind there).

The other showcase is a voxel viewer: fly through the 8x4x8-chunk world of `bench/voxels3d` (463,749 faces) in 3D. The top block of each column is grass (snow on the peaks), with dirt under it and rock deeper down, and soft shading in the corners where blocks meet, all baked into the vertex colours (there is no lighting).

```bash
target/release/kestrel run --release demos/voxels/voxels.kes
```

- Hold the right mouse button and drag to look, WASD to move, Space and Shift to go up and down, the mouse wheel to change speed.
- At start a `par for` builds every chunk's mesh on all cores (about 80 ms, estimated) and a loop copies them to the GPU (about 55 ms). Then each frame draws all 248 meshes, with an overlay of the fps, frame time and face count.
- At the default size it holds 60 fps, the display's rate. That number is capped: every hidden window was held at 60 fps while the viewer was measured, so the headroom was found by growing the world (the constant `WIDTH` in `voxels.kes`). 60 fps (capped) still holds at 4.2 million faces, 9 times the world, and drops just below at 7.3 million. `kestrel/demos/voxels/results.md` has the numbers and what limits it.

The voxel engine builds on that viewer: an endless world you walk or fly through, that loads and unloads around you, and where you break and place blocks. It is written in Kestrel in `kestrel/voxel/` (version 1, built and reviewed 2026-10-26). Run it from `kestrel/`:

```bash
cargo run -q --release -p kestrel-cli -- run --release voxel/main.kes
```

- WASD moves, Space jumps (walking) or goes up (flying), Shift goes down, F switches walking and flying, left click breaks a block, right click places stone, and Tab captures the mouse for first-person look (or hold the right button and drag). `kestrel/voxel/README.md` has the controls, the files (libraries such as `Grid[T]`, `Ring[T]`, `Pool[T]`, a ray caster, box collision and a view frustum, then the engine) and how to run its tests and benchmark.
- It holds its frame budget while streaming: the 99th-percentile frame work time is 15.0 to 15.8 ms (the pass mark is 16.7 ms) over 20 and 100 blocks a second and a 300 s circle, memory is flat (60.7 MB program arena, working set up 2.2 to 2.3% over the last 200 s), and an edit shows in the frame it was made (0.71 ms for a block in a chunk's middle, 1.82 ms at a chunk corner). The generic `Grid[u8]` costs 2.4% against the hand-written frozen viewer.
- Against Rust's `block-mesh` crate on the same input, Kestrel's block-face meshing takes 1.39 times Rust's time for the faces and 1.84 times for the faces plus mesh arrays (1.36 and 1.78 against Rust built for this CPU). `kestrel/bench/engine/results.md` has every number, how it was measured and the machine's load; the findings the build turned up are in `docs/superpowers/plans/2026-10-09-core-2a-followups.md`.
- The window has not been played by a person yet: the engine was built and tested headless, so the feel (mouse look, the text on screen) is still to be checked.

Two environment variables help with testing:

- `KESTREL_HEADLESS=<frames>` opens no window and runs that many frames, printing each `draw.*` call as a line instead (`KESTREL_HEADLESS_QUIET=1` silences them).
- `KESTREL_HIDDEN=1` opens a real window that isn't shown.

The API is in `docs/superpowers/specs/2026-10-10-window-design.md` (window, shapes, text, input) and `docs/superpowers/specs/2026-10-10-3d-textures-design.md` (textures, `draw.rects`, `draw3d`).

## Determinism

Kestrel can promise, and the compiler checks, that a function gives the same bits for the same inputs on every CPU and at any thread count. `@prove(deterministic)` refuses a function that reaches a source of nondeterminism (the shared `randf`, a clock, a window or input read) and checks that the state it changes can be copied and compared. The math functions (`sin`, `pow` and the rest) are Kestrel's own, not the C runtime's, so their results don't change with the machine, and the float mode is pinned. `random` has `Rng(seed:)`, a seeded generator whose state is a plain value, and `checksum(x)` is a content hash of any value that owns nothing. Assigning a list, string, struct, enum or map over an existing one reuses its storage when it has room, so a game can save and restore its state every frame without allocating. Together these give a game author rollback multiplayer, replays and rewind, with a checksum to find the frame where two peers drift apart.

`kestrel/demos/pong/` is a rollback Pong: two peers in one process over a simulated network with delay, jitter and loss, prediction and rollback, and an in-memory replay. Run it from `kestrel/` with `cargo run -q -p kestrel-cli -- run demos/pong/pong.kes` (W and S move, Tab shows the other peer, R replays and says whether the replay matches). The cost, the measurements and the method are in `kestrel/bench/determinism/results.md`: the own math is 1.1 to 2.7 times slower per call than the C runtime's, and a snapshot of a 58 MB voxel pool takes 19 ms where it took 49.

**Rollback.** The built-in `rollback` module is that netcode as a library for any two-player game whose `step` is deterministic: a game implements `Rollback` with one method, `pure step(var self, one: Input, two: Input)`, and drives a `Session` from `new_session(start, me: 1, delay: 2)` with five calls a frame: `remote(p)` for each packet that arrived, `local(i)`, `advance()`, `outgoing()` for the packet to send, and `desync()`. Kestrel has no `if let`, so the desync report is read in a helper: `let why = s.desync() else { return }`, then `print(why)`. `FakeNet` simulates a network in one process, and `Replay` with `check_replay` records the confirmed inputs and runs them again to check the checksums. Pong is written on it in 498 lines and two files (it was 836 lines in five) and prints the same bytes as before; like every built-in module, `rollback` isn't available in the Godot add-on.

## Files and replays

Any value a snapshot can copy turns into bytes and back: `to_bytes(game)` gives a `list[u8]` with a header naming the type's shape, and `from_bytes[Game](bytes)` reads it again or gives `none` for damaged or foreign bytes (never a crash). `first_difference(a, b)` names the first field where two values differ (`ball.x: 1.5 and 1.25`). Four small built-in modules need no window: `file` (`read`, `read_text`, `write`, `write_text`, `exists`, `list_dir`, `make_dir`, `remove`, `error`), `time` (`now`, `unix`, `sleep`), `args` (`all`, `program`) and `env` (`get`). Paths resolve next to the program.

Any window program can be recorded and replayed with no code: `kestrel run game.kes --record run.replay` logs what the window, input and clock natives returned and what the program printed; `kestrel replay game.kes run.replay` runs it again without a window and says `replay matches (N frames, M lines)` or where the output first differed. One `.replay` file is the bug report. Pong writes `pong.replay` at the end of a run and checks a file passed as its argument (`kestrel run demos/pong/pong.kes pong.replay`); the voxel engine saves on F5 and loads on F9 (`world.save`: seed, player and edits).

## Layout

- `kestrel/`: the compiler, a Cargo workspace.
  - `syntax`: lexer, parser, AST and diagnostics.
  - `check`: names, types, purity, moves, exclusivity and the arena escape check.
  - `kir`: a typed, structured middle layer, plus lowering and inlining.
  - `codegen`: Cranelift JIT and object files.
  - `runtime`: arenas, printing and panics.
  - `window`: the window, drawing and input natives (raylib, vendored in `window/raylib`).
  - `driver`: loading modules and compiling a program, shared by the CLI and the Godot add-on. The built-in modules (`window`, `draw`, `draw3d`, `input`, `random`, `rollback` and the rest) are in `driver/std`.
  - `cli`: the `kestrel` command.
  - `lsp`: the language server, `kestrel-lsp`.
  - `editors/vscode`: the VS Code extension (highlighting, and the client that starts `kestrel-lsp`).
  - `godot`: the Godot add-on and its demo project (built separately).
- `kestrel/demos/particles/`: the particles demo and its measured times.
- `kestrel/demos/pong/`: the rollback Pong demo (`pong.kes` and `game.kes`, on the built-in `rollback` module).
- `kestrel/demos/voxels/`: the voxel viewer (`voxels.kes`, `world.kes`, `fly.kes`) and its measured times.
- `kestrel/voxel/`: the voxel engine (libraries, engine, `main.kes`, its tests and benchmark); its measured times are in `kestrel/bench/engine/results.md`.
- `kestrel/tests/programs/` and `kestrel/tests/errors/`: example programs with their expected output, and expected errors.
- `kestrel/bench/`: benchmarks against Rust and C, with results; summary in `BENCHMARKS.md`.
- `docs/superpowers/specs/`: the designs: the language (syntax), the compiler (core), the window, and textures and 3D.
- `docs/superpowers/plans/`: the implementation plans, done so far.

## Status

The core language works: functions, structs with methods, enums and `match`, vectors, strings, arrays, lists, `for in`, optionals, modules, bit operators, arenas, owning values that move, and checked arithmetic.

It also runs inside Godot 4: see [`kestrel/godot`](kestrel/godot/README.md). GDScript calls Kestrel functions, and edits hot-reload.

Also built: closures and function values with the list methods that take them, interfaces (checked method sets with dynamic dispatch), `map` (the hash map), `par for` (loops that run on every core over `pure` code), range analysis (checks the compiler proves can't fail are left out, and `kestrel check --report` lists the rest), `@prove` performance contracts and `@assume`, generics (structs, enums and functions with rules, built 2026-10-25, reviewed and tested), global math and random numbers, determinism (`@prove(deterministic)`, `Rng`, `checksum`, storage-reusing assignment, Kestrel's own math; built 2026-10-28), and the window above with textures, 3D and the voxel viewer. Small structs now stay in registers, and `vec4`/`ivec4` are SIMD values with the wrapping operators `*% +% -%` (built 2026-10-24). The voxel engine version 1 is built on top of generics (reviewed, with the full suite at 1,472 tests passing, 2026-10-26). Since then: fast float math (Kestrel's own wide functions, inlined, 2026-10-29), the I/O round (`to_bytes`/`from_bytes`, the `file`, `time`, `args` and `env` modules, `--record` and `replay`, 2026-10-30), hot reload that keeps state (`setup`/`frame` programs, 2026-10-31), and the rollback library with modules in folders (`use rollback`, `use net.packet`, 2026-11-01). The performance round (2026-11-01) made compiled code faster without changing a bit of any deterministic program's output: small `if`s become selects, list headers and indexed elements stay in registers across loops, loop-invariant checks move before their loop, number sorts compute each key once and radix sort, and `--release` turns accumulator recursion like `fib` into a loop. Kestrel also builds and runs its tests on Linux x64 without the window (see the build commands above), and CI checks every push on Linux and Windows. Next: the language round (`exit`/`fail`, errors as `Result` with `try`, interface placeholders, inferred `pure`, packages), a showcase netplay demo with a tutorial, the GPU round (`par gpu for` and Kestrel shaders), and then the second round of the voxel race against `block-mesh` (greedy meshing, which needs bit-scan and popcount operations).

Speed versus Rust is within about 1.0x to 1.2x on tight scalar loops, struct updates and SIMD noise, and clearly behind on recursion (fib 2.4x), integer division (collatz 2.3x) and the voxel heightmap milestone (2.1x). The particle demo's update step is close now that small structs stay in registers: on one thread it takes 1.07 times Rust's time (1.15 times with the `pow` hoisted out, down from 1.5 and 3.4), but its copy loop is still 3.4 times behind, and a whole frame with bulk drawing was only 2-7% behind before that change. Leaving out the checks the compiler proves can't fail makes the 3D voxel world with checks on 2.2 times faster (1.4 s to 0.64 s, now 1.09 times its `--release` time, was 2.4 times) and a `for i` loop over `parts[i]` 26% faster; it adds 40-70 ms to the build of a big program like that one. After the performance round (2026-11-01), fib takes 1.31 times Rust's time in `--release` (was 2.4), collatz 1.57 (was 2.3) and the heightmap 1.84 (was 2.1); `map` is level with Rust, `filter` ahead of it, the copy loop 2.1 times behind (was 3.4) and the frozen 3D voxel world 1.04 times (was 1.15); the ratios above are from before it, and the numbers are in `kestrel/bench/determinism/results.md`. All the numbers, how they were measured and how to rerun them are in [BENCHMARKS.md](BENCHMARKS.md).
