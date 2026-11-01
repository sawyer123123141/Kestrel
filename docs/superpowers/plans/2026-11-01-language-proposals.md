# Language proposals (2026-11-01): five rulings Kestrel needs beyond demos

Roadmap step 7 and open question 4 (`2026-10-29-roadmap-proposal.md`), follow-ups 58, 82, 83 and 96 (`2026-10-09-core-2a-followups.md`), and section 4 of `2026-11-01-frontier-roadmap-proposal.md` (one-line picks) all wait on the same five rulings. This document gives each one in full: the problem in the repo's own code, what other languages do, two or three candidates in Kestrel syntax, the controller's pick and its cost. Nothing is built. Syntax and names are the user's; each section ends with one line where "yes" takes the pick.

## 1. Errors as values

**The problem.** A failure today is `none`, and the reason lives somewhere else or nowhere. `file.read` gives `list[u8]?` and leaves the reason in `file.error()`, one string per program that the next file call replaces. `from_bytes` gives `none` for any of ten reasons and keeps none of them. So the voxel engine's `load_world` (`kestrel/voxel/save.kes`) returns a `bool`, and a second function guesses the reason afterwards:

```
pub load_world(var w: World, var p: Player) -> bool {
    let bytes = file.read(SAVE_FILE) else { return false }
    let save = from_bytes[Save](bytes) else { return false }
    ...
}
pub load_failure() -> string {          // must be called right after, before file.error() changes
    let why = file.error()
    if not file.exists(SAVE_FILE) { return "no {SAVE_FILE} to load" }
    if why != "" { return "could not load: {why}" }
    "could not load: {SAVE_FILE} is damaged or was saved by another version of the engine; ..."
}
```

Pong's `check_file` (`kestrel/demos/pong/pong.kes`) writes the same two `let ... else` blocks with a `print` in each. A program can write its own `enum Result[T, E]` (`tests/programs/generic_result.kes`), but there is no short way to pass one up.

**Elsewhere.** Rust: `Result<T, E>` is an ordinary enum, and `?` returns the `Err` from the function (converting it with `From`); `?` also works on `Option` in a function that returns one; `panic!` is for bugs. Zig: an error union `E!T`, where errors are named values of an error set with no payload, and `try x` means `x catch |err| return err`. Swift: a function is marked `throws`, each call that can throw is marked `try`, `do { } catch { }` handles it and `try?` turns it into an optional; Swift 6 added typed `throws(E)`. Go: a second result of type `error` and `if err != nil { return err }`, with no short form. Odin: `or_return` on a call whose last result is an error.

**Candidates.**

A. A built-in `Result[T, E]` and a prefix `try`. `Result` is the enum `generic_result.kes` writes (`Ok { value: T }`, `Err { error: E }`), in scope everywhere. A plain `T` goes where a `Result[T, E]` is expected as `Ok`, as a `T` goes where a `T?` is (refused when `T` and `E` are one type: write `Result.Ok(value: x)`). `try r` gives the `Ok` value or returns the `Err` from the function, whose result must be a `Result` with the same `E`; on a `T?` in a function that returns an optional it returns `none`. `let ... else` may name the error.

```
load(path: string) -> Result[Save, string] {
    let bytes = try file.read(path)
    let save = try from_bytes[Save](bytes)
    save                                              // a Save goes in as Ok
}

let save = load(SAVE_FILE) else why { print(why); return }   // why is the string
match load(SAVE_FILE) {
    Ok s  { apply(s.value) }
    Err e { print("could not load: {e.error}") }
}
```

B. One built-in error type: `load(path: string) -> Save!`, where `T!` is "a `T` or an `Error`", a built-in struct with `message: string` (Go's single `error`, Swift's untyped `throws`). `try` as in A. There is no `E` to choose and nothing to convert, but a caller tells kinds of failure apart only by their text.

C. No new type: keep `T?` and add only a short form, `let bytes = file.read(path) or return none`. The cheapest; the reason is still lost.

