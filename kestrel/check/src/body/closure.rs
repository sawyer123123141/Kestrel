//! Closure literals. A literal's body is checked inside the enclosing `BodyChecker` with a
//! `ClosureFrame`, the way a `par for` body is: the frame owns the body's locals, starts its own
//! region contexts and loop state, and collects its purity.
//!
//! **What the frame resets.** Its allocation contexts (`ctxs` starts again at `[Function]`: the
//! body runs in whatever arena is current when it is called), its move state (restored when the
//! frame ends: building a closure moves nothing), and its loop state, by a floor: the loops
//! outside the literal stay in `loop_breaks`, `loop_moves` and `removing.loops` so that depths
//! stay comparable with the `par` stack's, but `break`, `continue` and `remove` can't reach
//! below the floor. **What it keeps:** the `par` stack and the enclosing function's purity
//! (`is_pure`), for the rules that a closure in a `pure` function or a `par for` body must be pure.
//!
//! **Locals.** A closure's parameters and variables belong to the closure (`LocalInfo::closure`,
//! `ClosureInfo::locals`), and `LocalInfo::owner` is the function the literal is written in, if
//! any. So a field default or a constant, which have no function, can hold closures.
//!
//! **Purity.** Inside a frame, `print`, random numbers and calls that aren't pure make the
//! closure impure instead of failing where they are. They fail only when the code around
//! requires purity: a `pure` function or a `par for` body (at the use), or an expected `pure`
//! function type (at the end of the literal). They never touch the enclosing function's `io`
//! or `mutates_params`.
//!
//! **Captures.** A body's reads of locals and parameters from outside it are collected in the
//! frame and become the closure's captures when it ends (see `captures`).

use std::collections::HashMap;

use kestrel_syntax::ast::{ClosureLit, Expr, ExprId, Stmt};
use kestrel_syntax::Span;

use super::captures::Reads;
use super::closure_result::Infer;
use super::moves::Moved;
use super::regions::Ctx;
use super::{BodyChecker, Expect};
use crate::closure::ClosureInfo;
use crate::sig::{Sig, SigParam};
use crate::tables::LocalId;
use crate::ty::Ty;

/// A closure literal being checked.
pub(crate) struct ClosureFrame {
    pub(crate) id: ExprId,
    /// Locals from this id on belong to the closure; earlier ones are outside it.
    pub(crate) first_local: u32,
    /// The closure's locals, parameters first.
    pub(crate) locals: Vec<LocalId>,
    /// How many loops, and removing loops, were open around the literal: `break`, `continue`
    /// and `remove` can't leave through them.
    loop_floor: usize,
    removing_floor: usize,
    /// How many `par for` bodies were open around the literal: `return` is refused only in the
    /// ones the closure opens itself.
    par_len: usize,
    /// Set while the result type is being worked out (no `-> T`, and no function type expected).
    pub(super) infer: Option<Infer>,
    /// Why the closure isn't pure, once something makes it so.
    impure: Option<String>,
    mutates_params: bool,
    /// The places from outside the body that it reads (see `captures`).
    pub(super) reads: Reads,
    /// The state of the code around, put back when the frame ends.
    saved: Saved,
}

struct Saved {
    ctxs: Vec<Ctx>,
    moved: Moved,
    discarded: Option<ExprId>,
    ret: Ty,
}

/// Something a `pure` function or a `par for` body can't do.
pub(crate) enum Impure<'a> {
    Print,
    Random,
    Exit,
    /// A call of the function with this name, which isn't `pure`.
    Call(&'a str),
    /// A call of a function value whose type isn't `pure`, as a message names it.
    Value(&'a str),
}

impl Impure<'_> {
    /// What the closure does, as "it ..." completes.
    fn does(&self) -> String {
        match self {
            Impure::Print => "calls `print`".to_string(),
            Impure::Random => "uses random numbers".to_string(),
            Impure::Exit => "calls `exit`".to_string(),
            Impure::Call(name) => format!("calls `{name}`, which isn't pure"),
            Impure::Value(shown) => format!("calls {shown}, which isn't a pure function type"),
        }
    }

    /// The error where the code around requires purity (`ctx` is what it is).
    fn error(&self, ctx: &str) -> String {
        match self {
            Impure::Print => format!("this closure calls `print`, which a {ctx} can't do"),
            Impure::Random => format!("this closure uses random numbers, which a {ctx} can't do"),
            Impure::Exit => format!("this closure calls `exit`, which a {ctx} can't do"),
            Impure::Call(name) => format!("this closure calls `{name}`, which isn't `pure`; a {ctx} can only call `pure` functions"),
            Impure::Value(shown) => format!("this closure calls {shown}, which isn't a pure function type; a {ctx} can only call `pure` functions"),
        }
    }
}

