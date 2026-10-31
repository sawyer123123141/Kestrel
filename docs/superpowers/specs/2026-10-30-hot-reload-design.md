# Hot reload that keeps state (design, 2026-10-30)

Roadmap step 3 (`2026-10-29-roadmap-proposal.md`). The user delegated the pick of the next round and its decisions ("u pick"), so every decision below is the controller's under the standing delegation; the names are listed at the end for the user to change.

## Why

Editing a running game is the single most used tool in game development. Today every change to a Kestrel program means stop, recompile, restart, play back to the moment. The Godot add-on already reloads a changed program but throws its state away; the standalone `kestrel run` does neither.

## Goals

- G1. `kestrel run game.kes` on a reloadable program: save the file, see the change in the running window within a second, with the game state kept (score, world, player) when the state's shape did not change.
- G2. When the shape changed, the program restarts cleanly with one sentence saying why.
- G3. A compile error in the edit never kills the session: the old code keeps running and the error is printed; the next save tries again.
- G4. No new syntax: a reloadable program is a convention of two functions; `main` programs are untouched. Built exes run the same program without reloading.
- G5. The Pong and voxel demos are reloadable and keep their state across an edit.

## Non-goals

- Migrating state across a shape change by field name (a later round; needs a self-describing format).
- Surviving a runtime failure in the new code (`fail` still exits 101; a follow-up can catch it and roll back).
- Reloading a program under `--record` or `kestrel replay`.
- Reload in the Godot add-on (it has its own, stateless).

## Anti-goals

- A reload that copies the world (58 MB for the voxel engine) or leaks it per reload.
- A reload that rebuilds GPU meshes and textures that are still valid.
- A window that flickers, closes or reopens on reload.
- A frame hitch longer than the swap itself: compiling happens off the game thread.

## The design (decisions)

1. **The reloadable shape is two `pub` functions, no syntax.** A program with no `main` and with
   ```
   pub setup() -> State           // any struct; opens the window, loads assets, makes the state
   pub frame(var s: State) -> bool   // one frame; false ends the program
   ```
   is reloadable. `State` is any type `checksum` takes, **plus GPU objects** (a `Mesh` or `Texture` is a registry id that stays valid across a reload on the same thread); it still may not hold an `Arena`, a function value or an interface value. The checker: `frame`'s `var` parameter must be the type `setup` returns ("`frame` takes the state `setup` makes: `var s: Game`, not `var s: World`"); both must be `pub` and free functions of the entry file; a program with both `main` and `setup` is refused ("a program has `main` or `setup` and `frame`, not both"). Everything else (`check`, `build`, proofs, `--report`) sees a **synthesized `main`**: `let s = setup()` then `while frame(var s) {}`, built by the driver, so exes and tests need nothing new. A `main` program is never reloaded.

2. **One game thread for the whole session.** Today `kestrel_rt_start` spawns a fresh 256 MiB thread per call and the window, the GPU registry and the program arena are all thread-local to it. The reloading host keeps **one** such thread for the session: it runs `setup` from the first module, then `frame` in a loop, and swaps which module's `frame` it calls. The window, the registry and the arena live on. The JIT modules are never unmapped (cranelift-jit leaks them by design, so old pointers stay valid); each reload leaks one module's code (a few MB), documented.

3. **Same shape: adopt the state in place, zero copy.** The host compares the `State` type's fingerprint (`kir::lower::bytes_fingerprint`, bare names, back-references by depth) between the old and new modules. Equal fingerprints mean the same memory layout, so the new module's `frame` is called on the **same** `State` memory the old one used. Nothing is copied, nothing is dropped, no `to_bytes`. String literals in the state still point into the old module's data, which stays mapped. Lists and maps stay in the program arena. GPU ids stay registered.

4. **Changed shape: reset with a sentence.** When the fingerprints differ, the host drops the old state with the old module's drop helper (so its meshes and textures are released), calls the new module's `setup()` for a fresh state, and prints `state reset: Game changed shape; setup() ran again` on stderr. Field-by-name migration is the later round.

5. **`window.open` while the window is open is a no-op that applies the size and title.** `setup()` opens the window, and after a shape change `setup()` runs again on the same thread with the window still open; today that would fail with "the window is already open". The new rule holds for every program (a `main` program calling `open` twice gets a resize, not a failure); `window.close` is unchanged.

