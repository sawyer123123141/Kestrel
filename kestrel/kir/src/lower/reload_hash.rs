//! The hash of a reloadable program's `setup` (hot reload design, decision 4): the host compares it
//! between the old code and the new to say "setup() changed" when a reload keeps the state. It
//! walks `setup`'s lowered code and hashes what each statement, expression and place is: its kind,
//! operators, constants, variant and field numbers, and the *names* of the structs, enums,
//! functions, method tables and heap constants it reaches through the module's tables. Indices
//! into those tables and lines are left out, so the hash changes when `setup` does and not when
//! code around it moves or declarations are reordered.

use super::bytes_fingerprint::checksum_of_text;
use crate::ir::*;
use crate::visit::{walk, Node};

/// The hash of function `setup` of the lowered module `m`.
pub(super) fn setup_hash(m: &Module, setup: FuncIdx) -> u64 {
    let f = &m.funcs[setup.0 as usize];
    let mut h = Hasher { m, f, bytes: Vec::new() };
    h.opt_ty(f.ret);
    h.nums(&[f.params.len() as u64, f.locals.len() as u64]);
    for l in &f.locals {
        h.str(&l.name);
        h.ty(l.ty);
        h.debug(l.kind);
    }
    walk(&f.body, &mut |n| h.node(n));
    checksum_of_text(&h.bytes) as u64
}

/// A function's name without the index a generated one ends with (`__kestrel_copy_57`).
fn stable_name(name: &str) -> &str {
    match name.strip_prefix("__kestrel_") {
        Some(_) => name.trim_end_matches(|c: char| c.is_ascii_digit()),
        None => name,
    }
}

fn len<T>(xs: &[T]) -> u64 {
    xs.len() as u64
}

/// The bytes the hash is taken of. Every string goes in with its length, and every list of
/// children with its count, so two different walks never give the same bytes.
struct Hasher<'m> {
    m: &'m Module,
    /// `setup` itself, for the names of its locals.
    f: &'m Func,
    bytes: Vec<u8>,
}