fn parameters(n: usize) -> String {
    match n {
        0 => "no parameters".to_string(),
        1 => "1 parameter".to_string(),
        n => format!("{n} parameters"),
    }
}

impl<'a> BodyChecker<'a> {
    /// How many loops are open around the innermost closure literal (0 outside any).
    pub(crate) fn loop_floor(&self) -> usize {
        self.frames.last().map_or(0, |f| f.loop_floor)
    }

    /// How many removing loops are open around the innermost closure literal.
    pub(crate) fn removing_floor(&self) -> usize {
        self.frames.last().map_or(0, |f| f.removing_floor)
    }

    /// Whether a `par for` body that a `return` would leave is open: not one around the literal.
    pub(crate) fn par_inside_frame(&self) -> bool {
        self.par.len() > self.frames.last().map_or(0, |f| f.par_len)
    }

    /// Whether the body being checked is a closure's.
    pub(crate) fn in_closure(&self) -> bool {
        !self.frames.is_empty()
    }

    /// An impure act at `span`. Inside a closure it is the closure's impurity, an error only where
    /// the code around requires purity. Returns whether it was handled so, in which case the caller
    /// has nothing else to report and doesn't mark the enclosing function.
    pub(crate) fn closure_impure(&mut self, span: Span, what: Impure) -> bool {
        if self.frames.is_empty() {
            return false;
        }
        if self.is_pure {
            self.error(span, what.error("`pure` function"));
        } else if !self.par.is_empty() {
            self.error(span, what.error("`par for` body"));
        } else if let Some(frame) = self.frames.last_mut() {
            frame.impure.get_or_insert_with(|| what.does());
        }
        true
    }

    /// A parameter of the closure that the body changes in place.
    pub(crate) fn closure_mutates(&mut self, local: LocalId) -> bool {
        let Some(id) = self.t.local(local).closure else { return false };
        if let Some(frame) = self.frames.iter_mut().find(|f| f.id == id) {
            frame.mutates_params = true;
        }
        true
    }

    /// The message for `break` or `continue` that would leave a closure, if it would.
    pub(crate) fn closure_jump_error(&self, word: &str) -> Option<String> {
        let floor = self.loop_floor();
        (floor > 0 && self.loop_breaks.len() <= floor).then(|| format!("`{word}` can't leave this closure; it is a function of its own"))
    }