**Pick: A.** It is the enum programs already write by hand, so `match`, generics, `checksum`, `to_bytes` and `first_difference` take it with no new rule, and `E` can be an enum when a caller must tell "no save" from "damaged save" (exactly what `load_failure` guesses at). `try` is one word that Zig and Swift use for the same job, and it reads before the call as GDScript's `await` does. `else why` extends today's unwrapping form instead of adding a second one.

**How it meets the rest.**
- `T?` stays for "may not be there" (a map lookup, `pop`, `find`, `env.get`, `to_int`); `Result` is for "tried and failed, and here is why". `try` on an optional in a function that returns a `Result` is refused with help: `let b = opt else { return Result.Err(error: "...") }`.
- `try` is a `return`: it leaves `arena { }` blocks and drops values as `return` does, and the escape check applies, so an error string made inside an `arena { }` can't be returned out of it.
- `pure` and `@prove`: nothing new. A `pure` function may return a `Result`; `try` is a tag test and a return. `no_alloc` names an error message built with `{}`, as it names any string; `no_panic` is untouched, since an `Err` is not a failure of the program; `deterministic` holds as for any enum.
- `Result` is lowered as every generic enum is (a `u8` tag and a union, `2026-10-09-kestrel-core-rebuild-design.md`, "Layout"). Generics refuse an optional or an owning type as `T` today, so `Result[int?, string]` and `Result[World, string]` (a `World` holds an `Arena`) are refused until those rules lift.
- `file.read`, `read_text` and `list_dir` give `Result[..., string]` whose error is the sentence `file.error()` gives today, and `file.error()` goes. `write`, `write_text`, `make_dir` and `remove` give `Result[bool, string]` (`true` on success) until Kestrel has a type for "nothing" (Rust's `()`). `std/file.kes` keeps its natives private and wraps them in Kestrel, so the native ABI doesn't change.
- `from_bytes[T](b)` gives `Result[T, string]` naming the reason: "the bytes end early (57 of 57)", "these bytes are another shape of `Save` (a field changed)", "byte 40 should be 0 or 1". `load_failure` and the `file.error()` reads in Pong go.
- A panic stays for the program's own mistakes: an index out of range, an overflow, a division by zero, a false `@assume`, and `fail` (section 3). Those end the program with exit code 101 and can't be caught. An `Err` is for the world being wrong: a missing file, a damaged save, a bad packet.

**Cost.** `syntax` (`try`, `else name`), `check` (the built-in `Result`, the `Ok` conversion, typing `try`, the refusals), `kir` (`try` as a tag test and a return; a reason per `from_bytes` failure in `lower/bytes_read.rs`), `driver/std/file.kes`, `lsp` (the keyword), tests, Pong and the voxel engine's `save.kes`. About 5-7 days.

**Decide:** a built-in `Result[T, E]` with `Ok`/`Err`, prefix `try`, `let x = r else why { }`, and `file`/`from_bytes` returning `Result[..., string]`? (yes = the pick)

## 2. Packages

**The problem.** A module is found next to the file that uses it or in a folder below it, and names never go up. Code shared by two programs must be copied. The rollback library became a compiler built-in (`kestrel/driver/std/rollback.kes` and `rollback/*.kes`) "because nothing else is needed to ship it" (`2026-10-30-rollback-library-proposal.md`), although the roadmap's rejected alternatives had ruled a game-only built-in against the general-language ruling. So a fix to it ships only with a new compiler, and Pong can't use the voxel engine's `noise.kes` without a copy.

**Elsewhere.** Cargo: `Cargo.toml` lists dependencies by version from the crates.io registry, by `git` URL with a `tag` or `rev`, or by `path`; `Cargo.lock` pins the exact versions and commits; downloads live in a cache under the user's home. Go: `go.mod` names the module by its import path (usually a repository URL) and its required versions, `go.sum` holds checksums, minimal version selection picks the highest version any module requires, and a major version 2 or later is part of the import path (`/v2`). Zig: `build.zig.zon` lists dependencies by `url` and a content `hash`, `zig fetch --save` adds one, and there is no central registry.

**Candidates.**

A. A manifest, a lock file and a cache. `kestrel.toml` beside the program's entry file; `kestrel.lock` written by `kestrel`, committed with the program; packages fetched into `%LOCALAPPDATA%\kestrel\packages\<name>-<commit>\`. Then `use netcode` loads the package's own file (`netcode/netcode.kes`, as with folders) and `use { Session from netcode.session }` a file inside it: full names start with the package's name.

```
# kestrel.toml
[package]
name = "pong"
[dependencies]
netcode = { git = "https://github.com/someone/netcode", tag = "v0.3.0" }
shared  = { path = "../shared" }

# kestrel.lock (written by kestrel add / kestrel update; never by run or build)
[[package]]
name = "netcode"
git = "https://github.com/someone/netcode"
tag = "v0.3.0"
commit = "<40 hex digits>"
hash = "<sha-256 of the package's files>"
```

- B. Go's way: no manifest; the `use` names the repository (`use "github.com/someone/netcode" v0.3.0`) and a lock pins it. Every file that uses it repeats the URL and version.
- C. Vendored: `kestrel add <url> --tag v0.3.0` copies the package into `packages/netcode/` beside the program and records the commit in `kestrel.lock`; `use netcode` looks in `packages/` after the program's folder. No cache and no network at build time, and edits to a package hot reload; the program's repository carries every package's files.

**Pick: A.** It is the shape Cargo, Go and Zig share, it keeps package files out of the program's repository, and `path` covers local editing (and hot reload of a package under development). How it works:
- **Finding a module.** `use x` looks at the built-in modules, the program's folder (as today) and the packages in `kestrel.toml`. A first name found in two places is an error ("`netcode` is both the folder netcode/ and a package in kestrel.toml; rename one"), since Kestrel never lets one name hide another; a package named like a built-in module is refused. Inside a package, `use` finds that package's own files and its own dependencies, never the program's files. Runtime errors and `--report` name a package file by its full path (`netcode/session.kes:88`).
- **Versions.** A dependency names a `tag` or a `rev`. A program has one copy of each package: when two packages ask for different tags of a third, the highest `vX.Y.Z` with the same `X` is taken (Go's rule); different `X` is an error naming both requirers. No registry and no version ranges until outside users exist.
- **Fetching.** `kestrel add <url> --tag <t>` and `kestrel update` change the toml and the lock; `kestrel run` and `build` fetch what the lock names when it is missing from the cache (with the `git` command), check its hash, and never change the lock. A package is a folder with its own `kestrel.toml`.
- **What `kestrel build` embeds.** The exe holds compiled code only, as now, plus the compiler version and each package's name and commit. `--record` writes the same list into the replay header (a version bump), and `kestrel replay` reports a different list on stderr and still runs, as it does today for a different compiler version.

**Cost.** A new crate `kestrel-pkg` (reading and writing the toml and the lock, the version rule, running `git`, hashing), `driver/src/load.rs` (search roots per package, full names with the package prefix), `cli` (`add`, `update`, fetching in `run` and `build`, the replay header), tests with a local bare git repository as the remote. About 6-8 days. Whether `rollback` then leaves the compiler for a package is a later ruling.

**Decide:** `kestrel.toml` + `kestrel.lock`, git URL with a tag or a path, a per-user cache, `use pkg.module`, one copy per package? (yes = the pick)

## 3. `exit(code)` and `fail("sentence")`

**The problem.** A library can't refuse a bad argument. `new_session` (`kestrel/driver/std/rollback.kes`) coerces instead, `me: if me == 2 { 2 } else { 1 }, delay: clamp(delay, 0, MAX_DELAY)`, and its comment says why: "A library can't yet stop a program with a message, so values out of range are clamped rather than refused" (follow-up 83). And a program can't set its exit code: `main` returns nothing and only a runtime failure exits (101), so Pong's replay check prints `replay differs at frame 360`, exits 0, and its tests read the line (follow-up 58).

**Elsewhere.** Rust: `std::process::exit(code)` ends the process at once without running destructors, and `panic!("...")` on the main thread ends it with code 101 (Kestrel's runtime errors already use 101). Go: `os.Exit(code)` skips deferred calls, and `panic` ends the program with code 2. Zig: `std.process.exit(code)` and `@panic("...")`. Swift: `fatalError("...")`.

**Candidates.**

A. Two built-in functions, `exit(code: int)` and `fail(message: string)`, that never return.
```
pub pure new_session[G: Rollback](start: G, me: int, delay: int) -> Session[G] {
    if me != 1 and me != 2 { fail("`me` is 1 or 2, found {me}") }
    if delay < 0 or delay > MAX_DELAY { fail("`delay` is 0 to {MAX_DELAY}, found {delay}") }
    ...
}
let hp = scores[name] else { fail("no score for {name}") }   // an else block may end with fail
```

- B. `main() -> int` for the exit code, and `fail`. A reloadable program has `setup` and `frame`, not `main`, and a function deep in the calls can't set the code, so B covers fewer cases.
- C. A plus an assertion, `require(cond, message)` (Kotlin's `require` is the same idea). `if cond { fail(...) }` is already one line; it can come later.

**Pick: A.** It closes both follow-ups with two names, and "never returns" already exists in the checker (`while true` with no `break` can end a value-returning function). The rules:
- `fail` prints the sentence and where it was called, as runtime errors do ("`me` is 1 or 2, found 3 on line 91 of rollback.kes"), flushes printed output and exits 101. It is allowed in `pure` functions and `par for` bodies, which can already stop on an index out of range. `exit` flushes output and ends with the code, 0 to 255 (another code is a `fail` that names it); it is an effect, so `pure` functions and `par for` bodies can't call it.
- `@prove`: `fail` breaks `no_panic` ("it calls `fail` (line 91)") and `--report` lists it as a way to fail. `deterministic` holds through it. Its message is built only on the path that ends the program, so `no_alloc` doesn't count it.
- Replays: `--record` writes the file on `exit` as on a normal end, with the code, and on `fail` as on a runtime failure today; `kestrel replay` checks the code too ("the recording exited with 0, this run with 1").
- Hot reload: `exit` ends the session (a `frame` that gives `false` is still the normal end); `fail` ends it as a runtime failure does today, per the hot reload spec's non-goal. Keeping the window open after a `fail` and waiting for a fixed save is a later follow-up.
- `exit` and `fail` become reserved built-in names; no `.kes` file in the repo uses either. The line named is the `fail`'s own (a library's line, not its caller's); naming the caller needs a call chain at run time, which Kestrel doesn't keep.

**Cost.** `check` (two built-ins, never returning, the `pure` rule), `kir` (a runtime call and an unreachable end), `runtime` (exit with flush; `fail` through the runtime error path with the program's text), the contracts and `--report`, the replay format (`cli`), `rollback.kes` refusing instead of clamping, Pong exiting 1 on a differing replay, tests. About 2 days.

**Decide:** `exit(code)` (not in `pure`) and `fail("sentence")` (allowed in `pure`, breaks `no_panic`, exit 101)? (yes = the pick)

## 4. Interface placeholders

**The problem.** The rollback library's `Input` is fixed: `buttons: int, x: float, y: float`, 12 bytes. A fighting game's stick and six buttons, or an RTS's command, can't be a game's input. Pong's own inputs were 4 bytes, and the wider `Input` is one of three measured causes of Pong's network frame being 6-9% slower on the library (follow-up 96; the replay grew from 4 to 24 bytes a frame); N players (follow-up 82) needs the step's signature to change too. The rollback spec lists "a game-defined input type (interfaces have no placeholders)" as a non-goal. The parser refuses `interface Rollback[I]` with "interfaces don't take placeholders yet; write the interface's methods with concrete types" (`kestrel/syntax/src/parser/iface.rs`), and a method's own placeholders with "methods can't have placeholders of their own yet" (`parser/generics.rs`). What generics have today: placeholders on structs, enums and functions; rules that are interfaces or `equatable`, `ordered`, `key`; one compiled copy per set of types, where a call through a rule is direct. An interface is both a rule and a value type (data and a method table, two words).

**Elsewhere.** Rust: traits take type parameters (`From<T>`, which a type may implement for several `T`) and have associated types (`Iterator`'s `type Item`, one per type). Swift: protocols have associated types (`associatedtype`), and since Swift 5.7 a protocol can name primary ones (`Collection<Int>`). Go: interfaces take type parameters since Go 1.18. Zig has no interfaces.

**Candidates.**

A. Placeholders on interfaces, in the generics' brackets:
```
pub interface Rollback[I] {
    pure step(var self, one: I, two: I)
}
struct Game: Rollback[Pad] {
    ...
    pure step(var self, one: Pad, two: Pad) { ... }
}
pub struct Session[G: Rollback[I], I: equatable] { game: G, local_in: [I], ... }
var s = new_session(Game(), me: 1, delay: 2)   // G is Game; I is Pad, from Game's list
p1: Session[Game, Pad]                          // a field writes both
```

B. Associated types:
```
pub interface Rollback {
    type Input
    pure step(var self, one: Input, two: Input)
}
struct Game: Rollback {
    type Input = Pad
    pure step(var self, one: Pad, two: Pad) { ... }
}
pub struct Session[G: Rollback] { game: G, local_in: [G.Input], ... }
p1: Session[Game]
```

C. No language change: widen the fixed `Input` (an `i64` of buttons, or a `[u8]` payload). The cost of follow-up 96 stays, and a payload needs decoding in every `step`.

**Pick: A.** The same brackets as every other generic; an interface with placeholders still works as a value type (`list[Handler[Click]]`); and a type may implement one interface for several types (`Handler[Click]` and `Handler[Key]`). B can't be a value type without naming its type (Rust's `dyn Iterator<Item = u32>`), and adds `type` lines and `G.Input` paths. A's cost is writing `Session[Game, Pad]` in a field where B writes `Session[Game]`; calls work it out.

What the checker needs:
- An interface instance per set of types (`Rollback[Pad]`), made like `Stack[int]` with its method signatures filled in; conformance checked per listed instance (`check_conformance`, `check/src/iface.rs`); `implements` comparing instances.
- Rules with types (`[G: Rollback[I]]`, `generic/rules.rs`), and one inference step: a placeholder used only in another's rule is worked out from the type's interface list ("`Game` lists `Rollback[Pad]`, so `I` is `Pad`"), with an error when the type lists two instances ("write the types: `new_session[Game, Pad](...)`"). The endless-copies check follows interface instances.
- In `kir`, a method table per type and interface instance.
- Interface methods get no placeholders of their own: a method table holds one function per method, and such a method would need a copy per type it is called with, unknown when the table is built (Rust refuses generic methods on `dyn` traits for this reason). Struct methods' own placeholders (`xs.map_to[U]`) stay a separate item.
- The library compares a guess with the real input, field by field today because Kestrel has no `==` on structs. A game's `I` needs a rule for it: `equatable` widens to structs and enums whose fields are all equatable (a NaN unequal, as for floats).

**Cost.** `syntax` (brackets on `interface`, in interface lists and rules), `check` (generic tables, `iface.rs`, `generic/rules.rs`, inference), `kir` (method tables per instance), `lsp` (hover), the rollback library (`Session[G, I]`, `Packet[I]`, `Replay[G, I]`) and Pong back on its own small input. About 5-7 days, plus 1 for structs as `equatable`.

**Decide:** placeholders on interfaces, `interface Rollback[I]` and `struct Game: Rollback[Pad]`, with `equatable` widened to plain structs? (yes = the pick)

## 5. `pure` inferred or written

**Today's rule.** `pure` marks a function the compiler verifies has no effects: no I/O, no writes but to its locals and `var` parameters, and calls only to `pure` functions. A `par for` body may call only `pure` functions; every `pure` function holds `@prove(deterministic)`; interface methods and function types say `pure` (`pure (A) -> R`); a closure is already inferred ("a closure is `pure` when its body is"). The language pass 1 design ruled that `pure` "is a checked declaration by design, so the checker won't infer it". The user's friends asked on 2026-10-28 whether it should be inferred. The voxel engine and Pong write `pure` 138 times, about three of every four functions, and a helper without it stops a `par for` far away with "pure functions can only call pure functions; `x` isn't pure", so `pure` gets added up the whole chain.

**Elsewhere.** D infers `pure`, `nothrow`, `@nogc` and `@safe` for templates, function literals and `auto` functions, and checks them where they are written. Rust has no `pure`; its parallel loops are made safe by `Send`, `Sync` and the borrow checker instead. Go, Zig and Swift have no purity marker.

**Candidates.**

- A. Written, as today.
- B. Inferred for every function with a body; a written `pure` is a checked promise, reported at the declaration as today. Interface methods and function types keep `pure` written, since they have no body to infer from.
- C. Inferred inside a module; written on `pub` items, the module's promise to its users.

**What inference changes.**
- `par for`: the error moves from the helper to the loop and names the chain, as contracts do: "this `par for` (line 155) calls `mesh_chunk`, which calls `log_face` (mesher.kes:40), which prints; a `par for` body can only call functions without effects". A `print` added deep in a helper breaks a distant loop; a written `pure` on the helper moves the error back to it.
- `@prove(deterministic)` follows the call graph already; an inferred pure function holds it as a written one does.
- Interfaces: a call through an interface is pure only when the interface method says `pure`; an implementation may be inferred pure.
- Function values: `let f = height` gets `pure (int, int, int) -> int` when `height` is inferred pure.
- "The result of pure function `x` is unused" fires for any effect-free function whose result is dropped, which the no-effect-statement rule already means.
- Packages (section 2): a package's `pub` function counts as pure to its users only when written, so an update can't silently break a user's `par for`.

**Pick: B, with the package rule.** Three of four functions are written `pure` already; closures and `deterministic` infer through calls already; the chain message is the form contracts use; and the package rule keeps the promises a library makes explicit, which is what C wanted, without making a single program write them.

**Cost.** `check`: after every body is checked, one pass over the call graph sets each function's purity (recursion: assume pure, drop on an effect, repeat until nothing changes); the `pure` and `par for` call checks in `check/src/body/call.rs`, `closure.rs` and `fnvalue.rs` move to that pass and gain the chain; error tests rewritten; `lsp` hover says "pure (inferred)". About 3-4 days. Written `pure` in the demos stays valid; removing it is optional.

**Decide:** infer `pure`, keep a written `pure` as a checked promise, required on interface methods, function types and a package's `pub` functions? (yes = the pick)

## What to decide

1. Errors: built-in `Result[T, E]`, prefix `try`, `let x = r else why { }`, `file`/`from_bytes` returning `Result[..., string]` (yes = the pick).
2. Packages: `kestrel.toml` + `kestrel.lock`, git URL with a tag or a path, a per-user cache, `use pkg.module` (yes = the pick).
3. `exit(code)` and `fail("sentence")`: `fail` allowed in `pure` and breaks `no_panic`, `exit` not in `pure` (yes = the pick).
4. Interfaces: placeholders (`interface Rollback[I]`, `struct Game: Rollback[Pad]`), and `equatable` for plain structs (yes = the pick; the alternative is associated types).
5. `pure`: inferred, with a written `pure` as a checked promise required on interface methods, function types and package `pub` functions (yes = the pick).
