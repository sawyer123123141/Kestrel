//! Kestrel's typed, structured middle layer. Loops stay loops; no
//! Cranelift (or any backend) types appear here.

use std::collections::HashMap;

pub use crate::contract::*;
use crate::layout::{EnumLayout, StructLayout};
pub use crate::ir_expr::*;
pub use crate::ir_stmt::*;
pub use crate::ops::{HashOp, ListOp, MapOp, MathOp, SortKey, StrOp};

/// Bits of a KIR line (`Expr::line`, `Stmt::SetLine`) that hold the line within its file; the
/// bits above hold the file's index in `Module::files`. Bit 31 stays clear.
pub const LINE_BITS: u32 = 21;
/// The file index that means "no name" (a file past the 1,023rd).
pub const NO_FILE: u32 = 1023;

/// A source position as KIR keeps it: the line within its file, and the file's index. A line
/// past 2,097,151 is clamped; a file index past 1,022 becomes `NO_FILE`.
pub fn pack_line(file: usize, line: u32) -> u32 {
    let file = u32::try_from(file).unwrap_or(NO_FILE).min(NO_FILE);
    (file << LINE_BITS) | line.min((1 << LINE_BITS) - 1)
}

/// The line and the file index of a packed KIR line (see `pack_line`).
pub fn unpack_line(loc: u32) -> (u32, u32) {
    (loc & ((1 << LINE_BITS) - 1), loc >> LINE_BITS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KTy {
    Bool,
    U8,
    I32,
    I64,
    F32,
    F64,
    /// `vec4`: four `F32` lanes as one SIMD value (lane `i` at byte `4 * i`), kept in a register
    /// like a number and never split into lanes. Size and alignment 16.
    F32X4,
    /// Four `I32` lanes as one SIMD value, like `F32X4` (`ivec4`).
    I32X4,
    /// A struct, laid out by `Module::structs`. Lives in memory, never in a register.
    Struct(StructIdx),
    /// An enum, laid out by `Module::enums`: a u8 tag, then the variant's fields.
    Enum(EnumIdx),
    /// A string: a pointer to UTF-8 bytes at offset 0, its length (i64) at 8.
    Str,
    /// `[T]`: a pointer to the elements at 0, the length (i64) at 8.
    Array(ElemIdx),
    /// `list[T]`: pointer, length, capacity (i64s), then the arena it grows in.
    List(ElemIdx),
    /// A pointer to a runtime arena (null until created).
    Arena,
    /// A GPU object's id (an `i64`; 0 is none), owned like an arena.
    Gpu,
    /// `T?`: a u8 tag (0 = `none`) at offset 0, then the value.
    Optional(ElemIdx),
    /// `map[K, V]`: a 64-byte header (see `MapLayout`) over entries and an
    /// index table in the map's arena.
    Map(ElemIdx, ElemIdx),
    /// A function value, whatever its signature: the code's address at offset 0 and the
    /// environment (an `i64`; 0 when there is none) at 8. The code takes the environment first,
    /// then the parameters (see `KSig`).
    Fn,
    /// An interface value, whatever its interface: a pointer to the concrete value at offset 0
    /// and a pointer to its method table (see `MethodTable`) at 8.
    Iface,
}

/// What an indirect call needs to know about the function it calls: each parameter's type and
/// whether it is passed by pointer (`var`, or an aggregate: the same rule as `LocalKind::RefParam`
/// for a parameter), and the result. `pure` is the source signature's: a `pure` function type, or
/// a `pure` interface method (what `@prove(deterministic)` reads; codegen ignores it).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KSig {
    pub params: Vec<(KTy, bool)>,
    pub ret: Option<KTy>,
    pub pure: bool,
}

impl KTy {
    pub fn is_aggregate(self) -> bool {
        matches!(
            self,
            KTy::Struct(_)
                | KTy::Enum(_)
                | KTy::Str
                | KTy::Array(_)
                | KTy::List(_)
                | KTy::Map(..)
                | KTy::Arena
                | KTy::Gpu
                | KTy::Optional(_)
                | KTy::Fn
                | KTy::Iface
        )
    }

    /// A SIMD vector (`F32X4`, `I32X4`). It answers `false` to `is_int`, `is_float`, `is_signed`
    /// and `is_aggregate`: every question about its lanes goes through `lane()`.
    pub fn is_vector(self) -> bool {
        matches!(self, KTy::F32X4 | KTy::I32X4)
    }

    /// The type of one lane of a vector (`F32` or `I32`); a scalar's lane is itself.
    pub fn lane(self) -> KTy {
        match self {
            KTy::F32X4 => KTy::F32,
            KTy::I32X4 => KTy::I32,
            other => other,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, KTy::F32 | KTy::F64)
    }

    pub fn is_int(self) -> bool {
        matches!(self, KTy::U8 | KTy::I32 | KTy::I64)
    }

    pub fn is_signed(self) -> bool {
        matches!(self, KTy::I32 | KTy::I64)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FuncIdx(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalIdx(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StructIdx(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EnumIdx(pub u32);

/// An element type of arrays and lists, in `Module::elems`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElemIdx(pub u32);

/// A method table in `Module::tables`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableIdx(pub u32);

/// The method table of one concrete type as one interface: static data of 8-byte words,
/// `[type_id, copy, print, method_0, method_1, ...]` (see `TABLE_METHODS`). Every entry after the
/// type id is a function's address. `copy` gives an interface value its own copy of the concrete
/// value: `(data: i64) -> i64`, the copy's address. `print` writes the concrete value, taken by
/// pointer. The methods are the type's own methods, in the interface's declaration order, so a
/// call passes the result pointer (if any), the data pointer as `self`, then the arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct MethodTable {
    /// The concrete type's id: the same for every table of one type.
    pub type_id: u32,
    pub copy: FuncIdx,
    pub print: FuncIdx,
    pub methods: Vec<FuncIdx>,
}

/// The word of a method table that holds its copy function's address.
pub const TABLE_COPY: u32 = 1;
/// The word of a method table that holds its print function's address.
pub const TABLE_PRINT: u32 = 2;
/// The word of a method table that holds method 0's address; method `n` is at `TABLE_METHODS + n`.
pub const TABLE_METHODS: u32 = 3;

/// A zero-filled global in `Module::globals`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalIdx(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub funcs: Vec<Func>,
    pub structs: Vec<StructLayout>,
    pub enums: Vec<EnumLayout>,
    pub elems: Vec<KTy>,
    /// The generated function that prints each array or list type printed.
    pub print_fns: HashMap<KTy, FuncIdx>,
    /// `par for` bodies, each called through `Stmt::ParFor`.
    pub par_bodies: Vec<FuncIdx>,
    /// Zero-filled global memory, by `GlobalIdx`. Empty without heap constants; otherwise
    /// global 0 is the "built" flag (`Bool`), global 1 the constants' arena (`Arena`), and
    /// one more follows for each heap constant that is used.
    pub globals: Vec<KTy>,
    /// `__kestrel_constants`, which builds the heap constants once (it can run again and does
    /// nothing). `main` calls it first; a host that calls other functions runs it itself.
    pub init: Option<FuncIdx>,
    /// `None` for a library (see `kestrel_check::check_with`).
    pub main: Option<FuncIdx>,
    /// The source files' names (base names, like `geom.kes`), by the file index in each line
    /// (see `pack_line`). Empty when the compiler wasn't given names: runtime errors then give
    /// only the line.
    pub files: Vec<String>,
    /// The method table of each (concrete type, interface) pair an interface value is made from.
    pub tables: Vec<MethodTable>,
    /// Each call `inline_small` left in place and why: function by function, each in the order
    /// decided. A call left in a function that is inlined is listed again under each caller of the
    /// copy, with the same reason.
    pub not_inlined: Vec<NotInlined>,
    /// Each `@assume` condition, by the id its `Stmt::Assume` gives it.
    pub assumptions: Vec<AssumeInfo>,
    /// Each function, method or loop with a `@prove`, by the id its `Stmt::Prove` blocks give it.
    pub contracts: Vec<ContractSite>,
    /// What each copy of a generic function or method was made from, by the copy's function.
    pub copies: HashMap<FuncIdx, CopyNote>,
    /// The generic functions and methods that have no copy, by name and packed lines (see
    /// `Func::lines`): nothing was checked in them.
    pub uncopied: Vec<(String, (u32, u32))>,
    /// What each local `hoist::hoist_math` made held where it was written in the loop, by function
    /// and local: the report and contracts write a load of the local as that expression, so their
    /// text is what it was before the pass.
    pub hoisted: HashMap<(FuncIdx, LocalIdx), Expr>,
    /// The build range analysis marked the module for, in place (`range::remove_checks` or
    /// `mark_proven`); `None` until then. Codegen compiles an analysed module as it is.
    pub analysed: Option<crate::range::Arith>,
    /// What the reloading host needs of a reloadable program (`setup` and `frame`, no `main`);
    /// `None` for any other.
    pub reload: Option<ReloadInfo>,
}

/// A reloadable program's state and `setup`, as the reloading host compares them between the old
/// code and the new (hot reload design, decisions 3 and 4). Equal descriptions mean the same
/// memory layout, so the new `frame` runs on the old state in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadInfo {
    /// The state type's description: the text `to_bytes`'s fingerprint hashes, with `gpu` for a
    /// GPU object.
    pub state_description: String,
    /// A hash of `setup`'s lowered body, without its lines and with every index into the module
    /// replaced by what it names: it changes when `setup` does, not when other code moves.
    pub setup_hash: u64,
    pub state_size: u32,
    pub state_align: u32,
}

/// What a copy of a generic was made for (see `Module::copies`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyNote {
    /// The generic's plain name: `best` or `Stack.push`.
    pub generic: String,
    /// What made it: `Stack[string]` for a copy of a method, the copy's own name (`best[Enemy]`)
    /// for a copy of a function.
    pub made_by: String,
    /// The packed line (see `pack_line`) of the first place in the program's own code that led to
    /// the copy.
    pub made_at: u32,
}

/// An `@assume` condition as written: for its run-time error and the report.
#[derive(Clone, Debug, PartialEq)]
pub struct AssumeInfo {
    /// The condition's text ("n <= xs.len").
    pub text: String,
    /// The `@assume`'s span in the program's source.
    pub span: kestrel_syntax::Span,
    /// The packed line of the `@assume` (see `pack_line`).
    pub line: u32,
}

/// A memory location: a local, or a path of fields and elements inside one.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub root: LocalIdx,
    /// Steps from the root inward.
    pub path: Vec<Proj>,
}

/// Whether an element access is bounds-checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexCheck {
    /// Checked at run time.
    Checked,
    /// Known in bounds by lowering itself (a `for ... in` loop's own index,
    /// when the body can't move the elements; the built-in helpers).
    Known,
    /// Proven in bounds by range analysis. Codegen leaves the check out (or,
    /// when asked to verify proofs, emits it and stops with a compiler-bug
    /// message if it fails).
    Proven,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Proj {
    /// A struct field.
    Field(u32),
    /// Field `.1` of variant `.0` of an enum (valid while that variant is stored).
    Variant(u32, u32),
    /// The value inside an optional (valid while it isn't `none`).
    Payload,
    /// An element of an array or list. The index is any integer type. It is
    /// bounds-checked unless `check` says otherwise.
    Index { at: Expr, check: IndexCheck },
    /// The key of entry `at` of a map (its index in the entries, not a key).
    MapKey { at: Expr },
    /// The value of entry `at` of a map. Checked, it panics unless the
    /// entry is still there (the loop body may have replaced the map).
    MapValue { at: Expr, checked: bool },
    /// Lane `.0` of a vector (`v.x` is lane 0). Only ever a place's last step.
    Lane(u8),
}

impl Proj {
    /// The index expression inside an element or entry projection.
    pub fn at(&self) -> Option<&Expr> {
        match self {
            Proj::Index { at, .. } | Proj::MapKey { at } | Proj::MapValue { at, .. } => Some(at),
            Proj::Field(_) | Proj::Variant(..) | Proj::Payload | Proj::Lane(_) => None,
        }
    }

    pub fn at_mut(&mut self) -> Option<&mut Expr> {
        match self {
            Proj::Index { at, .. } | Proj::MapKey { at } | Proj::MapValue { at, .. } => Some(at),
            Proj::Field(_) | Proj::Variant(..) | Proj::Payload | Proj::Lane(_) => None,
        }
    }
}

impl Place {
    pub fn local(root: LocalIdx) -> Self {
        Place { root, path: Vec::new() }
    }
}

impl Expr {
    /// Whether this is a read of a whole local (`Load` or `Move` of it, no projection): nothing to
    /// check and nothing that runs, so it can stand anywhere the local still holds the same value.
    pub fn is_local_read(&self) -> bool {
        matches!(&self.kind, ExprKind::Load(p) | ExprKind::Move(p) if p.path.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Func {
    pub name: String,
    pub params: Vec<LocalIdx>,
    pub ret: Option<KTy>,
    pub locals: Vec<Local>,
    pub body: Vec<Stmt>,
    pub is_pure: bool,
    pub io: bool,
    pub mutates_params: bool,
    /// `pub` and in the entry file: a host (the Godot add-on) may call it.
    pub exported: bool,
    /// The packed lines (see `pack_line`) of the declaration's first and last line: `(0, 0)` for a
    /// generated function. A closure's code has the literal's lines. The report finds the function an
    /// inlined item came from by them.
    pub lines: (u32, u32),
    /// For a `native` function: the symbol it imports (see `native_symbol`).
    /// It has no body; Rust implements it with Kestrel's calling convention.
    pub native: Option<String>,
}

/// The start of the name of an assign helper's temporary that holds today's copy because the
/// target's own list, array or map has too little room (see `lower::assign_into`): the report
/// words that copy as the assignment's. (A name starting with `__` is one the compiler made.)
pub(crate) const ROOM_COPY: &str = "__room";

/// The name of each local `hoist::hoist_math` makes (see `Module::hoisted`).
pub(crate) const HOISTED: &str = "__hoisted";

#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub name: String,
    pub ty: KTy,
    pub kind: LocalKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind {
    /// Lives in a register.
    Value,
    /// Needs a memory slot: lowering (and inlining) assigns it to a struct, or to a local passed
    /// as a `var` argument somewhere. `scalarize` may turn it into `Value` (a number whose `var`
    /// argument inlining removed) or `Unused` (a plain struct, split into one `Value` local per
    /// field).
    Addressed,
    /// Holds a pointer to the caller's value: a `var` parameter, or any
    /// aggregate parameter (read-only ones are passed by pointer too). Also
    /// a `for ... in` element (`Stmt::Bind`), a heap constant (`Stmt::BindGlobal`) or a closure's
    /// capture (`Stmt::BindEnv`).
    RefParam,
    /// Nothing refers to it any more: a struct `scalarize` split into one local per field.
    Unused,
}

/// A call the inliner left in place.
#[derive(Clone, Debug, PartialEq)]
pub struct NotInlined {
    /// The function holding the call (after inlining, so a call inside an inlined body belongs to
    /// the function it was inlined into).
    pub caller: FuncIdx,
    /// The packed line of the call (see `pack_line`). A call statement has no line of its own: it
    /// takes its first argument's line, or the line of the latest expression before it.
    pub line: u32,
    pub callee: FuncIdx,
    /// Why, as a clause: "it calls itself", "it is 912 nodes; the limit is 256", "it is 5,812 nodes
    /// after its own inlining; the limit is 4,096", "it has an early `return`", "the inlining budget
    /// of `bench` ran out", "inlined calls nest more than 16 deep" (the callee already holds copies
    /// nesting 16 deep), "it calls back into `walk`" (a call back into a cycle of calls, left in
    /// the one-level copy of a function of the cycle; it names the function called). A native's call
    /// isn't listed.
    pub reason: String,
}
