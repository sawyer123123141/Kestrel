use std::collections::{HashMap, HashSet};

use kestrel_syntax::ast::{BindId, ExprId, FnDecl, Item, Program};
use kestrel_syntax::Span;

use crate::builtins::Builtin;
use crate::closure::ClosureInfo;
use crate::generic::{CopyInfo, GenericOwner, Generics, Instance, ParamId};
use crate::sig::{Sig, SigId};
use crate::ty::Ty;

pub use crate::list_method::ListMethod;
pub use crate::str_method::StrMethod;
pub use crate::tables_iface::{IfaceId, IfaceInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FnId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConstId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructId(pub u32);

/// The built-in vector types are the first structs, before any user struct (see `vectors`).
pub const VEC2: StructId = StructId(0);
pub const VEC3: StructId = StructId(1);
pub const IVEC2: StructId = StructId(2);
pub const IVEC3: StructId = StructId(3);
pub const BUILTIN_STRUCTS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnumId(pub u32);

/// An interned element type of an array or list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElemId(pub u32);

/// What a name or call expression refers to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Res {
    Local(LocalId),
    Const(ConstId),
    Fn(FnId),
    Print,
    Convert(Ty),
    /// `vec4(...)`: builds a SIMD vector of this type (`ctor_args` per lane, or `vec_makes`).
    SimdMake(Ty),
    /// `Point(...)`: builds a struct.
    Construct(StructId),
    /// `Block.Air` or `Block.Stone(...)`: builds an enum value.
    Variant(EnumId, u32),
    /// `xs.add(v)` on a list.
    ListAdd,
    /// `xs.remove_at(i)` and the other list methods besides `add` (see `ListMethod`).
    List(ListMethod),
    /// `s.len`, `s[i]` and the string methods (see `StrMethod`).
    Str(StrMethod),
    /// `m.remove(k)`: changes the map.
    MapRemove,
    /// `m.get(k, d)` / `m.has(k)`: reads the map.
    MapRead,
    /// `Arena()`
    NewArena,
    /// A global built-in function or constant (`sqrt(x)`, `PI`).
    Builtin(Builtin),
    /// A user function named as a value (`let f = score`): its type is `Ty::Fn(fn_sig(f))`.
    FnValue(FnId),
    /// A call through a function value (a local by name, a struct field, or `CallValue`).
    /// For a field call (`b.on_click()`), `field_index` of the call has the field.
    CallValue(SigId),
    /// `d.hp()` where `d` is an interface value: the method in slot `slot` of the interface's method
    /// table, which is `Tables::iface(iface).methods[slot]` (declaration order).
    IfaceCall { iface: IfaceId, slot: u32 },
    /// `x.hp()` where `x` is a `T` and a rule of `T` has the method: the method in slot `slot` of
    /// interface `iface`, one of `param`'s rules (the first whose method is `pure`, else the first
    /// that has it). The call is checked against the interface's abstract method.
    ParamCall { param: ParamId, iface: IfaceId, slot: u32 },
}

#[derive(Clone, Debug)]
pub struct LocalInfo {
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
    pub is_param: bool,
    pub is_var_param: bool,
    /// Passed as a `var` argument somewhere, so it needs a memory slot.
    pub addressed: bool,
    /// The function the local is written in; `None` in a field default or a constant, where
    /// only a closure's own locals exist.
    pub owner: Option<FnId>,
    /// The closure literal the local belongs to (a parameter or a variable of its body), if any.
    /// A closure's locals are listed in its `ClosureInfo`, not in its function's `FnInfo::locals`.
    pub closure: Option<ExprId>,
    pub span: Span,
    /// A `match` or `for ... in` binding aliases the variable it reads
    /// from: changes go to it.
    pub alias: Option<LocalId>,
}

#[derive(Clone, Debug)]
pub struct ParamSig {
    pub name: String,
    pub ty: Ty,
    pub is_var: bool,
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    pub name: String,
    /// Index into `Program::items`.
    pub item: usize,
    /// For a method: its index in its struct's or enum's `methods`.
    pub method: Option<usize>,
    pub is_pure: bool,
    /// Declared `native`: Rust implements it, so there is no body to check.
    pub is_native: bool,
    pub params: Vec<ParamSig>,
    pub ret: Ty,
    /// The function's type as a value (see `Tables::fn_sig`).
    pub sig: SigId,
    pub span: Span,
    /// Every local owned by this function, parameters first in order.
    pub locals: Vec<LocalId>,
    pub io: bool,
    pub mutates_params: bool,
    /// An interface's method: a signature with no body. It is never called or checked; KIR
    /// lowers it as a stub (see `kir::lower`).
    pub is_abstract: bool,
    /// The generic whose placeholders its types use: its own for a generic function, its
    /// struct's or enum's for a method of a generic type; `None` for plain code.
    pub generic: Option<GenericOwner>,
    /// A generic function's own placeholders, in order (a method's are its type's).
    pub type_params: Vec<ParamId>,
    /// For a copy of a generic function or method: what it was made from.
    pub copy: Option<CopyInfo>,
}

