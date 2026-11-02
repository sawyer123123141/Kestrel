//! KIR statements, and the arguments `print` takes.

use crate::ir::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let(LocalIdx, Expr),
    Assign(Place, Expr),
    If { cond: Expr, then_body: Vec<Stmt>, else_body: Vec<Stmt> },
    While { cond: Expr, body: Vec<Stmt> },
    /// `for var from start to end`: `end` exclusive, step +1, both
    /// evaluated once before the first iteration.
    ForRange { var: LocalIdx, start: Expr, end: Expr, body: Vec<Stmt> },
    Break,
    Continue,
    Return(Option<Expr>),
    /// A call whose result (if any) is discarded.
    /// The call's packed line (see `pack_line`); 0 for a call the compiler made.
    Call(FuncIdx, Vec<Arg>, u32),
    /// A call through a function value whose result (if any) is discarded.
    CallIndirect { callee: Place, args: Vec<Arg>, sig: KSig },
    /// A call through an interface value whose result (if any) is discarded (see
    /// `ExprKind::CallVirtual`).
    CallVirtual { recv: Place, slot: u32, args: Vec<Arg>, sig: KSig, keeps: ContractSet, method: FuncIdx },
    Print(Vec<PrintArg>),
    /// Prints with no separators and no newline (used by generated printers).
    Write(Vec<PrintArg>),
    /// `rand_seed(n)`: makes the random numbers repeat from this integer.
    RandSeed(Expr),
    /// Appends a value to the list at a place, growing it if it's full.
    ListAdd(Place, Expr),
    /// Removes or inserts an item of the list at a place; see `ListOp` for the arguments.
    List(ListOp, Place, Vec<Expr>),
    /// Sets the length (i64) of the list at a place to a value no greater than the old length;
    /// the items past it are not dropped, whoever shortens a list has moved or dropped them. An
    /// assign helper (see `Helpers::assign_fn`) also sets it on an array, and raises it within the
    /// room `Cap` reports, then writes every item past the old length before anything reads it;
    /// `to_bytes` raises it to the room `ListOp::Reserve` made, then writes every byte.
    SetLen(Place, Expr),
    /// `CopyItems(dst, src)`: copies the items of the array or list at `src` into the buffer of the
    /// one at `dst` and sets `dst`'s length to `src`'s. The items hold no heap memory, and `dst` has
    /// room (an assign helper checks `Cap` first); the two are different places. A list keeps its
    /// capacity and its arena.
    CopyItems(Place, Place),
    /// Points a reference local at a place (a `for ... in` element, found
    /// once per iteration).
    Bind(LocalIdx, Place),
    /// Points a reference local at a global: a heap constant, built before `main`.
    BindGlobal(LocalIdx, GlobalIdx),
    /// Points a reference local at `offset` bytes into the environment whose address `env` (an
    /// `i64`) gives: a capture, in a closure's code or an environment's copy function.
    BindEnv { local: LocalIdx, env: Expr, offset: u32 },
    /// Gives the function value at a place its own copy of its environment, in the current arena,
    /// through the copy function in the environment's header (the captures are copied deep). A
    /// value with no environment (0) is left alone. `fixup` emits it for a copied function value.
    CopyFnEnv(Place),
    /// Gives the interface value at a place its own copy of its concrete value, in the current
    /// arena, through the copy function in its method table. `fixup` emits it for a copied
    /// interface value.
    CopyIface(Place),
    /// Makes an arena current: a fresh one (`arena { }`), or the one at a
    /// place (`in x { }`, creating it there if it's still null). The number is the packed line of the
    /// block (0 for an arena the compiler pushes).
    PushArena(Option<Place>, u32),
    /// Undoes the latest `PushArena`, freeing the arena for `arena { }`.
    PopArena { free: bool },
    /// Frees the arena at a place (dropping it) and nulls the place.
    FreeArena(Place),
    /// Hands the GPU object at a place to the runtime's drop hook and zeroes the place.
    DropGpu(Place),
    /// Gives the array, list, map or string at a place its own copy of its
    /// elements (a string's bytes), in the current arena (a list's capacity
    /// becomes its length).
    Dup(Place),
    /// `m[key] = value`: adds the key or replaces its value. The key, then
    /// the value, are evaluated first. The flag says the key is a string that
    /// may be a read of an existing place (see `FnLowerer::reads_place`): when
    /// it adds an entry, the entry gets its own copy of the key's bytes in the
    /// current arena. A key already there keeps its bytes, and a fresh key
    /// (a literal, an interpolation, a call's result) is never copied.
    MapSet(Place, Expr, Expr, bool),
    /// Ends a loop over the map: its lock count goes back to `saved`.
    MapUnlock(Place, Expr),
    /// `par for`: runs `func` (a `par_bodies` entry) over `0..count` on
    /// many threads. Its parameters are `args` (pointers to the variables
    /// the body uses), then the start and end of a range of indexes.
    ParFor { func: FuncIdx, args: Vec<Arg>, count: Expr },
    /// Records the line (a packed line, see `pack_line`) of the call that follows, from the
    /// program's own code to a built-in function that isn't `pure`, for the errors that function
    /// raises.
    SetLine(u32),
    /// Stops the program with a run-time error. The body of an interface method's stub, which
    /// nothing calls.
    Trap,
    /// `fail(message)`: stops the program with the string `message` and " on line L" (a packed
    /// line, see `pack_line`), through the runtime's `fail`, as every run-time error does. Line 0
    /// names no line: the copy function of an owning type's method table, which no line of the
    /// program calls directly, fails with a fixed message so. Ends its block, as `Return` does.
    Fail { message: Expr, line: u32 },
    /// `exit(code)`: flushes printed output, finishes a recording and ends the process with the
    /// integer `code`; a code outside 0 to 255 fails naming `line` (a packed line). Ends its block.
    Exit { code: Expr, line: u32 },
    /// `@assume`: the conditions (each with its id, an index into `Module::assumptions`), then
    /// `body`, the rest of the block the assumption is written in. Each condition is evaluated in
    /// order when `check` is `Check` (a false one stops the program); its body is a plain block,
    /// treated like an `If` arm by every walker.
    Assume { conds: Vec<(u32, Expr)>, check: AssumeCheck, body: Vec<Stmt> },
    /// Code a contract covers (see `ContractSite`): an annotated loop's statements (`at` 0), or a
    /// contracted callee's body the inliner expanded, with its value's temporary (`at` the call's
    /// packed line). `id` is the site's index in `Module::contracts`. A plain block, treated like
    /// an `If` arm by every walker.
    Prove { id: u32, at: u32, body: Vec<Stmt> },
}

/// Whether an assumption's conditions are evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssumeCheck {
    /// Evaluated where the assumption is written; a false one stops the program.
    Check,
    /// Not evaluated (a `--release` build that doesn't need it; `KESTREL_CHECK_PROOFS=1` still
    /// evaluates it).
    Trust,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PrintArg {
    Value(Expr),
    Str(String),
    /// An interpolated string: its parts print with nothing between them.
    /// Only in `Print` and `Write`; inside a `Format` the parts are flattened.
    Interp(Vec<PrintArg>),
}
