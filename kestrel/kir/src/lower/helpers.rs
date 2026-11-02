//! Functions generated during lowering, one per type that needs them: deep
//! copies and printing of arrays and lists (a string copies in place with
//! `Stmt::Dup`). Structs and enums are handled inline; collections always go
//! through a call, so types that contain themselves through a collection
//! (`struct Node { kids: [Node] }`) work.

use std::collections::HashMap;

use kestrel_check::{ListMethod, Subst, Tables};
use kestrel_syntax::ast::ExprId;

use super::consts::ConstGlobals;
use super::list_fns::Callee;
use super::kty;
pub(crate) use super::heap::{has_heap, owning};
use crate::ir::*;

/// `place`'s optional holds a value: its tag isn't 0.
pub(super) fn has_value(place: &Place) -> Expr {
    let tag = expr(ExprKind::Tag(place.clone()), KTy::U8);
    let zero = expr(ExprKind::Int(0), KTy::U8);
    expr(ExprKind::Binary(BinOp::Ne, Box::new(tag), Box::new(zero)), KTy::Bool)
}

pub(super) fn elem_of(ty: KTy, t: &Tables) -> KTy {
    match ty {
        KTy::Array(e) | KTy::List(e) => kty(t.elems[e.0 as usize]),
        other => unreachable!("{other:?} has no elements"),
    }
}

pub(super) fn expr(kind: ExprKind, ty: KTy) -> Expr {
    Expr { kind, ty, line: 0 }
}

/// `tag(place) == variant`
pub(super) fn tag_is(place: &Place, variant: u32) -> Expr {
    let tag = expr(ExprKind::Tag(place.clone()), KTy::U8);
    let v = expr(ExprKind::Int(variant as i64), KTy::U8);
    expr(ExprKind::Binary(BinOp::Eq, Box::new(tag), Box::new(v)), KTy::Bool)
}

pub(super) fn with(place: &Place, proj: Proj) -> Place {
    let mut p = place.clone();
    p.path.push(proj);
    p
}

/// A closure literal under the substitution it is lowered under (P4): a copy of a generic, or an
/// instance's field default, gets its own code, environment and layout; plain code has `None`.
pub(crate) type LiteralKey = (ExprId, Option<Subst>);

pub(crate) struct Helpers {
    /// The index of the first helper: helpers follow the user's functions.
    pub(super) base: u32,
    pub(crate) funcs: Vec<Func>,
    pub(super) copy: HashMap<KTy, FuncIdx>,
    /// The `checksum` fold of each type that isn't one word (see `checksum`).
    pub(super) checksum: HashMap<KTy, FuncIdx>,
    /// The `to_bytes` function of each type, and the size and write functions behind it (see
    /// `bytes`).
    pub(super) to_bytes: HashMap<KTy, FuncIdx>,
    pub(super) bytes_size: HashMap<KTy, FuncIdx>,
    pub(super) bytes_write: HashMap<KTy, FuncIdx>,
    /// The `first_difference` walk of each type, and the function behind each call (see
    /// `difference`).
    pub(super) difference: HashMap<KTy, FuncIdx>,
    pub(super) first_difference: HashMap<KTy, FuncIdx>,
    /// The `from_bytes` function of each type, and the read function of each type it reads (see
    /// `bytes_read`).
    pub(super) from_bytes: HashMap<KTy, FuncIdx>,
    pub(super) bytes_read: HashMap<KTy, FuncIdx>,
    /// The storage-reusing assignment of each type assigned that way (see `assign_into`).
    pub(super) assign: HashMap<KTy, FuncIdx>,
    /// The locals of the assign helper being built (see `assign_into`), which its copies add
    /// temporaries to.
    pub(super) assign_locals: Vec<Local>,
    drop: HashMap<KTy, FuncIdx>,
    pub(crate) print: HashMap<KTy, FuncIdx>,
    /// The search behind `has` and `find`, per list or array type.
    pub(super) find: HashMap<KTy, FuncIdx>,
    /// The `==` of each struct or enum that doesn't compare inline (see `eq`).
    pub(super) eq: HashMap<KTy, FuncIdx>,
    /// The loop behind each `filter`, `map`, `any` and `all` (see `list_fn`).
    pub(super) list_fns: HashMap<(ListMethod, KTy, Callee), FuncIdx>,
    /// The compare function behind each `sort` (no callee) and string-key `sort_by`, or the key
    /// function behind each other `sort_by`, per item type (see `sort`).
    pub(super) sort_less: HashMap<(KTy, Option<Callee>), FuncIdx>,
    /// The function a `sort_by` call site calls, per list type and key.
    pub(super) sort_calls: HashMap<(KTy, Callee), FuncIdx>,
    /// Each `sort_by` compare or key function with the code of the literal key it calls.
    pub(super) sort_idle: Vec<(FuncIdx, FuncIdx)>,
    /// The thunk of each function used as a value (see `thunk_for`).
    pub(super) thunks: HashMap<FuncIdx, FuncIdx>,
    /// The code of each closure literal (see `closure_code`).
    pub(super) closures: HashMap<LiteralKey, FuncIdx>,
    /// The copy function of each closure literal's environment (see `env_copy_fn`).
    pub(super) env_copies: HashMap<LiteralKey, FuncIdx>,
    /// The layout of each closure literal's environment (see `env_layout`).
    pub(super) env_layouts: HashMap<LiteralKey, EnvLayout>,
    pub(crate) par_bodies: Vec<FuncIdx>,
    /// The heap constants the functions use, and their globals.
    pub(super) consts: ConstGlobals,
    /// The method tables of interface values (see `tables`).
    pub(crate) ifaces: super::tables::IfaceTables,
    /// Each `@assume` condition lowered so far, by its id (see `assume`).
    pub(crate) assumptions: Vec<AssumeInfo>,
    /// The program's `@prove`s and the contract sites made so far (see `prove`).
    pub(crate) proves: super::prove::Proves,
}

