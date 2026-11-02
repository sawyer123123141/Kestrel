//! The collection pass (R2, R6, G13): after every body is checked, with no errors, one `FnInfo` per
//! copy (a generic function or method and a set of types), with every type its lowering reads
//! interned, so lowering only looks types up. The tables are frozen after it.
//!
//! It walks from every root (each non-generic, non-abstract function body, constant initializer
//! and non-generic struct's field default) and from each copy it makes, under the copy's
//! substitution, and from each missing field default of an instance it builds, under the
//! instance's. A request with no copy yet appends a `FnInfo` and queues its body. Copies are
//! never checked again: they share their generic's locals, store summaries and checks (G24, G25).
//!
//! The walk stops at the caps (G10, P9, P25, P41): a type argument nested more than 32 levels in
//! a copy or in any instance the walk interns, a type list that would grow past 1,000 parts
//! (checked before it is interned, so no oversized name is built), or more than 10,000 copies of
//! one generic.

use std::collections::{HashMap, HashSet, VecDeque};

use kestrel_syntax::ast::{Block, Expr, ExprId, Item, Program};
use kestrel_syntax::visit::{each_expr, each_expr_in};
use kestrel_syntax::{Diagnostic, Span};

use crate::builtins::Builtin;
use crate::generic::copies_checksum::{checksum_arg, ChecksumArg};
use crate::generic::{expand, CopyInfo, GenericOwner, Subst, TyListId};
use crate::tables::{EnumId, FnId, FnInfo, ParamSig, Res, StructId, Tables};
use crate::ty::Ty;

/// The most levels a type argument may nest (`int` is 0, `list[int]` 1).
const MAX_LEVELS: u32 = 32;
/// The most copies of one generic (and default walks of one generic struct's instances).
const MAX_COPIES: u32 = 10_000;
const WIDE: &str = "a type grew to 1,000 parts";

/// Runs the static function expansion check, then, when it reports nothing, the walk. The check
/// calls this only when there are no errors, so every instance and `type_args` entry is sound.
pub(crate) fn collect_copies(program: &Program, t: &mut Tables, diags: &mut Vec<Diagnostic>) {
    // No generic is declared: nothing to copy (the editor runs this on every clean edit, P30).
    if t.generic.params.is_empty() {
        return;
    }
    if expand::check_fn_expansion(program, t, diags) {
        return;
    }
    let mut walk = Walk { program, t, units: Vec::new(), queue: VecDeque::new(), copies: HashMap::new(), defaults: HashSet::new(), walked: HashMap::new(), nest: HashMap::new() };
    if let Err(d) = walk.run() {
        diags.push(d);
    }
}

/// Where a body is walked: its substitution (`None` in plain code, whose types are all
/// concrete), the copy whose walk led here, the plain-code expression that started it (G21), and
/// the unit being walked (`None` in plain code).
#[derive(Clone, Copy)]
pub(super) struct Ctx {
    pub(super) subst: Option<Subst>,
    pub(super) made_by: Option<FnId>,
    pub(super) made_at: Option<Span>,
    pub(super) unit: Option<usize>,
}

const PLAIN: Ctx = Ctx { subst: None, made_by: None, made_at: None, unit: None };

/// A body queued for the walk.
#[derive(Clone, Copy)]
pub(super) enum Body {
    Copy(FnId),
    /// The default of an instance's field (by index), which a construction left out.
    Default(StructId, usize),
}

/// A queued body, how to walk it, and the unit whose walk queued it (for the caps' chains).
pub(super) struct Unit {
    pub(super) body: Body,
    pub(super) ctx: Ctx,
    pub(super) parent: Option<usize>,
}

pub(super) struct Walk<'a> {
    pub(super) program: &'a Program,
    pub(super) t: &'a mut Tables,
    pub(super) units: Vec<Unit>,
    queue: VecDeque<usize>,
    /// Copies made per generic, for the cap.
    copies: HashMap<FnId, u32>,
    /// The instance fields whose defaults are queued, each once.
    defaults: HashSet<(StructId, usize)>,
    /// Instances whose defaults were walked, per declaration, for the cap.
    walked: HashMap<StructId, HashSet<StructId>>,
    /// How many levels each type nests, as worked out so far.
    pub(super) nest: HashMap<Ty, u32>,
}