impl FnInfo {
    /// The function's (or method's) declaration in the AST.
    pub fn decl<'p>(&self, program: &'p Program) -> &'p FnDecl {
        match (&program.items[self.item], self.method) {
            (Item::Fn(f), None) => f,
            (Item::Struct(s), Some(m)) => &s.methods[m],
            (Item::Enum(e), Some(m)) => &e.methods[m],
            (Item::Interface(i), Some(m)) => &i.methods[m],
            _ => unreachable!("FnInfo points at a function or a method"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StructInfo {
    pub name: String,
    /// Index into `Program::items`; `usize::MAX` for built-in structs.
    pub item: usize,
    pub span: Span,
    pub fields: Vec<FieldInfo>,
    pub methods: HashMap<String, FnId>,
    /// The interfaces it says it implements (`struct S: A, B`), in the order written, each once.
    pub ifaces: Vec<IfaceId>,
    /// A generic declaration's placeholders (its fields use them); empty for everything else.
    pub params: Vec<ParamId>,
    /// For an instance (`Stack[int]`): its declaration and type arguments.
    pub instance: Option<Instance<StructId>>,
    /// No placeholder anywhere: false for a generic declaration and for an instance whose
    /// arguments hold one (`Stack[T]` inside other generic code).
    pub concrete: bool,
}

#[derive(Clone, Debug)]
pub struct EnumInfo {
    pub name: String,
    pub item: usize,
    pub span: Span,
    pub variants: Vec<VariantInfo>,
    pub methods: HashMap<String, FnId>,
    pub ifaces: Vec<IfaceId>,
    /// As for `StructInfo`.
    pub params: Vec<ParamId>,
    pub instance: Option<Instance<EnumId>>,
    pub concrete: bool,
}

#[derive(Clone, Debug)]
pub struct VariantInfo {
    pub name: String,
    pub span: Span,
    pub fields: Vec<FieldInfo>,
}

#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
    pub has_default: bool,
    /// The default puts memory in the arena the struct is built in (`[1, 2]`, `array[T](n)`, a
    /// list, a constant that holds one). Only set for struct fields.
    pub default_allocates: bool,
}

#[derive(Clone, Debug)]
pub struct ConstInfo {
    pub name: String,
    pub ty: Ty,
    pub item: usize,
}

/// Everything the checker learned, keyed by AST ids.
#[derive(Debug, Default)]
pub struct Tables {
    pub expr_ty: Vec<Ty>,
    pub coerce: HashMap<ExprId, Ty>,
    /// The arguments that are lent to an interface parameter instead of being copied: each is
    /// directly a concrete struct or enum **place** (as KIR's `place()` defines one: a local, a
    /// field, a list or array element, a heap constant; not a map lookup or a string index) passed
    /// to a parameter of interface type, `var` or not. Each also has a `coerce` entry. KIR borrows
    /// exactly these and boxes every other coercion to an interface.
    pub iface_borrows: HashSet<ExprId>,
    pub res: HashMap<ExprId, Res>,
    pub bind_local: HashMap<BindId, LocalId>,
    /// For a call: entry `i` is the index (into the call's args) of the
    /// argument bound to parameter `i`.
    pub call_args: HashMap<ExprId, Vec<usize>>,
    /// For a call by a local's name (`f(x)`, `Res::CallValue`): the local that holds the function.
    pub call_local: HashMap<ExprId, LocalId>,
    /// For a call by a constant's name (`RULE(3)` with `let RULE = square`, `Res::CallValue`): the
    /// constant that holds the function.
    pub call_const: HashMap<ExprId, ConstId>,
    /// Closure literals, by the literal's id.
    pub closures: HashMap<ExprId, ClosureInfo>,
    /// The expressions in closure bodies that read a capture: per expression, the closure (the
    /// innermost one around it), the index of the capture in its `ClosureInfo::captures`, and the
    /// fields left to follow from the capture's value (a read of `p.pos.x` under the capture
    /// `p.pos`). The expression is the whole read: a name, a field access, a call by a local's
    /// name, or a call of a function field (`b.on(1)` reads `b.on`). Only the code of that
    /// closure may lower these expressions: KIR binds the captures there (`lower::env`).
    pub capture_reads: HashMap<ExprId, (ExprId, u32, Vec<u32>)>,
    /// The closure literals that get a stack environment: each is directly an argument (the
    /// argument expression itself) of a call of a user function or method (`Res::Fn`) or of a
    /// call through a function value (`Res::CallValue`). The checker holds them to the call
    /// exclusivity rule (`body::exclusive`); KIR gives stack environments to exactly these.
    pub stack_closures: HashSet<ExprId>,
    pub locals: Vec<LocalInfo>,
    pub fns: Vec<FnInfo>,
    pub consts: Vec<ConstInfo>,
    pub structs: Vec<StructInfo>,
    pub struct_by_name: HashMap<String, StructId>,
    pub enums: Vec<EnumInfo>,
    pub enum_by_name: HashMap<String, EnumId>,
    pub ifaces: Vec<IfaceInfo>,
    pub iface_by_name: HashMap<String, IfaceId>,
    /// For a `match`: per arm, the variant it matches, or `None` for `else`.
    pub match_arms: HashMap<ExprId, Vec<Option<u32>>>,
    /// For a field access `a.b`: the index of `b` in its struct.
    pub field_index: HashMap<ExprId, u32>,
    /// For a struct or variant constructor: per field, the argument index,
    /// or `None` for the default.
    pub ctor_args: HashMap<ExprId, Vec<Option<usize>>>,
    /// For a lane of a `vec4` (`v.x`, `p.pos.w`): which one, 0 to 3. A lane of a place is a place,
    /// as a field is; a swizzle (`v.xz`) is in `swizzles`.
    pub lanes: HashMap<ExprId, u32>,
    /// For a swizzle like `v.xz`: the components it picks, in order.
    pub swizzles: HashMap<ExprId, Vec<u32>>,
    /// The vector makes that aren't one number per component (no `ctor_args`): a
    /// fill (`vec3(0.5)`), a copy or conversion (`vec3(iv)`) and a mixed make (`vec3(v2, 1)`).
    pub vec_makes: HashSet<ExprId>,
    /// Reads of owning variables that move them (they can't be copied).
    pub moves: HashSet<ExprId>,
    /// Per struct and enum: whether it holds an `Arena` or a `GpuHandle` somewhere inside.
    pub owning_structs: Vec<bool>,
    pub owning_enums: Vec<bool>,
    /// Element types of arrays and lists, indexed by `ElemId`.
    pub elems: Vec<Ty>,
    /// Every map type written or inferred, with where; their value types
    /// are checked once it's known which types own an arena.
    pub map_types: Vec<(Ty, Span)>,
    /// `for ... in` loops over maps that could gain keys while they run (by
    /// their iterated expression), which lock the map. Loops over maps that
    /// can't change there (a `let`, a read-only parameter, anything shared
    /// in a `par for` body) don't, so many threads may read one at once.
    pub locked_map_loops: HashSet<ExprId>,
    /// `let x = opt else` bindings that read the optional's payload in place (views), instead of
    /// moving or copying it. KIR maps uses of `x` to the payload's place and doesn't drop `x`.
    pub let_else_views: HashSet<LocalId>,
    /// `for ... in` loops over lists whose body removes the loop's item (`remove b`), by their
    /// iterated expression: KIR compacts the list as such a loop runs.
    pub removing_loops: HashSet<ExprId>,
    /// Each `remove b` statement, by its span (it has no id): the loop item it removes.
    pub removes: HashMap<Span, LocalId>,
    pub elem_ids: HashMap<Ty, ElemId>,
    /// Function signatures, indexed by `SigId` (see `intern_sig`).
    pub sigs: Vec<Sig>,
    pub sig_ids: HashMap<Sig, SigId>,
    pub fn_by_name: HashMap<String, FnId>,
    pub const_by_name: HashMap<String, ConstId>,
    /// Every constant in the order `check_consts` checked it: each comes after the constants it uses.
    pub const_order: Vec<ConstId>,
    pub main: Option<FnId>,
    /// A reloadable program's `setup`, `frame` and state type (it has no `main`; see `reload`).
    pub reload: Option<crate::reload::ReloadShape>,
    /// Copied from the program: where the built-in modules' source is. Only they may
    /// name `GpuHandle` (see `in_builtin`).
    pub builtin_ranges: Vec<std::ops::Range<u32>>,
    /// Copied from the program: the integer literals written in hex, which may use their type's
    /// full width (see `hex_literal`).
    pub hex_literals: HashSet<ExprId>,
    /// The hex literals above their type's positive range but within its bit width (`0xFFFFFFFF`
    /// as an `int`), by id: the value is the bit pattern read as the type's signed integer
    /// (-1 there). KIR reads the literal's value from here.
    pub wrapped_literals: HashMap<ExprId, i64>,
    /// Placeholders, type lists, instances and copies (see `generic`).
    pub generic: Generics,
    /// Stores into `var` parameters, for `var_stores::resolve` (P38); empty once checking ends.
    pub var_stores: crate::var_stores::VarStores,
}

impl Tables {
    pub fn ty(&self, e: ExprId) -> Ty {
        self.expr_ty[e.0 as usize]
    }

    pub fn local(&self, id: LocalId) -> &LocalInfo {
        &self.locals[id.0 as usize]
    }

    pub fn func(&self, id: FnId) -> &FnInfo {
        &self.fns[id.0 as usize]
    }

    pub fn strukt(&self, id: StructId) -> &StructInfo {
        &self.structs[id.0 as usize]
    }

    pub fn enm(&self, id: EnumId) -> &EnumInfo {
        &self.enums[id.0 as usize]
    }

    pub fn elem(&self, id: ElemId) -> Ty {
        self.elems[id.0 as usize]
    }

    /// Whether `span` starts inside the source of a built-in module.
    pub fn in_builtin(&self, span: Span) -> bool {
        self.builtin_ranges.iter().any(|r| r.contains(&span.start))
    }
}