6. **Watching and compiling.** The game thread polls the program's files' modification times (the driver's `SourceFile::changed`) once per `frame` call, at most four times a second (stat of a handful of files). On a change it hands the paths to a **compile thread** (parse, check, prove, lower, JIT, the same steps as `run`) and keeps running frames; when the compile finishes the game thread does the swap between two frames (decision 3 or 4), resets `kestrel_rt_call_line`/`call_file`, runs the new module's init thunk (its constants), and prints `reloaded pong.kes in 0.31 s` (wall time from the save to the swap). A compile error prints the diagnostics as `run` would, then `still running the previous code; fix the error and save again`; the old module keeps running and the files' times are refreshed so the same error is not reported twice. A second save during a compile queues one more compile after it. Built-in modules never change.

7. **Flags and modes.** Reloading is on for every reloadable program under `kestrel run`; `--no-reload` turns it off (the host still drives `setup`/`frame` on one thread). `--record` implies `--no-reload` with a note (`--record runs the program without reloading`); `kestrel replay` never reloads. `KESTREL_HEADLESS=<n>` works as today (the frame count ends the program), so the tests edit a file mid-run and read stderr.

8. **The demos.** Pong: `setup` does what `main` did before the loop (the replay-check mode returns a state whose `frame` returns false at once, after printing the check); `frame` is one loop body; the end-of-run prints and the replay write move to where `frame` returns false. The voxel engine: `VoxelState { w: World, s: Stream, r: Render, p: Player, ... the scalars }`; `Render` holds `list[Mesh?]`, allowed by decision 1, so meshes survive a same-shape reload. Both keep their headless tests and frame counts.

9. **Docs.** The syntax spec gains a "Reloadable programs" section under Declarations; the README's "A window" section gets the two-function shape and `kestrel run` reload sentence; the handoff a Start here.

## Constraints

- No new syntax, keywords or Cargo dependencies. Files under about 400 lines. Messages are sentences that say what to do.
- The JIT path only; `kestrel build` exes never reload (the synthesized `main`).
- Never a visible window in tests (`KESTREL_HEADLESS`, `KESTREL_HIDDEN`).
- The swap happens between frames on the game thread; no native, draw or input call is in flight.

## Tests

- Checker: the shape rules of decision 1 (missing `pub`, wrong parameter type, `main` plus `setup`, `State` holding an `Arena` or a function value: "`setup`'s state can't hold a function value; keep functions out of the state").
- Program tests: a `setup`/`frame` program runs under `kestrel run`, `kestrel build` and `kestrel check --report` exactly like the equivalent `main` program (same output, same proofs).
- Reload tests (`cli/tests/reload.rs`, a scratch copy per test, headless with a large frame count, the test edits the file and reads stderr): a constant change shows in the output with the state kept (a counter keeps counting); a shape change prints `state reset` and the counter restarts; a compile error prints the diagnostics and `still running`, the counter keeps counting, and a fix reloads; two saves in a row give two reloads; a `main` program is never reloaded; `--no-reload` and `--record` never reload; a reload keeps a string field made from a literal readable; a reload keeps a GPU object (a texture's size printed before and after, hidden window if headless cannot hold one; else the state test uses a `Mesh?`-holding struct headless).
- `window.open` twice: a program test printing `window.size()` after a second `open` with another size.
- Demos: Pong's and the voxel engine's headless runs unchanged in output and frame count; one reload test each (a constant edit mid-run keeps the score / the player position).

## Measurements (when the PC is free)

Reload latency for Pong and the voxel engine (save to `reloaded` line): median of five; the per-frame cost of the file poll; memory per reload (`KESTREL_ARENA_REPORT` and process RSS after ten reloads).

## Names (the user's call; the picks are built)

| Pick | Alternatives |
|---|---|
| `setup()` and `frame(var s)` | `init`/`update`, `start`/`tick`, `load`/`step` |
| `--no-reload` | `--once`, `--static` |
| `reloaded pong.kes in 0.31 s`, `state reset: Game changed shape; setup() ran again`, `still running the previous code; fix the error and save again` | wording |

## Left to the builder

How the host calls `setup` and `frame` (the export thunks `k_export_<name>` with `ExportInfo`, or `fn_ptr` with the Kestrel ABI; the thunk is expected); how the compile thread hands the module back (a channel); the poll interval's exact value; whether the drop helper for `State` is reached through an export or a generated `k_reload_drop` symbol; the test harness's file-edit synchronization (a marker line the program prints each frame is expected).
