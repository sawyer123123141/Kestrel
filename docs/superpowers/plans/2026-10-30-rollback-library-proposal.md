# Proposal: a rollback library, and modules in folders (2026-10-30)

Roadmap step 4 (`2026-10-29-roadmap-proposal.md`). Written while the PC was busy, for the user to rule on after hot reload lands. Two parts: the library itself needs no syntax; where a program finds a library does.

## The pitch

Pong's rollback netcode (`demos/pong/peer.kes`, `net.kes`, `replay.kes`: 400 lines) is the part of a multiplayer game nobody wants to write twice: a snapshot ring, input history, a guess for the other player, rollback and re-simulation when the guess was wrong, confirmed frames, checksums every N frames, a desync report. Today it is demo code. As a library, any deterministic Kestrel game gets "cannot desync, feels instant" by implementing one interface.

## Part A: the `rollback` module (no new syntax)

**What the game writes.**

```
use { Rollback, Input, Session, new_session from rollback }

struct Game: Rollback {
    ...
    pure step(var self, mine: Input, theirs: Input) { ... }   // the whole simulation, deterministic
}

pub setup() -> State {
    State(session: new_session[Game](new_game(SEED), me: 1, delay: 2), ...)
}

pub frame(var s: State) -> bool {
    let input = Input(buttons: read_buttons(), x: 0, y: 0)
    s.session.local(input)                       // my input for this frame
    for p in s.net.receive() { s.session.remote(p) }   // the other player's, as it arrives
    s.session.advance()                          // predict, roll back, re-simulate, confirm
    s.net.send(s.session.outgoing())             // what to send: inputs and checksums
    if let why = s.session.desync() { print(why) }
    draw(s.session.game())
    window.frame()
}
```

- **`Input`** is the library's own struct, fixed so an interface can name it: `buttons: u32` (a bit per button), `x: float`, `y: float` (two axes). It is 12 bytes a frame, enough for a fighter, a platformer, a racer; a game with more packs it (text chat is not rollback input). The alternative, a game-defined input type, needs an interface method that mentions a placeholder, which interfaces cannot do today.
- **`Rollback`** is an interface with one method, `pure step(var self, mine: Input, theirs: Input)`; the library proves `@prove(deterministic, no_alloc)` through it where it can, because a call through an interface whose method is `pure` keeps the contracts (the determinism round's rule).
- **`Session[G: Rollback]`** holds the ring of `G` snapshots (storage-reusing assignment makes it allocate nothing after the first lap), input histories, the guess, the confirmed frame, the checksum ring and the report. `advance` is Pong's `peer.kes` logic, generic. Two players only this round (Pong's case; N players is a follow-up with the same shape).
- **`outgoing() -> Packet`** and **`remote(p: Packet)`** are the wire: a `Packet` is plain data (frame, the last 8 inputs, a checksum and its frame), so a game sends it with `to_bytes` over whatever it has (round 3's UDP natives, or the simulated `Net` from Pong, which moves into the module as `FakeNet` for tests and demos).
- **`desync() -> string?`** gives the report once: `desync at frame 330: my checksum 0x..., theirs 0x...`, and when both sides' snapshots are at hand (the fake net), `first_difference` of the two states.
- **Replay:** `Session.replay() -> Replay` (seed, inputs, checksums), which `to_bytes` saves; `check_replay[G](start: G, r: Replay) -> int` re-runs it.

**Where it lives: a built-in module in plain Kestrel, like `random`.** `use rollback` works in every program, in built exes and the JIT, with no path and no install. It is embedded in the compiler (`driver/std/rollback.kes`, a few files if it splits: the embedding table allows one source per module, so either one 400-line file or the table grows to several files per module, a small driver change). This is the pick because nothing else is needed to ship it, and Kestrel is "a compiled language for games". The 2026-10-12 ruling ("Kestrel stays a general language") was about the compiler, not its library: `random` is already such a module.

**Rejected:** a library in the repo used by path (needs Part B first, and a copy per project until packages exist); a function-value `step` (breaks `no_alloc` and `deterministic` through the ring); a game-defined input type (interfaces have no placeholders; a language change for one library).

**Open question (names, the user's):** `rollback`, `Session`, `Input`, `Rollback`; `local`/`remote`/`advance`/`outgoing`; `FakeNet`.

**Cost:** about Pong's 400 lines made generic, plus tests (a desync, a lost packet, a replay); Pong shrinks to its game and drawing. One round, smaller than the I/O round.

## Part B: modules in folders (syntax, the user's)

Today a module is one file next to the file that uses it. A project of any size wants folders (`net/`, `world/`, `ui/`), and a shared library wants one home.

**Pick: dotted names are folders.** `use net.packet` loads `net/packet.kes` relative to the using file; `use { Packet from net.packet }` likewise; a folder may hold a `net.kes` that `use net` loads (so a folder can start as one file and split later without changing its users). Items keep their last name (`packet.Packet` is `Packet` after `use { Packet from net.packet }`); two modules with the same last name in different folders are "`net.packet` and `ui.packet` are both `packet`; use one of them with its full name" (the full name `net.packet.Packet` works in a type position too, which closes today's gap that `geom.Point` doesn't work in a type position).

- **Why this one:** no quotes, no new keyword, reads like `draw.rect`, and it is what GDScript's `preload("res://net/packet.gd")` means without the string. Relative to the using file, so a folder moves with its users.
- **Alternatives:** `use "net/packet"` (a string; exact but ugly, and strings in `use` invite paths that escape the project); `use net/packet` (a slash in a name; parses oddly next to division); a `kestrel.toml` search path (packages' job, later); `..` in a name to go up (rejected: a library a project shares goes in a parent folder, which is what packages are for).
- **Shipped libraries** (Part A) need none of this when they are built in; a user's own library is a folder the project contains, until packages exist.

**Cost:** the loader (`driver/src/lib.rs` `load`), the link step's naming (`module.item` becomes `a.b.item`), the LSP's file lookup, error messages that name files, and the spec's "Imports and visibility". One small task; it can ride with the rollback round or alone.

## What to decide

1. Part A as a built-in module (the pick), or wait for Part B and a `lib/` folder?
2. Part B's syntax: dotted folders (the pick), or one of the alternatives?
3. Names in Part A.

Nothing is built until the user answers; hot reload (in progress) comes first either way.