impl Helpers {
    pub(crate) fn new(base: u32) -> Self {
        let (copy, drop, print) = (HashMap::new(), HashMap::new(), HashMap::new());
        let (find, thunks, closures, env_copies) = (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new());
        let (funcs, par_bodies, consts) = (Vec::new(), Vec::new(), ConstGlobals::default());
        let (env_layouts, list_fns) = (HashMap::new(), HashMap::new());
        let (sort_less, sort_calls, sort_idle) = (HashMap::new(), HashMap::new(), Vec::new());
        Helpers {
            base,
            funcs,
            copy,
            checksum: HashMap::new(),
            to_bytes: HashMap::new(),
            bytes_size: HashMap::new(),
            bytes_write: HashMap::new(),
            difference: HashMap::new(),
            first_difference: HashMap::new(),
            from_bytes: HashMap::new(),
            bytes_read: HashMap::new(),
            assign: HashMap::new(),
            assign_locals: Vec::new(),
            drop,
            print,
            find,
            eq: HashMap::new(),
            list_fns,
            sort_less,
            sort_calls,
            sort_idle,
            thunks,
            closures,
            env_copies,
            env_layouts,
            par_bodies,
            consts,
            ifaces: Default::default(),
            assumptions: Vec::new(),
            proves: Default::default(),
        }
    }

    /// Adds an outlined `par for` body, named for its index.
    pub(crate) fn add_par_body(&mut self, mut f: Func) -> FuncIdx {
        let idx = FuncIdx(self.base + self.funcs.len() as u32);
        // The body's statements moved here, with any annotated loop in them.
        self.repoint(&f.body, idx);
        f.name = format!("__kestrel_par_{}", idx.0);
        self.funcs.push(f);
        self.par_bodies.push(idx);
        idx
    }

