pub use crate::ast_ops::*;

use crate::span::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BindId(pub u32);

#[derive(Clone, Debug)]
pub struct Program {
    pub items: Vec<Item>,
    /// Total `ExprId`s handed out; side tables size by this.
    pub expr_count: u32,
    pub bind_count: u32,
    /// The span offsets of the source of Kestrel's built-in modules (which may use types
    /// that user code can't, like `GpuHandle`). Empty from the parser and `link`; the
    /// driver, which knows which files are built in, fills it after linking.
    pub builtin_ranges: Vec<std::ops::Range<u32>>,
    /// The `@prove` annotations, kept beside the tree and keyed by the span of the function,
    /// method or loop each is on, so no node changes shape. Spans are unique, so a target has one.
    pub proves: Vec<ProveAnnot>,
    /// The integer literals written in hex (`0xFF`), by id, kept beside the tree so `ExprKind::Int`
    /// stays as it is. A hex literal may use its type's full width: `0xFFFFFFFF` is an `int`'s bit pattern.
    pub hex_literals: Vec<ExprId>,
}

/// What a `@prove` promises (see `2026-10-18-at-annotations-design.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Contract {
    NoAlloc,
    NoChecks,
    NoPanic,
    Inline,
    /// The result, and every change to `var` parameters, depends only on the arguments.
    Deterministic,
}

impl Contract {
    pub fn name(self) -> &'static str {
        match self {
            Contract::NoAlloc => "no_alloc",
            Contract::NoChecks => "no_checks",
            Contract::NoPanic => "no_panic",
            Contract::Inline => "inline",
            Contract::Deterministic => "deterministic",
        }
    }

    pub fn from_name(name: &str) -> Option<Contract> {
        [Contract::NoAlloc, Contract::NoChecks, Contract::NoPanic, Contract::Inline, Contract::Deterministic].into_iter().find(|c| c.name() == name)
    }
}