impl Hasher<'_> {
    fn nums(&mut self, ns: &[u64]) {
        for n in ns {
            self.bytes.extend_from_slice(&n.to_le_bytes());
        }
    }

    fn str(&mut self, s: &str) {
        self.nums(&[s.len() as u64]);
        self.bytes.extend_from_slice(s.as_bytes());
    }

    /// A kind's name and the numbers that go with it.
    fn tag(&mut self, kind: &str, ns: &[u64]) {
        self.str(kind);
        self.nums(ns);
    }

    /// A value whose debug text holds no index and no line: an operator, a kind, a flag.
    fn debug(&mut self, v: impl std::fmt::Debug) {
        self.str(&format!("{v:?}"));
    }

    fn func(&mut self, g: FuncIdx) {
        let m = self.m;
        self.str(stable_name(&m.funcs[g.0 as usize].name));
    }

    fn ty(&mut self, t: KTy) {
        let m = self.m;
        let elem = |e: ElemIdx| m.elems[e.0 as usize];
        match t {
            KTy::Struct(s) => {
                self.tag("struct", &[]);
                self.str(&m.structs[s.0 as usize].name);
            }
            KTy::Enum(e) => {
                self.tag("enum", &[]);
                self.str(&m.enums[e.0 as usize].name);
            }
            KTy::Array(e) => {
                self.tag("array", &[]);
                self.ty(elem(e));
            }
            KTy::List(e) => {
                self.tag("list", &[]);
                self.ty(elem(e));
            }
            KTy::Optional(e) => {
                self.tag("optional", &[]);
                self.ty(elem(e));
            }
            KTy::Map(k, v) => {
                self.tag("map", &[]);
                self.ty(elem(k));
                self.ty(elem(v));
            }
            KTy::Bool | KTy::U8 | KTy::I32 | KTy::I64 | KTy::F32 | KTy::F64 | KTy::F32X4 | KTy::I32X4 => self.debug(t),
            KTy::Str | KTy::Arena | KTy::Gpu | KTy::Fn | KTy::Iface => self.debug(t),
        }
    }

    fn opt_ty(&mut self, t: Option<KTy>) {
        self.nums(&[u64::from(t.is_some())]);
        if let Some(t) = t {
            self.ty(t);
        }
    }

    fn sig(&mut self, s: &KSig) {
        self.nums(&[len(&s.params)]);
        for &(t, by_ref) in &s.params {
            self.ty(t);
            self.nums(&[u64::from(by_ref)]);
        }
        self.opt_ty(s.ret);
        self.nums(&[u64::from(s.pure)]);
    }

    /// The shape of a call's arguments; the values and places in them are walked on their own.
    fn args(&mut self, args: &[Arg]) {
        self.nums(&[len(args)]);
        for a in args {
            self.str(if matches!(a, Arg::Value(_)) { "value" } else { "ref" });
        }
    }

    fn print_parts(&mut self, parts: &[PrintArg]) {
        self.nums(&[len(parts)]);
        for p in parts {
            match p {
                PrintArg::Value(_) => self.tag("value", &[]),
                PrintArg::Str(s) => {
                    self.tag("str", &[]);
                    self.str(s);
                }
                PrintArg::Interp(inner) => {
                    self.tag("interp", &[]);
                    self.print_parts(inner);
                }
            }
        }
    }

    /// A call through an interface value (see `ExprKind::CallVirtual`).
    fn virtual_call(&mut self, slot: u32, sig: &KSig, keeps: ContractSet, method: FuncIdx, args: &[Arg]) {
        self.tag("call virtual", &[u64::from(slot), u64::from(keeps.0)]);
        self.sig(sig);
        self.func(method);
        self.args(args);
    }

    fn env(&mut self, kind: &str, env: &Env) {
        self.tag(kind, &[len(&env.layout.offsets), u64::from(env.layout.size), len(&env.values)]);
        self.nums(&env.layout.offsets.iter().map(|&o| u64::from(o)).collect::<Vec<_>>());
        self.func(env.copy);
    }

    fn node(&mut self, n: Node) {
        match n {
            Node::Stmt(s) => self.stmt(s),
            Node::Expr(e) => {
                self.tag("expr", &[]);
                self.ty(e.ty);
                self.expr(&e.kind);
            }
            Node::Place(p, written) => {
                self.tag("place", &[u64::from(written), u64::from(p.root.0), len(&p.path)]);
                for proj in &p.path {
                    match proj {
                        Proj::Field(i) => self.tag("field", &[u64::from(*i)]),
                        Proj::Variant(v, i) => self.tag("variant", &[u64::from(*v), u64::from(*i)]),
                        Proj::Payload => self.tag("payload", &[]),
                        Proj::Index { check, .. } => {
                            self.tag("index", &[]);
                            self.debug(check);
                        }
                        Proj::MapKey { .. } => self.tag("map key", &[]),
                        Proj::MapValue { checked, .. } => self.tag("map value", &[u64::from(*checked)]),
                        Proj::Lane(l) => self.tag("lane", &[u64::from(*l)]),
                    }
                }
            }
        }
    }

    /// What a statement is, without the statements, expressions and places inside it (the walk
    /// visits those) and without its lines.
    fn stmt(&mut self, s: &Stmt) {
        let (m, f) = (self.m, self.f);
        match s {
            Stmt::Let(l, _) => self.tag("let", &[u64::from(l.0)]),
            Stmt::Assign(..) => self.tag("assign", &[]),
            Stmt::If { then_body, else_body, .. } => self.tag("if", &[len(then_body), len(else_body)]),
            Stmt::While { body, .. } => self.tag("while", &[len(body)]),
            Stmt::ForRange { var, body, .. } => self.tag("for", &[u64::from(var.0), len(body)]),
            Stmt::Break => self.tag("break", &[]),
            Stmt::Continue => self.tag("continue", &[]),
            Stmt::Return(v) => self.tag("return", &[u64::from(v.is_some())]),
            Stmt::Call(g, args, _) => {
                self.tag("call", &[]);
                self.func(*g);
                self.args(args);
            }
            Stmt::CallIndirect { args, sig, .. } => {
                self.tag("call indirect", &[]);
                self.sig(sig);
                self.args(args);
            }
            Stmt::CallVirtual { slot, args, sig, keeps, method, .. } => self.virtual_call(*slot, sig, *keeps, *method, args),
            Stmt::Print(parts) => {
                self.tag("print", &[]);
                self.print_parts(parts);
            }
            Stmt::Write(parts) => {
                self.tag("write", &[]);
                self.print_parts(parts);
            }
            Stmt::RandSeed(_) => self.tag("rand seed", &[]),
            Stmt::ListAdd(..) => self.tag("list add", &[]),
            Stmt::List(op, _, xs) => {
                self.tag("list", &[len(xs)]);
                self.debug(op);
            }
            Stmt::SetLen(..) => self.tag("set len", &[]),
            Stmt::CopyItems(..) => self.tag("copy items", &[]),
            Stmt::Bind(l, _) => self.tag("bind", &[u64::from(l.0)]),
            // A heap constant: by the constant's name (its local's) and type, not its global.
            Stmt::BindGlobal(l, g) => {
                self.tag("bind global", &[u64::from(l.0)]);
                self.str(&f.locals[l.0 as usize].name);
                self.ty(m.globals[g.0 as usize]);
            }
            Stmt::BindEnv { local, offset, .. } => self.tag("bind env", &[u64::from(local.0), u64::from(*offset)]),
            Stmt::CopyFnEnv(_) => self.tag("copy fn env", &[]),
            Stmt::CopyIface(_) => self.tag("copy iface", &[]),
            Stmt::PushArena(p, _) => self.tag("push arena", &[u64::from(p.is_some())]),
            Stmt::PopArena { free } => self.tag("pop arena", &[u64::from(*free)]),
            Stmt::FreeArena(_) => self.tag("free arena", &[]),
            Stmt::DropGpu(_) => self.tag("drop gpu", &[]),
            Stmt::Dup(_) => self.tag("dup", &[]),
            Stmt::MapSet(_, _, _, copy_key) => self.tag("map set", &[u64::from(*copy_key)]),
            Stmt::MapUnlock(..) => self.tag("map unlock", &[]),
            Stmt::ParFor { func, args, .. } => {
                self.tag("par for", &[]);
                self.func(*func);
                self.args(args);
            }
            Stmt::SetLine(_) => self.tag("set line", &[]),
            Stmt::Trap => self.tag("trap", &[]),
            Stmt::Fail(msg) => {
                self.tag("fail", &[]);
                self.str(msg);
            }
            // An assumption and a contract by what they say, not by their ids.
            Stmt::Assume { conds, check, body } => {
                self.tag("assume", &[len(conds), len(body)]);
                for (id, _) in conds {
                    self.str(&m.assumptions[*id as usize].text);
                }
                self.debug(check);
            }
            Stmt::Prove { id, at, body } => {
                let site = &m.contracts[*id as usize];
                self.tag("prove", &[len(&site.contracts), u64::from(*at != 0), len(body)]);
                for (c, _) in &site.contracts {
                    self.debug(c);
                }
            }
        }
    }

    /// What an expression is, without the expressions, statements and places inside it.
    fn expr(&mut self, k: &ExprKind) {
        let m = self.m;
        match k {
            ExprKind::Int(i) => self.tag("int", &[*i as u64]),
            ExprKind::Float(x) => self.tag("float", &[x.to_bits()]),
            ExprKind::Bool(b) => self.tag("bool", &[u64::from(*b)]),
            ExprKind::StrLit(s) => {
                self.tag("string", &[]);
                self.str(s);
            }
            ExprKind::Format(parts) => {
                self.tag("format", &[]);
                self.print_parts(parts);
            }
            ExprKind::ArrayNew(_) => self.tag("array new", &[]),
            ExprKind::ArrayLit(xs) => self.tag("array lit", &[len(xs)]),
            ExprKind::ListNew => self.tag("list new", &[]),
            ExprKind::MapNew => self.tag("map new", &[]),
            ExprKind::Map(op, _, xs) => {
                self.tag("map", &[len(xs)]);
                self.debug(op);
            }
            ExprKind::Len(_) => self.tag("len", &[]),
            ExprKind::ReadBytes(_, xs) => self.tag("read bytes", &[len(xs)]),
            ExprKind::Cap(_) => self.tag("cap", &[]),
            ExprKind::Str(op, xs) => {
                self.tag("str", &[len(xs)]);
                self.debug(op);
            }
            ExprKind::NewArena => self.tag("new arena", &[]),
            ExprKind::StaticArena => self.tag("static arena", &[]),
            ExprKind::Some(_) => self.tag("some", &[]),
            ExprKind::NoneLit => self.tag("none", &[]),
            ExprKind::Move(_) => self.tag("move", &[]),
            ExprKind::Load(_) => self.tag("load", &[]),
            ExprKind::Unary(op, _) => {
                self.tag("unary", &[]);
                self.debug(op);
            }
            ExprKind::Binary(op, ..) => {
                self.tag("binary", &[]);
                self.debug(op);
            }
            ExprKind::And(..) => self.tag("and", &[]),
            ExprKind::Or(..) => self.tag("or", &[]),
            ExprKind::Cast(_) => self.tag("cast", &[]),
            ExprKind::Proven(_) => self.tag("proven", &[]),
            ExprKind::Math(op, xs) => {
                self.tag("math", &[len(xs)]);
                self.debug(op);
            }
            ExprKind::Hash(op, xs) => {
                self.tag("hash", &[len(xs)]);
                self.debug(op);
            }
            ExprKind::RandFloat { ranged } => self.tag("rand float", &[u64::from(*ranged)]),
            ExprKind::RandInt(..) => self.tag("rand int", &[]),
            ExprKind::Call(g, args) => {
                self.tag("call", &[]);
                self.func(*g);
                self.args(args);
            }
            ExprKind::FnRef(g) => {
                self.tag("fn ref", &[]);
                self.func(*g);
            }
            ExprKind::MakeFn { code, env } => {
                self.tag("make fn", &[]);
                self.func(*code);
                match env {
                    EnvKind::None => self.tag("no env", &[]),
                    EnvKind::Heap(env) => self.env("heap env", env),
                    EnvKind::Stack(env) => self.env("stack env", env),
                }
            }
            ExprKind::FnEnv(_) => self.tag("fn env", &[]),
            ExprKind::EnvDup(_) => self.tag("env dup", &[]),
            ExprKind::BlockDup { size, align, .. } => self.tag("block dup", &[u64::from(*size), u64::from(*align)]),
            ExprKind::CallIndirect { args, sig, .. } => {
                self.tag("call indirect", &[]);
                self.sig(sig);
                self.args(args);
            }
            // A method table by the functions in it.
            ExprKind::MakeIface { table, data } => {
                let table = &m.tables[table.0 as usize];
                self.tag("make iface", &[u64::from(matches!(data, IfaceData::Boxed(_))), len(&table.methods)]);
                for &g in [table.copy, table.print].iter().chain(&table.methods) {
                    self.func(g);
                }
            }
            ExprKind::CallVirtual { slot, args, sig, keeps, method, .. } => self.virtual_call(*slot, sig, *keeps, *method, args),
            ExprKind::StructLit(s, xs) => {
                self.tag("struct lit", &[len(xs)]);
                self.str(&m.structs[s.0 as usize].name);
            }
            ExprKind::VariantLit(e, v, xs) => {
                self.tag("variant lit", &[u64::from(*v), len(xs)]);
                self.str(&m.enums[e.0 as usize].name);
            }
            ExprKind::Tag(_) => self.tag("tag", &[]),
            ExprKind::FieldOf(_, i) => self.tag("field of", &[u64::from(*i)]),
            ExprKind::VecLit(xs) => self.tag("vec lit", &[len(xs)]),
            ExprKind::Splat(_) => self.tag("splat", &[]),
            ExprKind::Shuffle(_, lanes) => self.tag("shuffle", &lanes.map(u64::from)),
            ExprKind::LaneOf(_, l) => self.tag("lane of", &[u64::from(*l)]),
            ExprKind::Seq(stmts, _) => self.tag("seq", &[len(stmts)]),
            ExprKind::If { then_body, then_val, else_body, else_val, .. } => {
                let has = |v: &Option<Box<Expr>>| u64::from(v.is_some());
                self.tag("if", &[len(then_body), has(then_val), len(else_body), has(else_val)]);
            }
        }
    }
}