    /// A closure literal: `(x: int) -> int { x + 1 }`. `want` is the type the code around expects,
    /// which gives parameter types and the result type where the literal leaves them out.
    pub(crate) fn check_closure(&mut self, e: &Expr, lit: &ClosureLit, want: Option<Ty>) -> Ty {
        let arg_name = self.arg_name.take();
        let infer_result = std::mem::take(&mut self.infer_result);
        let want_sig = match want {
            Some(Ty::Fn(s)) => Some(self.t.sig(s).clone()),
            _ => None,
        };
        // A function type of another shape can't give anything; its mismatch is reported once.
        let mut want_sig = want_sig;
        let mut shape_error = false;
        if let Some(w) = &want_sig {
            if w.params.len() != lit.params.len() {
                let shown = self.tn(want.expect("a signature came from it"));
                let msg = format!("this closure takes {}, but `{shown}` takes {}", parameters(lit.params.len()), parameters(w.params.len()));
                self.error(e.span, msg);
                want_sig = None;
                shape_error = true;
            }
        }
        let expect_given = want_sig.is_some();
        let mut tys = Vec::with_capacity(lit.params.len());
        for (i, p) in lit.params.iter().enumerate() {
            let ty = match (&p.ty, want_sig.as_ref().and_then(|w| w.params.get(i))) {
                (Some(t), _) => self.resolve(t),
                (None, Some(wp)) => wp.ty,
                (None, None) => {
                    // (Where the shape was wrong, that was said already.)
                    if !matches!(want, Some(Ty::Fn(_))) {
                        let name = &p.bind.name;
                        let msg = format!(
                            "give `{name}` a type: `({name}: int) {{ ... }}`; Kestrel can only leave it out where a function type is expected"
                        );
                        self.error(p.bind.span, msg);
                    }
                    Ty::Error
                }
            };
            tys.push(ty);
        }
        let declared = lit.ret.as_ref().map(|t| self.resolve(t));
        // (`map` gives the item type, and takes the result type from the body.)
        let known = declared.or(want_sig.as_ref().filter(|_| !infer_result).map(|w| w.ret));

        self.push_frame(e.id, known.is_none());
        self.ret = known.unwrap_or(Ty::Error);
        self.scopes.push(HashMap::new());
        for (p, &ty) in lit.params.iter().zip(&tys) {
            self.declare(&p.bind, ty, p.is_var, true, p.is_var);
        }
        let ret = self.check_closure_body(e, lit, known);
        self.scopes.pop();
        let mut frame = self.pop_frame();
        let captures = self.finish_captures(e.id, std::mem::take(&mut frame.reads));

        let is_pure = frame.impure.is_none();
        let params = lit.params.iter().zip(&tys).map(|(p, &ty)| SigParam { ty, is_var: p.is_var }).collect();
        let sig = self.t.intern_sig(Sig { params, ret, is_pure });
        let mut refused = false;
        if let (true, Some(w), Some(why)) = (expect_given, &want_sig, &frame.impure) {
            if w.is_pure {
                let what = match &arg_name {
                    Some(name) => format!("`{name}` needs a pure one"),
                    None => "a `pure` function is expected here".to_string(),
                };
                self.error(e.span, format!("this closure isn't pure (it {why}), but {what}"));
                refused = true;
            }
        }
        self.t.closures.insert(
            e.id,
            ClosureInfo {
                sig,
                captures,
                is_pure,
                locals: frame.locals,
                mutates_params: frame.mutates_params,
                encl_fn: self.fn_id,
            },
        );
        // Said already: the mismatch of the types needn't be said again.
        if refused || shape_error || tys.contains(&Ty::Error) {
            return Ty::Error;
        }
        Ty::Fn(sig)
    }

    fn push_frame(&mut self, id: ExprId, infer: bool) {
        let saved = Saved {
            ctxs: std::mem::replace(&mut self.ctxs, vec![Ctx::Function]),
            moved: self.moved.clone(),
            discarded: self.discarded.take(),
            ret: self.ret,
        };
        self.frames.push(ClosureFrame {
            id,
            first_local: self.t.locals.len() as u32,
            locals: Vec::new(),
            loop_floor: self.loop_breaks.len(),
            removing_floor: self.removing.loops_len(),
            par_len: self.par.len(),
            infer: infer.then_some(Infer { found: None, bare_return: None }),
            impure: None,
            mutates_params: false,
            reads: Reads::default(),
            saved,
        });
    }

    fn pop_frame(&mut self) -> ClosureFrame {
        let mut frame = self.frames.pop().expect("a closure frame is open");
        self.ctxs = std::mem::take(&mut frame.saved.ctxs);
        self.moved = std::mem::take(&mut frame.saved.moved);
        self.discarded = frame.saved.discarded;
        self.ret = frame.saved.ret;
        frame
    }

    /// Checks the body and gives the closure's result type: the one written or expected (`known`),
    /// or else the one the body's `return`s and tail show.
    fn check_closure_body(&mut self, e: &Expr, lit: &ClosureLit, known: Option<Ty>) -> Ty {
        let expect = match known {
            Some(Ty::Unit) => Expect::Discard,
            Some(r) => Expect::Value(Some(r)),
            None => Expect::Infer,
        };
        let body = self.check_block(&lit.body, expect);
        let tail = match lit.body.stmts.last() {
            Some(Stmt::Expr(t)) => Some(t),
            _ => None,
        };
        let ret = match known {
            Some(r) => r,
            None => self.inferred_result(tail, body),
        };
        if let (Some(t), false) = (tail, ret == Ty::Unit) {
            self.consume(t);
            self.check_return_region(t);
        }
        // An unresolved result type was reported already; don't pile on.
        if !matches!(ret, Ty::Unit | Ty::Error) && body == Ty::Unit {
            let msg = format!("this closure must return {}, but its body can finish without a value", self.tn(ret));
            self.error_help(e.span, msg, "end the body with a value or a `return`");
        }
        ret
    }
}
