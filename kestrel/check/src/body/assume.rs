//! `@assume(cond)`: the condition is an ordinary `bool` expression, and its form is limited to
//! what the range analysis can use and what is safe to skip under `--release`: comparisons of
//! integers joined with `and`, whose sides are names (locals, parameters, constants), fields of
//! them, integer literals, `.len`, `+`, `-`, `*`, unary `-` and parentheses. Such a condition has
//! no effects and can fail only by its own arithmetic. (Syntax pick 2, provisional: see
//! `2026-10-18-at-annotations-design.md`; `side` is the one place to widen it.)

use kestrel_syntax::ast::{BinaryOp, Expr, ExprKind, UnaryOp};
use kestrel_syntax::Span;

use super::BodyChecker;
use crate::ty::Ty;

const FORM: &str = "`@assume` takes comparisons of integers joined with `and`, like `@assume(i < xs.len and n >= 0)`";
const NO_CALLS: &str =
    "`@assume` can't call a function or index a list (a `--release` build may not run it); compare names, fields, numbers and `.len`";

/// What is wrong with a condition, and where.
type Bad = (Span, &'static str);

impl<'a> BodyChecker<'a> {
    /// Checks the condition as a `bool`, then (when that found nothing wrong) its form. One error
    /// at most: the first problem found.
    pub(super) fn check_assume(&mut self, cond: &Expr) {
        let before = self.diags.len();
        self.check_value(cond, Some(Ty::Bool));
        if self.diags.len() == before {
            if let Err((span, msg)) = self.assume_conds(cond, cond) {
                self.error(span, msg);
            }
        }
    }

    /// `e` is `a and b`, or a comparison. `whole` is the condition, where a form error points.
    fn assume_conds(&self, e: &Expr, whole: &Expr) -> Result<(), Bad> {
        match &e.kind {
            ExprKind::Binary { op: BinaryOp::And, lhs, rhs } => {
                self.assume_conds(lhs, whole)?;
                self.assume_conds(rhs, whole)
            }
            ExprKind::Binary { op: BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne, lhs, rhs } => {
                self.assume_side(lhs, whole)?;
                self.assume_side(rhs, whole)
            }
            _ => Err((whole.span, FORM)),
        }
    }

    /// Whether `e` is an allowed side of a comparison; if not, what to say: the first call or
    /// index in it, else the form error.
    fn assume_side(&self, e: &Expr, whole: &Expr) -> Result<(), Bad> {
        let bad = || Err(self.calls_in(e).unwrap_or((whole.span, FORM)));
        if !self.t.ty(e.id).is_int() {
            return bad();
        }
        match &e.kind {
            ExprKind::Int(_) | ExprKind::Ident(_) => Ok(()),
            ExprKind::Field { base, .. } => self.assume_path(base, whole),
            ExprKind::Unary { op: UnaryOp::Neg, expr } => self.assume_side(expr, whole),
            ExprKind::Binary { op: BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul, lhs, rhs } => {
                self.assume_side(lhs, whole)?;
                self.assume_side(rhs, whole)
            }
            _ => bad(),
        }
    }

    /// A field's base: a name or fields of one.
    fn assume_path(&self, e: &Expr, whole: &Expr) -> Result<(), Bad> {
        match &e.kind {
            ExprKind::Ident(_) => Ok(()),
            ExprKind::Field { base, .. } => self.assume_path(base, whole),
            _ => Err(self.calls_in(e).unwrap_or((whole.span, FORM))),
        }
    }

    /// The first call or index in `e`, as an error to report.
    fn calls_in(&self, e: &Expr) -> Option<Bad> {
        match &e.kind {
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::CallValue { .. } | ExprKind::TypeCall { .. } | ExprKind::Index { .. } => {
                Some((e.span, NO_CALLS))
            }
            ExprKind::Field { base, .. } => self.calls_in(base),
            ExprKind::Unary { expr, .. } | ExprKind::Try(expr) => self.calls_in(expr),
            ExprKind::Binary { lhs, rhs, .. } => self.calls_in(lhs).or_else(|| self.calls_in(rhs)),
            ExprKind::Brackets { .. } => unreachable!("settled after parsing"),
            _ => None,
        }
    }
}