    /// Statements that give the value at `place`, just copied byte for
    /// byte, its own copy of every string, array and list inside it.
    pub(crate) fn fixup(&mut self, place: Place, ty: KTy, t: &Tables) -> Vec<Stmt> {
        let mut out = Vec::new();
        match ty {
            KTy::Str => out.push(Stmt::Dup(place)),
            // A function value's environment holds copies of its captures; the copy gets its own.
            KTy::Fn => out.push(Stmt::CopyFnEnv(place)),
            // An interface value's copy gets its own copy of the concrete value, through its table.
            KTy::Iface => out.push(Stmt::CopyIface(place)),
            KTy::Array(_) | KTy::List(_) => out.push(Stmt::Call(self.copy_fn(ty, t), vec![Arg::Ref(place)], 0)),
            KTy::Map(..) => out.push(Stmt::Call(self.map_copy_fn(ty, t), vec![Arg::Ref(place)], 0)),
            KTy::Optional(e) => {
                let then_body = self.fixup(with(&place, Proj::Payload), kty(t.elems[e.0 as usize]), t);
                if !then_body.is_empty() {
                    out.push(Stmt::If { cond: has_value(&place), then_body, else_body: Vec::new() });
                }
            }
            KTy::Struct(s) => {
                for (i, f) in t.structs[s.0 as usize].fields.iter().enumerate() {
                    if has_heap(kty(f.ty), t) {
                        out.extend(self.fixup(with(&place, Proj::Field(i as u32)), kty(f.ty), t));
                    }
                }
            }
            KTy::Enum(e) => {
                for (v, variant) in t.enums[e.0 as usize].variants.iter().enumerate() {
                    let mut then_body = Vec::new();
                    for (i, f) in variant.fields.iter().enumerate() {
                        if has_heap(kty(f.ty), t) {
                            let p = with(&place, Proj::Variant(v as u32, i as u32));
                            then_body.extend(self.fixup(p, kty(f.ty), t));
                        }
                    }
                    if !then_body.is_empty() {
                        out.push(Stmt::If { cond: tag_is(&place, v as u32), then_body, else_body: Vec::new() });
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Statements that free every arena and drop every GPU object inside the value at `place`.
    pub(crate) fn drop_stmts(&mut self, place: Place, ty: KTy, t: &Tables) -> Vec<Stmt> {
        let mut out = Vec::new();
        match ty {
            _ if !owning(ty, t) => {}
            KTy::Arena => out.push(Stmt::FreeArena(place)),
            KTy::Gpu => out.push(Stmt::DropGpu(place)),
            KTy::Optional(e) => {
                let then_body = self.drop_stmts(with(&place, Proj::Payload), kty(t.elems[e.0 as usize]), t);
                out.push(Stmt::If { cond: has_value(&place), then_body, else_body: Vec::new() });
            }
            KTy::Array(_) | KTy::List(_) => out.push(Stmt::Call(self.drop_fn(ty, t), vec![Arg::Ref(place)], 0)),
            KTy::Struct(s) => {
                // Other fields' memory may live in the struct's own arenas, so those go last.
                let fields = &t.structs[s.0 as usize].fields;
                let order = fields.iter().enumerate().filter(|(_, f)| kty(f.ty) != KTy::Arena);
                for (i, f) in order.chain(fields.iter().enumerate().filter(|(_, f)| kty(f.ty) == KTy::Arena)) {
                    out.extend(self.drop_stmts(with(&place, Proj::Field(i as u32)), kty(f.ty), t));
                }
            }
            KTy::Enum(e) => {
                for (v, variant) in t.enums[e.0 as usize].variants.iter().enumerate() {
                    let mut then_body = Vec::new();
                    let fields = variant.fields.iter().enumerate();
                    let order = fields.clone().filter(|(_, f)| kty(f.ty) != KTy::Arena);
                    for (i, f) in order.chain(fields.filter(|(_, f)| kty(f.ty) == KTy::Arena)) {
                        then_body.extend(self.drop_stmts(with(&place, Proj::Variant(v as u32, i as u32)), kty(f.ty), t));
                    }
                    if !then_body.is_empty() {
                        out.push(Stmt::If { cond: tag_is(&place, v as u32), then_body, else_body: Vec::new() });
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// `drop(var xs)`: drops each element.
    fn drop_fn(&mut self, ty: KTy, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.drop.get(&ty) {
            return f;
        }
        let idx = self.reserve();
        self.drop.insert(ty, idx);
        let elem = elem_of(ty, t);
        let body = vec![Self::each(|at| self.drop_stmts(at, elem, t))];
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_drop_{}", idx.0),
            params: vec![LocalIdx(0)],
            ret: None,
            locals: Self::locals(ty),
            body,
            is_pure: true,
            io: false,
            mutates_params: true,
            exported: false,
            lines: (0, 0),
            native: None,
        };
        idx
    }

    /// Reserves a slot for a helper, so a type can refer to its own helper.
    pub(super) fn reserve(&mut self) -> FuncIdx {
        let idx = FuncIdx(self.base + self.funcs.len() as u32);
        let placeholder = Func {
            name: String::new(),
            params: Vec::new(),
            ret: None,
            locals: Vec::new(),
            body: Vec::new(),
            is_pure: true,
            io: false,
            mutates_params: false,
            exported: false,
            lines: (0, 0),
            native: None,
        };
        self.funcs.push(placeholder);
        idx
    }

    /// Locals of a helper: the collection (by pointer) and a loop index.
    pub(super) fn locals(ty: KTy) -> Vec<Local> {
        vec![
            Local { name: "xs".into(), ty, kind: LocalKind::RefParam },
            Local { name: "i".into(), ty: KTy::I64, kind: LocalKind::Value },
        ]
    }

    /// `for i from 0 to xs.len { body(xs[i]) }`
    pub(super) fn each(body: impl FnOnce(Place) -> Vec<Stmt>) -> Stmt {
        let (xs, i) = (Place::local(LocalIdx(0)), LocalIdx(1));
        // The body never moves the elements, so `i < xs.len` holds throughout.
        let at = with(&xs, Proj::Index { at: expr(ExprKind::Load(Place::local(i)), KTy::I64), check: IndexCheck::Known });
        Stmt::ForRange {
            var: i,
            start: expr(ExprKind::Int(0), KTy::I64),
            end: expr(ExprKind::Len(xs), KTy::I64),
            body: body(at),
        }
    }

    /// `copy(var xs)`: a fresh buffer for `xs`, then each element's own copies.
    fn copy_fn(&mut self, ty: KTy, t: &Tables) -> FuncIdx {
        if let Some(&f) = self.copy.get(&ty) {
            return f;
        }
        let idx = self.reserve();
        self.copy.insert(ty, idx);
        let elem = elem_of(ty, t);
        let mut body = vec![Stmt::Dup(Place::local(LocalIdx(0)))];
        if has_heap(elem, t) {
            let each = Self::each(|at| self.fixup(at, elem, t));
            if !matches!(&each, Stmt::ForRange { body, .. } if body.is_empty()) {
                body.push(each);
            }
        }
        self.funcs[(idx.0 - self.base) as usize] = Func {
            name: format!("__kestrel_copy_{}", idx.0),
            params: vec![LocalIdx(0)],
            ret: None,
            locals: Self::locals(ty),
            body,
            is_pure: true,
            io: false,
            mutates_params: true,
            exported: false,
            lines: (0, 0),
            native: None,
        };
        idx
    }
}