/// The `@prove` annotations on one function, method or loop, merged: each contract once, in the
/// order written, with where it is written.
#[derive(Clone, Debug)]
pub struct ProveAnnot {
    /// The span of the `FnDecl` or loop statement it is on.
    pub target: Span,
    pub contracts: Vec<(Contract, Span)>,
    /// From the first `@` to the last `)`.
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Item {
    Fn(FnDecl),
    Const(ConstDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Interface(InterfaceDecl),
    Use(UseDecl),
}

/// `use math`, `use { noise2, lerp from math }`, or several clauses in braces.
#[derive(Clone, Debug)]
pub struct UseDecl {
    pub clauses: Vec<UseClause>,
    pub span: Span,
}

/// One module: with `names`, those come into scope unqualified; without,
/// the module's `pub` items are reached as `module.name`.
#[derive(Clone, Debug)]
pub struct UseClause {
    pub module: String,
    pub module_span: Span,
    pub names: Vec<(String, Span)>,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub is_pub: bool,
    pub name: String,
    pub name_span: Span,
    /// `enum E[T, K]`: the placeholders, empty for an ordinary enum.
    pub type_params: Vec<TypeParam>,
    /// `enum E: A, B`: the interfaces it says it implements, with where each is written.
    pub ifaces: Vec<(String, Span)>,
    pub variants: Vec<VariantDecl>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

/// `interface Name { methods }`: a list of method signatures. Each method is a `FnDecl` with
/// an empty body (spanned at the end of the signature), like a native's; its `self` is typed
/// as the interface.
#[derive(Clone, Debug)]
pub struct InterfaceDecl {
    pub is_pub: bool,
    pub name: String,
    pub name_span: Span,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct VariantDecl {
    pub name: String,
    pub name_span: Span,
    /// Named fields; empty for a unit variant. Fields never have defaults.
    pub fields: Vec<FieldDecl>,
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub is_pub: bool,
    pub name: String,
    pub name_span: Span,
    /// `struct S[T]`: the placeholders, empty for an ordinary struct.
    pub type_params: Vec<TypeParam>,
    /// `struct S: A, B`: the interfaces it says it implements, with where each is written.
    pub ifaces: Vec<(String, Span)>,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub name: String,
    pub name_span: Span,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: String,
    pub name_span: Span,
    pub is_pure: bool,
    pub is_pub: bool,
    /// `native name(params)`: implemented in Rust, with no body. `body` is
    /// then empty, spanned at the end of the signature.
    pub is_native: bool,
    /// `best[T: Named](...)`: the placeholders, empty for an ordinary function. Methods, natives
    /// and interface methods keep it empty.
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
    pub span: Span,
}

/// One placeholder of a generic struct, enum or function: `T`, or `T: Named + ordered` with the
/// rules (interface names or built-in rule words) a type must meet to take its place.
#[derive(Clone, Debug)]
pub struct TypeParam {
    pub name: String,
    pub span: Span,
    pub rules: Vec<(String, Span)>,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub bind: Binding,
    pub is_var: bool,
    /// `self` / `var self`: the type is the enclosing struct, filled in by the parser.
    pub is_self: bool,
    pub ty: TypeExpr,
}

#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub is_pub: bool,
    pub bind: Binding,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

/// A name being introduced: `let` target, parameter, loop variable.
#[derive(Clone, Debug)]
pub struct Binding {
    pub id: BindId,
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TypeExpr {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TypeKind {
    Named { name: String, args: Vec<TypeExpr> },
    Array(Box<TypeExpr>),
    Optional(Box<TypeExpr>),
    /// `(A, var B) -> R`, `(A)` (gives nothing) or `pure (A) -> R`: the type of a function value.
    Fn { params: Vec<FnTypeParam>, ret: Option<Box<TypeExpr>>, is_pure: bool },
}

/// One parameter of a function type: `A`, or `var A` for one the function changes in place.
#[derive(Clone, Debug)]
pub struct FnTypeParam {
    pub is_var: bool,
    pub ty: TypeExpr,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// `let x = v`, or `let x = opt else { ... }`, which unwraps an optional (or a `Result`) and
    /// runs the `else` block (which must leave) when it's `none` (or an error). `else_name` is the
    /// name the error gets in the block: `let x = r else why { ... }`.
    Let { is_var: bool, bind: Binding, ty: Option<TypeExpr>, value: Expr, else_name: Option<Binding>, else_block: Option<Block>, span: Span },
    Assign { target: Expr, op: AssignOp, value: Expr, span: Span },
    While { cond: Expr, body: Block, span: Span },
    ForRange { var: Binding, start: Expr, end: Expr, body: Block, span: Span },
    /// `for v in xs` / `for i, v in xs`
    /// `for v in xs` / `for i, v in xs`; with `par`, iterations run at the same time.
    ForIn { par: bool, index: Option<Binding>, var: Binding, iter: Expr, body: Block, span: Span },
    /// `arena { ... }`: everything allocated inside is freed at `}`.
    Arena { body: Block, span: Span },
    /// `in x.mem { ... }`: allocations inside go to the arena at `x.mem`.
    In { arena: Expr, body: Block, span: Span },
    Break(Span),
    Continue(Span),
    Return { value: Option<Expr>, span: Span },
    /// `remove b`: removes the current item `b` of a `for b in xs` loop over a list.
    Remove { name: String, name_span: Span, span: Span },
    /// `@assume(cond)`: a fact the analysis may use for the rest of the block. `text` is the
    /// condition as written, with each run of white space one space.
    Assume { cond: Expr, text: String, span: Span },
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: ExprId,
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(u64),
    Float(f64),
    Bool(bool),
    /// `none`: an optional with no value.
    None,
    Str(String),
    /// `"hp = {hp}"`: literal text and expressions, in order.
    Interp(Vec<InterpPart>),
    Ident(String),
    Unary { op: UnaryOp, expr: Box<Expr> },
    /// `try r`: the value inside a `Result` or an optional, or else a return of its error (or
    /// `none`) from the function.
    Try(Box<Expr>),
    Binary { op: BinaryOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Call { name: String, name_span: Span, args: Vec<Arg> },
    If { cond: Box<Expr>, then_block: Block, else_block: Option<Block> },
    /// `base.name`
    Field { base: Box<Expr>, name: String, name_span: Span },
    /// `receiver.name(args)`
    MethodCall { receiver: Box<Expr>, name: String, name_span: Span, args: Vec<Arg> },
    Match { scrutinee: Box<Expr>, arms: Vec<MatchArm> },
    /// `base[index]`
    Index { base: Box<Expr>, index: Box<Expr> },
    /// `base[...]` before it is known whether the brackets hold an index or types. Made by the
    /// parser only, and settled (into `Index`, `TypeApply`, `TypeCall` or a type) before anything
    /// else sees it.
    Brackets { base: Box<Expr>, index: Option<Box<Expr>>, types: Vec<TypeExpr> },
    /// `best[int]`, `Stack[int]`: a generic named with types and not called.
    TypeApply { name: String, name_span: Span, type_args: Vec<TypeExpr> },
    /// `[a, b, c]`
    ArrayLit(Vec<Expr>),
    /// `{k: v, ...}`, a map's keys and values in order.
    MapLit(Vec<(Expr, Expr)>),
    /// `array[int](n)`, `list[vec3]()`: a built-in generic type, called.
    TypeCall { name: String, name_span: Span, type_args: Vec<TypeExpr>, args: Vec<Arg> },
    /// `callee(args)` where the callee is neither a bare name (`Call`) nor a method:
    /// `handlers[i](3)`, `make_adder(2)(5)`, `(f)(x)`.
    CallValue { callee: Box<Expr>, args: Vec<Arg> },
    /// `(x: int) -> int { x + 1 }`: a function with no name.
    Closure(Box<ClosureLit>),
}

/// A closure literal: `(params) [-> Type] { body }`. A parameter may leave out its type when
/// the context gives it.
#[derive(Clone, Debug)]
pub struct ClosureLit {
    pub params: Vec<ClosureParam>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub struct ClosureParam {
    pub bind: Binding,
    pub is_var: bool,
    pub ty: Option<TypeExpr>,
}

#[derive(Clone, Debug)]
pub enum InterpPart {
    Lit(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    /// `Variant` or `Variant binding`.
    Variant { name: String, name_span: Span, binding: Option<Binding> },
    Else(Span),
}

#[derive(Clone, Debug)]
pub struct Arg {
    pub label: Option<String>,
    pub is_var: bool,
    pub value: Expr,
    pub span: Span,
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::While { span, .. }
            | Stmt::ForRange { span, .. }
            | Stmt::ForIn { span, .. }
            | Stmt::Arena { span, .. }
            | Stmt::In { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::Remove { span, .. }
            | Stmt::Assume { span, .. } => *span,
            Stmt::Break(span) | Stmt::Continue(span) => *span,
            Stmt::Expr(e) => e.span,
        }
    }

}
