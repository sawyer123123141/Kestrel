use kestrel_syntax::ast;

use super::{kty, FnLowerer};
use crate::ir::*;

/// One lowered arm: its variant (`None` for `else`), statements and value.
type Arm = (Option<u32>, Vec<Stmt>, Option<Expr>);

impl<'a> FnLowerer<'a> {
    /// The place being matched: the scrutinee itself when it is one,
    /// otherwise a temporary holding its value.
    fn scrutinee(&mut self, e: &ast::Expr, pre: &mut Vec<Stmt>) -> Place {
        self.place_or_temp(e, pre, true)
    }

    /// Registers each arm's binding as a view of `place`, then lowers its body.
    fn arms(&mut self, e: &ast::Expr, place: &Place, as_value: bool) -> Vec<Arm> {
        let ast::ExprKind::Match { arms, .. } = &e.kind else { unreachable!("not a match") };
        let variants = self.t.match_arms[&e.id].clone();
        let mut out = Vec::with_capacity(arms.len());
        for (arm, variant) in arms.iter().zip(variants) {
            if let (ast::Pattern::Variant { binding: Some(b), .. }, Some(v)) = (&arm.pattern, variant) {
                let l = self.t.bind_local[&b.id];
                self.bindings.insert(l, (place.clone(), v));
            }
            let (stmts, value) = if as_value { self.block_value(&arm.body) } else { (self.block_stmts(&arm.body), None) };
            out.push((variant, stmts, value));
        }
        out
    }

    /// `tag(place) == variant`
    pub(super) fn tag_is(&self, place: &Place, variant: u32, line: u32) -> Expr {
        let tag = Expr { kind: ExprKind::Tag(place.clone()), ty: KTy::U8, line };
        let v = Expr { kind: ExprKind::Int(variant as i64), ty: KTy::U8, line };
        Expr { kind: ExprKind::Binary(BinOp::Eq, Box::new(tag), Box::new(v)), ty: KTy::Bool, line }
    }

    /// A `match` used as a value: a chain of `if`s on the tag. The last arm
    /// needs no test: the checker made the arms exhaustive.
    pub(crate) fn match_value(&mut self, e: &ast::Expr) -> ExprKind {
        let ast::ExprKind::Match { scrutinee, .. } = &e.kind else { unreachable!("not a match") };
        let (ty, line) = (kty(self.ty(e.id)), self.line(e.span));
        let mut pre = Vec::new();
        let place = self.scrutinee(scrutinee, &mut pre);
        let mut arms = self.arms(e, &place, true);
        let (_, mut else_body, mut else_val) = arms.pop().expect("a match has at least one arm");
        for (variant, then_body, then_val) in arms.into_iter().rev() {
            let cond = self.tag_is(&place, variant.expect("only the last arm can be `else`"), line);
            let kind = ExprKind::If {
                cond: Box::new(cond),
                then_body,
                then_val: then_val.map(Box::new),
                else_body,
                else_val: else_val.map(Box::new),
            };
            else_body = Vec::new();
            else_val = Some(Expr { kind, ty, line });
        }
        let value = else_val.expect("a value match has an arm that yields a value");
        pre.extend(else_body);
        if pre.is_empty() {
            value.kind
        } else {
            ExprKind::Seq(pre, Box::new(value))
        }
    }

    /// A `match` whose value is discarded: a chain of `if` statements.
    pub(crate) fn match_stmt(&mut self, e: &ast::Expr, out: &mut Vec<Stmt>) {
        let ast::ExprKind::Match { scrutinee, .. } = &e.kind else { unreachable!("not a match") };
        let line = self.line(e.span);
        let place = self.scrutinee(scrutinee, out);
        let mut arms = self.arms(e, &place, false);
        let (_, mut chain, _) = arms.pop().expect("a match has at least one arm");
        for (variant, then_body, _) in arms.into_iter().rev() {
            let cond = self.tag_is(&place, variant.expect("only the last arm can be `else`"), line);
            chain = vec![Stmt::If { cond, then_body, else_body: chain }];
        }
        out.extend(chain);
    }
}