impl<'a> Walk<'a> {
    /// Walks every root in turn, each followed by everything it leads to (so a copy's `made_at`
    /// is the first place in plain code that led to it, G21).
    fn run(&mut self) -> Result<(), Diagnostic> {
        let program = self.program;
        for f in 0..self.t.fns.len() {
            let info = self.t.func(FnId(f as u32));
            if info.generic.is_some() || info.is_abstract || info.is_native {
                continue;
            }
            self.block(&info.decl(program).body, PLAIN)?;
            self.drain()?;
        }
        for item in &program.items {
            match item {
                Item::Const(c) => {
                    self.expr_tree(&c.value, PLAIN)?;
                    self.drain()?;
                }
                Item::Struct(s) if s.type_params.is_empty() => {
                    for d in s.fields.iter().filter_map(|f| f.default.as_ref()) {
                        self.expr_tree(d, PLAIN)?;
                        self.drain()?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn drain(&mut self) -> Result<(), Diagnostic> {
        let program = self.program;
        while let Some(u) = self.queue.pop_front() {
            let (body, ctx) = (self.units[u].body, self.units[u].ctx);
            match body {
                Body::Copy(f) => {
                    let info = self.t.func(f);
                    let generic = info.copy.expect("a copy").generic;
                    for l in info.locals.clone() {
                        self.sub(self.t.local(l).ty, ctx)?;
                    }
                    self.block(&self.t.func(generic).decl(program).body, ctx)?;
                }
                Body::Default(inst, field) => {
                    let def = self.t.strukt(inst).instance.expect("an instance").def;
                    let info = self.t.strukt(def);
                    let Item::Struct(decl) = &program.items[info.item] else { continue };
                    let name = &info.fields[field].name;
                    let default = decl.fields.iter().find(|f| &f.name == name).and_then(|f| f.default.as_ref());
                    if let Some(d) = default {
                        self.expr_tree(d, ctx)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn block(&mut self, block: &Block, ctx: Ctx) -> Result<(), Diagnostic> {
        let mut exprs = Vec::new();
        each_expr(block, &mut |e: &Expr| exprs.push((e.id, e.span, checksum_arg(e))));
        exprs.into_iter().try_for_each(|(id, span, arg)| self.expr(id, span, arg, ctx))
    }

    fn expr_tree(&mut self, e: &Expr, ctx: Ctx) -> Result<(), Diagnostic> {
        let mut exprs = Vec::new();
        each_expr_in(e, &mut |e: &Expr| exprs.push((e.id, e.span, checksum_arg(e))));
        exprs.into_iter().try_for_each(|(id, span, arg)| self.expr(id, span, arg, ctx))
    }

    /// One expression: its types interned under `ctx`, and the copies and default walks it needs.
    /// `checksum` is the argument of a `checksum(x)` call, which each copy judges.
    fn expr(&mut self, id: ExprId, span: Span, checksum: ChecksumArg, ctx: Ctx) -> Result<(), Diagnostic> {
        let at = ctx.made_at.unwrap_or(span);
        let ty = self.sub(self.t.ty(id), ctx)?;
        let type_args = match self.t.generic.type_args.get(&id) {
            Some(&l) => Some(self.sub_list(l, ctx)?),
            None => None,
        };
        if let Some(&target) = self.t.coerce.get(&id) {
            let target = self.sub(target, ctx)?;
            self.convert(ty, target, ctx, at)?;
        }
        let closure = match ctx.subst {
            Some(_) => self.t.closures.get(&id).map(|c| (c.sig, c.locals.clone(), c.captures.iter().map(|c| c.ty).collect::<Vec<Ty>>())),
            None => None,
        };
        if let Some((sig, locals, captures)) = closure {
            self.sub(Ty::Fn(sig), ctx)?;
            for ty in captures {
                self.sub(ty, ctx)?;
            }
            for l in locals {
                self.sub(self.t.local(l).ty, ctx)?;
            }
        }
        match self.t.res.get(&id).copied() {
            Some(Res::Fn(f) | Res::FnValue(f)) => {
                let info = self.t.func(f);
                if let (Some(_), None, Some(args)) = (info.generic, info.copy, type_args) {
                    self.request(f, args, ctx, at)?;
                }
            }
            Some(Res::ParamCall { param, iface, slot }) => {
                let concrete = self.sub(Ty::Param(param), ctx)?;
                let method = self.t.iface(iface).methods[slot as usize];
                let name = self.t.func(method).name.rsplit('.').next().unwrap_or_default().to_string();
                self.method_of(concrete, &name, ctx, at)?;
            }
            Some(Res::Construct(sid)) => {
                let Ty::Struct(inst) = self.sub(Ty::Struct(sid), ctx)? else { return Ok(()) };
                if self.t.strukt(inst).instance.is_some() {
                    let missing: Vec<usize> =
                        self.t.ctor_args.get(&id).map(|a| a.iter().enumerate().filter(|(_, a)| a.is_none()).map(|(i, _)| i).collect()).unwrap_or_default();
                    for field in missing {
                        self.queue_default(inst, field, ctx, at)?;
                    }
                }
            }
            Some(Res::Variant(eid, _)) => {
                self.sub(Ty::Enum(eid), ctx)?;
            }
            Some(Res::CallValue(sig)) => {
                self.sub(Ty::Fn(sig), ctx)?;
            }
            Some(Res::Convert(ty) | Res::SimdMake(ty)) => {
                self.sub(ty, ctx)?;
            }
            Some(Res::Builtin(b @ (Builtin::Checksum | Builtin::ToBytes | Builtin::FirstDifference))) => {
                if let Some((arg, root)) = checksum {
                    self.checksum_copy(b, arg, root, span, ctx)?;
                }
            }
            Some(Res::Builtin(Builtin::FromBytes)) => self.decode_copy(id, ty, span, ctx)?,
            _ => {}
        }
        Ok(())
    }

    /// A conversion of a `src` value to `target`: an instance converted to an interface (or its
    /// optional) needs a copy of every method the interface's table holds.
    fn convert(&mut self, src: Ty, target: Ty, ctx: Ctx, at: Span) -> Result<(), Diagnostic> {
        let unwrap = |t: &Tables, ty: Ty| match ty {
            Ty::Optional(e) => t.elem(e),
            other => other,
        };
        // An `Ok` wrap converts to the `Result`'s `T` first.
        let target = self.t.result_parts(target).map_or(target, |(value, _)| value);
        let Ty::Iface(i) = unwrap(self.t, target) else { return Ok(()) };
        let src = unwrap(self.t, src);
        let names: Vec<String> = self.t.iface(i).methods.iter().map(|&m| self.t.func(m).name.rsplit('.').next().unwrap_or_default().to_string()).collect();
        names.iter().try_for_each(|name| self.method_of(src, name, ctx, at))
    }

    /// The method `name` of a concrete type: a copy when the type is an instance. A plain type's
    /// method is a root, and an interface type's calls go through its table.
    fn method_of(&mut self, ty: Ty, name: &str, ctx: Ctx, at: Span) -> Result<(), Diagnostic> {
        let (method, args) = match ty {
            Ty::Struct(s) => (self.t.strukt(s).methods.get(name).copied(), self.t.strukt(s).instance.map(|i| i.args)),
            Ty::Enum(e) | Ty::Variant(e, _) => (self.t.enm(e).methods.get(name).copied(), self.t.enm(e).instance.map(|i| i.args)),
            _ => return Ok(()),
        };
        match (method, args) {
            (Some(m), Some(args)) => self.request(m, args, ctx, at),
            _ => Ok(()),
        }
    }

    /// The copy of generic `f` for `args`, made and queued when it doesn't exist yet.
    fn request(&mut self, f: FnId, args: TyListId, ctx: Ctx, at: Span) -> Result<(), Diagnostic> {
        if self.t.generic.fn_copies.contains_key(&(f, args)) {
            return Ok(());
        }
        let tys = self.t.ty_list(args).to_vec();
        if !tys.iter().all(|&a| self.t.is_concrete(a)) {
            debug_assert!(false, "a copy of `{}` was requested with placeholders; this is a compiler bug", self.t.func(f).name);
            return Ok(());
        }
        let levels = tys.iter().map(|&a| self.nest(a)).max().unwrap_or(0);
        if levels > MAX_LEVELS {
            return Err(self.copy_cap(f, args, ctx, "32 levels"));
        }
        let made = self.copies.get(&f).copied().unwrap_or(0);
        if made == MAX_COPIES {
            return Err(self.copy_cap(f, args, ctx, "10,000 copies"));
        }
        self.copies.insert(f, made + 1);
        let info = self.t.func(f).clone();
        let s = Subst { owner: info.generic.expect("a generic"), args };
        // A signature too big for the cap is this copy's: blame its chain, naming the copy.
        if self.size_under(Ty::Fn(info.sig), s).is_err() {
            return Err(self.copy_cap(f, args, ctx, WIDE));
        }
        // Instances the signature makes are the requester's (for the cap's chain).
        let here = Ctx { subst: Some(s), ..ctx };
        let mut params = Vec::with_capacity(info.params.len());
        for p in &info.params {
            params.push(ParamSig { name: p.name.clone(), ty: self.sub(p.ty, here)?, is_var: p.is_var });
        }
        let ret = self.sub(info.ret, here)?;
        let Ty::Fn(sig) = self.sub(Ty::Fn(info.sig), here)? else { unreachable!("a signature substitutes to a signature") };
        let name = self.copy_name(f, args);
        let id = FnId(self.t.fns.len() as u32);
        let copy = CopyInfo { generic: f, args, made_at: at, made_by: ctx.made_by };
        self.t.fns.push(FnInfo { name, params, ret, sig, type_params: Vec::new(), copy: Some(copy), ..info });
        self.t.generic.fn_copies.insert((f, args), id);
        let unit = self.units.len();
        let walk = Ctx { subst: Some(s), made_by: Some(id), made_at: Some(at), unit: Some(unit) };
        self.units.push(Unit { body: Body::Copy(id), ctx: walk, parent: ctx.unit });
        self.queue.push_back(unit);
        Ok(())
    }


    /// Queues the default of instance `inst`'s field `field`, once.
    fn queue_default(&mut self, inst: StructId, field: usize, ctx: Ctx, at: Span) -> Result<(), Diagnostic> {
        if !self.defaults.insert((inst, field)) {
            return Ok(());
        }
        let i = self.t.strukt(inst).instance.expect("an instance");
        let walked = self.walked.entry(i.def).or_default();
        let full = !walked.contains(&inst) && walked.len() as u32 == MAX_COPIES;
        if full {
            return Err(self.walk_cap(ctx, (GenericOwner::Struct(i.def), Some(Ty::Struct(inst))), "10,000 copies"));
        }
        walked.insert(inst);
        let unit = self.units.len();
        let walk = Ctx { subst: Some(Subst { owner: GenericOwner::Struct(i.def), args: i.args }), made_by: ctx.made_by, made_at: Some(at), unit: Some(unit) };
        self.units.push(Unit { body: Body::Default(inst, field), ctx: walk, parent: ctx.unit });
        self.queue.push_back(unit);
        Ok(())
    }

    /// `ty` under `ctx`'s substitution, interned. A type that would grow past `MAX_PARTS` stops the
    /// walk before anything is interned or named (P41); an instance it interns whose type
    /// arguments nest past the cap stops it after (P25).
    pub(super) fn sub(&mut self, ty: Ty, ctx: Ctx) -> Result<Ty, Diagnostic> {
        let Some(s) = ctx.subst else { return Ok(ty) };
        if let Err(wide) = self.size_under(ty, s) {
            return Err(self.walk_cap(ctx, (wide, None), WIDE));
        }
        let before = (self.t.structs.len(), self.t.enums.len());
        let out = self.t.subst(ty, s);
        self.check_new(before, ctx)?;
        Ok(out)
    }

    fn sub_list(&mut self, l: TyListId, ctx: Ctx) -> Result<TyListId, Diagnostic> {
        let Some(s) = ctx.subst else { return Ok(l) };
        if let Err(wide) = self.list_size_under(l, s) {
            return Err(self.walk_cap(ctx, (wide, None), WIDE));
        }
        let before = (self.t.structs.len(), self.t.enums.len());
        let out = self.t.subst_list(l, s);
        self.check_new(before, ctx)?;
        Ok(out)
    }

    /// The instances interned since `before` (struct and enum counts), each within the cap.
    fn check_new(&mut self, (structs, enums): (usize, usize), ctx: Ctx) -> Result<(), Diagnostic> {
        let new_structs = (structs..self.t.structs.len()).filter_map(|id| {
            let i = self.t.structs[id].instance?;
            Some((GenericOwner::Struct(i.def), i.args, Ty::Struct(StructId(id as u32))))
        });
        let new_enums = (enums..self.t.enums.len()).filter_map(|id| {
            let i = self.t.enums[id].instance?;
            Some((GenericOwner::Enum(i.def), i.args, Ty::Enum(EnumId(id as u32))))
        });
        let new: Vec<(GenericOwner, TyListId, Ty)> = new_structs.chain(new_enums).collect();
        for (def, args, ty) in new {
            if self.list_nest(args) > MAX_LEVELS {
                return Err(self.walk_cap(ctx, (def, Some(ty)), "32 levels"));
            }
        }
        Ok(())
    }
}
