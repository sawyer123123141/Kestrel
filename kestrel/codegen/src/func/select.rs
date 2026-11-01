//! Small `if`s as straight-line code, and the exact shift after a passed even test (spec decision
//! 2 of the performance round).
//!
//! An `if` whose sides only give register locals values made of register locals and constants,
//! with nothing that can fail, call, or touch memory, is lowered with no jump: both sides are
//! computed in written order, then each local the sides assign takes the value of the side the
//! condition picks (an integer or bool `select`, a `cmov`; a float through `wide_pick`). A branch
//! on data the predictor can't learn (collatz's parity) costs far more than the few instructions
//! of the side not taken. Nothing computed on that side can be seen: it only reaches the locals
//! through the pick, and it can't trap, print or write memory.
//!
//! Separately, a value tested by `n % 2^k == 0` is a multiple of `2^k` on the side where the test
//! passed, so `n / 2^k` there is one arithmetic shift, with no rounding fix for negative `n`. The
//! fact is held by the tested Cranelift value, so it ends where `n` is assigned. On a select the
//! shift also runs when the test failed, but its result is then never picked.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{Block, InstBuilder, Value};
use cranelift_frontend::Variable;
use cranelift_module::Module;
use kestrel_kir::{self as kir, BinOp, ExprKind, KTy, LocalIdx, MathOp, Stmt, UnOp};

use super::arith::float_cc;
use super::divide::{pow2_rem_test, wrap_literal};
use super::{Check, FuncLowerer, Slot};

/// At most this many operations on each side of a selected `if`.
const MAX_OPS: usize = 3;
/// At most this many register locals assigned by its two sides together.
const MAX_LOCALS: usize = 2;
/// At most this many operations in a float value a selected `if` picks. A float pick is a mask
/// and a blend in the vector registers, and a float `if` is most often a bound or a bounce that is
/// rarely taken and well predicted: swarm's bounce (`y = -y`, `vy = -vy * 0.9`) measured 1.08 ns
/// a particle as a branch and 1.49 as selects. With 0, only picks of locals and constants (a
/// clamp, a min or max, which Cranelift makes `minss`/`maxss`) become selects.
const MAX_FLOAT_OPS: usize = 0;

/// One side of an `if`: its statements and, for a value `if`, its value.
pub(super) type Side<'e> = (&'e [Stmt], Option<&'e kir::Expr>);

/// A lowered `if` condition.
pub(super) struct Test {
    /// The condition as a bool.
    pub(super) flag: Value,
    /// A float compare `x cc y` (kept for a select): its mask picks floats in the float registers.
    float: Option<(FloatCC, Value, Value)>,
    /// For `n % 2^k == 0` (true) or `!= 0` (false): `n`'s value and `k`.
    exact: Option<(Value, u32, bool)>,
}

impl Test {
    /// What the then side (`then`) or the else side knows: a value that is a multiple of `2^k`.
    fn exact_on(&self, then: bool) -> Option<(Value, u32)> {
        self.exact.filter(|e| e.2 == then).map(|(v, k, _)| (v, k))
    }
}

/// Whether converting `from` to `to` narrows an integer (checked in a checked build).
fn narrows(from: KTy, to: KTy) -> bool {
    let bits = |t: KTy| match t {
        KTy::I64 => 64,
        KTy::I32 => 32,
        _ => 8,
    };
    from.is_int() && to.is_int() && bits(to) < bits(from)
}

fn scalar(t: KTy) -> bool {
    t.is_int() || t.is_float() || t == KTy::Bool
}

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// Lowers an `if` condition. `floats` keeps a float compare's operands, for a select.
    pub(super) fn test(&mut self, cond: &kir::Expr, floats: bool) -> Result<Test, String> {
        if let Some((n, k, is_eq)) = pow2_rem_test(cond) {
            let v = self.expr(n)?;
            let flag = self.mask_test(v, k, is_eq);
            return Ok(Test { flag, float: None, exact: Some((v, k, is_eq)) });
        }
        if let ExprKind::Binary(op, l, r) = &cond.kind {
            if floats && op.is_comparison() && l.ty.is_float() {
                let (x, y) = (self.expr(l)?, self.expr(r)?);
                let cc = float_cc(*op);
                let flag = self.b.ins().fcmp(cc, x, y);
                return Ok(Test { flag, float: Some((cc, x, y)), exact: None });
            }
        }
        Ok(Test { flag: self.expr(cond)?, float: None, exact: None })
    }

    /// `v % 2^k == 0` (`is_eq`) or `!= 0`, as `v & (2^k - 1)` against zero.
    pub(super) fn mask_test(&mut self, v: Value, k: u32, is_eq: bool) -> Value {
        let low = self.b.ins().band_imm_s(v, (1i64 << k) - 1);
        let cc = if is_eq { IntCC::Equal } else { IntCC::NotEqual };
        self.b.ins().icmp_imm_s(cc, low, 0)
    }

    /// Lowers one side of a branching `if` in `block` (whose only predecessor is the branch), with
    /// what the test tells that side: its statements, then its value `val` if control reaches it.
    /// Returns `None` when control doesn't reach the end, else the side's value, if it has one.
    pub(super) fn branch_side(
        &mut self,
        t: &Test,
        then: bool,
        block: Block,
        (body, val): Side,
    ) -> Result<Option<Option<Value>>, String> {
        self.b.switch_to_block(block);
        let fact = t.exact_on(then);
        if fact.is_some() {
            // Sealed now, so a local's value read here is the very value tested before the branch.
            self.b.seal_block(block);
        }
        self.exact.extend(fact);
        let end = if self.stmts(body)? { Some(val.map(|v| self.expr(v)).transpose()?) } else { None };
        if fact.is_some() {
            self.exact.pop();
        }
        Ok(end)
    }

    /// The register locals the two sides of an `if` assign, when the `if` can be straight-line
    /// code: every statement gives a register local a value; no side has a check left, a call, a
    /// load or store of memory, a division but by a constant, a nested `if`, more than `MAX_OPS`
    /// operations, or a float value of more than `MAX_FLOAT_OPS`; and at most `MAX_LOCALS` locals
    /// are assigned.
    pub(super) fn selectable(&self, sides: [Side; 2]) -> Option<Vec<LocalIdx>> {
        let mut locals = Vec::new();
        for (body, val) in sides {
            let mut ops = 0;
            for s in body {
                let (l, e) = match s {
                    Stmt::Assign(p, e) if p.path.is_empty() => (p.root, e),
                    Stmt::Let(l, e) => (*l, e),
                    _ => return None,
                };
                if !self.in_register(l) || !self.pure_value(e, &mut ops) {
                    return None;
                }
                if !locals.contains(&l) {
                    locals.push(l);
                }
            }
            if val.is_some_and(|v| !self.pure_value(v, &mut ops)) || ops > MAX_OPS {
                return None;
            }
        }
        (locals.len() <= MAX_LOCALS).then_some(locals)
    }

    /// Whether `e` can be a value a selected `if` picks: `pure`, and within `MAX_FLOAT_OPS`
    /// operations when it is a float. Counts its operations into `ops`.
    fn pure_value(&self, e: &kir::Expr, ops: &mut usize) -> bool {
        let mut here = 0;
        let ok = self.pure(e, Check::Emit, &mut here) && !(e.ty.is_float() && here > MAX_FLOAT_OPS);
        *ops += here;
        ok
    }

    /// Whether local `l` is a number or bool held in a Cranelift variable.
    fn in_register(&self, l: LocalIdx) -> bool {
        matches!(self.slots[l.0 as usize], Slot::Var(_)) && scalar(self.f.locals[l.0 as usize].ty)
    }

    /// Whether `e` can be computed when its side isn't taken: it reads only register locals and
    /// constants, and nothing in it can fail, call or touch memory. Counts its operations into
    /// `ops`. `check` is how `e`'s own integer checks are emitted (`Skip` inside `Proven`).
    fn pure(&self, e: &kir::Expr, check: Check, ops: &mut usize) -> bool {
        if !scalar(e.ty) {
            return false;
        }
        let checked = check.arith(self.shared.opts.checked_arith);
        let ok = match &e.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => return true,
            ExprKind::Load(p) => return p.path.is_empty() && self.in_register(p.root),
            ExprKind::Proven(inner) => {
                return self.proven_check() == Check::Skip && self.pure(inner, Check::Skip, ops);
            }
            ExprKind::Unary(UnOp::Neg, x) => (e.ty.is_float() || !checked) && self.pure(x, Check::Emit, ops),
            ExprKind::Unary(_, x) => self.pure(x, Check::Emit, ops),
            ExprKind::Cast(x) => !(checked && narrows(x.ty, e.ty)) && self.pure(x, Check::Emit, ops),
            ExprKind::Binary(op, l, r) => {
                let ok = match op {
                    _ if op.is_comparison() => scalar(l.ty),
                    // A float `%` is a call or a long inline sequence.
                    _ if e.ty.is_float() => *op != BinOp::Rem,
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Shl | BinOp::Shr => !checked,
                    // A division by a variable can trap in hardware, even when proven safe on its side.
                    BinOp::Div | BinOp::Rem => match r.kind {
                        ExprKind::Int(c) => {
                            let c = wrap_literal(e.ty, c);
                            c != 0 && !(e.ty.is_signed() && c == -1)
                        }
                        _ => false,
                    },
                    _ => true,
                };
                ok && self.pure(l, Check::Emit, ops) && self.pure(r, Check::Emit, ops)
            }
            ExprKind::Math(op, args) => {
                e.ty.is_float()
                    && matches!(op, MathOp::Abs | MathOp::Min | MathOp::Max | MathOp::Sqrt)
                    && args.iter().all(|a| self.pure(a, Check::Emit, ops))
            }
            _ => false,
        };
        *ops += 1;
        ok
    }

    /// Lowers a selectable `if` (see `selectable`): both sides in written order, each from the
    /// locals' values before the `if`, then each local in `locals` gets the value of the side the
    /// test picks. Returns the picked value of a value `if`.
    pub(super) fn select_sides(&mut self, t: &Test, sides: [Side; 2], locals: &[LocalIdx]) -> Result<Option<Value>, String> {
        let vars: Vec<Variable> = locals
            .iter()
            .map(|l| match self.slots[l.0 as usize] {
                Slot::Var(v) => v,
                _ => unreachable!("selectable locals live in variables"),
            })
            .collect();
        let before: Vec<Value> = vars.iter().map(|&v| self.b.use_var(v)).collect();
        let mut got = Vec::with_capacity(2);
        for (i, (body, val)) in sides.into_iter().enumerate() {
            for (&v, &x) in vars.iter().zip(&before) {
                self.b.def_var(v, x);
            }
            let fact = t.exact_on(i == 0);
            self.exact.extend(fact);
            self.stmts(body)?;
            let value = val.map(|e| self.expr(e)).transpose()?;
            if fact.is_some() {
                self.exact.pop();
            }
            let after: Vec<Value> = vars.iter().map(|&v| self.b.use_var(v)).collect();
            got.push((after, value));
        }
        let mut mask = None;
        for (i, &v) in vars.iter().enumerate() {
            let picked = self.pick_by(t, &mut mask, got[0].0[i], got[1].0[i]);
            self.b.def_var(v, picked);
        }
        Ok(match (got[0].1, got[1].1) {
            (Some(a), Some(b)) => Some(self.pick_by(t, &mut mask, a, b)),
            _ => None,
        })
    }

    /// `a` where the test holds, else `b`: an integer `select`, or for floats the compare's mask
    /// (made once, in `mask`) when the test is a float compare at least as wide as the values.
    fn pick_by(&mut self, t: &Test, mask: &mut Option<Value>, a: Value, b: Value) -> Value {
        if a == b {
            return a;
        }
        let ty = self.b.func.dfg.value_type(a);
        if !ty.is_float() {
            return self.b.ins().select(t.flag, a, b);
        }
        if let Some((cc, x, y)) = t.float {
            if self.b.func.dfg.value_type(x).bits() >= ty.bits() {
                let m = match *mask {
                    Some(m) => m,
                    None => *mask.insert(self.fmask(cc, x, y)),
                };
                return self.pick(m, a, b);
            }
        }
        self.pick(t.flag, a, b)
    }
}
