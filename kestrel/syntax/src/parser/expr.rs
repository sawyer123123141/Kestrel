use super::{PResult, Parser};
use crate::ast::{BinaryOp, Expr, ExprKind, UnaryOp};
use crate::token::Tok;

fn cmp_op(t: &Tok) -> Option<BinaryOp> {
    Some(match t {
        Tok::EqEq => BinaryOp::Eq,
        Tok::NotEq => BinaryOp::Ne,
        Tok::Lt => BinaryOp::Lt,
        Tok::Gt => BinaryOp::Gt,
        Tok::Le => BinaryOp::Le,
        Tok::Ge => BinaryOp::Ge,
        _ => return None,
    })
}

impl Parser {
    pub(crate) fn expr(&mut self) -> PResult<Expr> {
        self.or_expr()
    }

    fn binary(&mut self, op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        let span = lhs.span.to(rhs.span);
        self.expr_node(ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span)
    }

    fn or_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.and_expr()?;
        while self.eat(&Tok::Or) {
            let rhs = self.and_expr()?;
            lhs = self.binary(BinaryOp::Or, lhs, rhs);
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.cmp_expr()?;
        while self.eat(&Tok::And) {
            let rhs = self.cmp_expr()?;
            lhs = self.binary(BinaryOp::And, lhs, rhs);
        }
        Ok(lhs)
    }

    fn cmp_expr(&mut self) -> PResult<Expr> {
        let lhs = self.bit_or()?;
        let Some(op) = cmp_op(self.peek()) else { return Ok(lhs) };
        let op_at = self.pos;
        self.bump();
        let rhs = self.bit_or()?;
        let e = self.binary(op, lhs, rhs);
        // Another comparison (or `list<int>()`, `Map<string, int>()`) after it.
        if self.after_comparison(&e, op_at) {
            return Err(());
        }
        Ok(e)
    }

    /// `|`, then `^`, then `&`, then shifts, each binding tighter (as in GDScript).
    fn bit_or(&mut self) -> PResult<Expr> {
        self.left_assoc(&[(Tok::Pipe, BinaryOp::BitOr)], Self::bit_xor)
    }

    fn bit_xor(&mut self) -> PResult<Expr> {
        self.left_assoc(&[(Tok::Caret, BinaryOp::BitXor)], Self::bit_and)
    }

    fn bit_and(&mut self) -> PResult<Expr> {
        self.left_assoc(&[(Tok::Amp, BinaryOp::BitAnd)], Self::shift)
    }

    fn shift(&mut self) -> PResult<Expr> {
        self.left_assoc(&[(Tok::Shl, BinaryOp::Shl), (Tok::Shr, BinaryOp::Shr)], Self::add_expr)
    }

    /// `operand (op operand)*` for the operators in `ops`.
    fn left_assoc(&mut self, ops: &[(Tok, BinaryOp)], operand: fn(&mut Self) -> PResult<Expr>) -> PResult<Expr> {
        let mut lhs = operand(self)?;
        while let Some(&(_, op)) = ops.iter().find(|(t, _)| self.at(t)) {
            self.bump();
            let rhs = operand(self)?;
            lhs = self.binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    fn add_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.mul_expr()?;
        loop {
            if self.at_step() {
                self.step_spelling(&lhs);
                return Err(());
            }
            let op = match self.peek() {
                Tok::Plus => BinaryOp::Add,
                Tok::Minus => BinaryOp::Sub,
                Tok::PlusPercent => BinaryOp::AddWrap,
                Tok::MinusPercent => BinaryOp::SubWrap,
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.mul_expr()?;
            lhs = self.binary(op, lhs, rhs);
        }
    }

    fn mul_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinaryOp::Mul,
                Tok::Slash => BinaryOp::Div,
                Tok::Percent => BinaryOp::Rem,
                Tok::StarPercent => BinaryOp::MulWrap,
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.unary()?;
            lhs = self.binary(op, lhs, rhs);
        }
    }

    fn unary(&mut self) -> PResult<Expr> {
        let start = self.span();
        if self.eat(&Tok::Try) {
            let inner = self.nested(Self::unary)?;
            let span = start.to(inner.span);
            return Ok(self.expr_node(ExprKind::Try(Box::new(inner)), span));
        }
        let op = match self.peek() {
            Tok::Minus => UnaryOp::Neg,
            Tok::Not => UnaryOp::Not,
            Tok::Tilde => UnaryOp::BitNot,
            _ => return self.postfix(),
        };
        self.bump();
        let inner = self.nested(Self::unary)?;
        let span = start.to(inner.span);
        Ok(self.expr_node(ExprKind::Unary { op, expr: Box::new(inner) }, span))
    }

    /// A primary expression followed by any `.field` / `.method(args)` / `[index]` / `(args)`.
    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.at(&Tok::LBracket) {
                // After a name or a field the brackets may hold types (`best[Enemy]`).
                e = if matches!(e.kind, ExprKind::Ident(_) | ExprKind::Field { .. }) {
                    self.brackets_after(e)?
                } else {
                    self.index_after(e)?
                };
                continue;
            }
            if self.at(&Tok::LParen) {
                // Not a bare name (`primary` takes those) and not `.name(`: a call of a value.
                let (args, end) = self.call_args()?;
                let span = e.span.to(end);
                e = self.expr_node(ExprKind::CallValue { callee: Box::new(e), args }, span);
                continue;
            }
            if self.at(&Tok::Dot) && self.peek_at(1) == &Tok::Dot {
                self.range_spelling();
                return Err(());
            }
            if self.at(&Tok::Colon) && self.peek_at(1) == &Tok::Colon {
                self.path_spelling(&e);
                return Err(());
            }
            if !self.eat(&Tok::Dot) {
                break;
            }
            let (name, name_span) = self.expect_ident("a field or method name")?;
            if self.at(&Tok::LParen) {
                let (args, end) = self.call_args()?;
                let span = e.span.to(end);
                let kind = ExprKind::MethodCall { receiver: Box::new(e), name, name_span, args };
                e = self.expr_node(kind, span);
            } else {
                let span = e.span.to(name_span);
                e = self.expr_node(ExprKind::Field { base: Box::new(e), name, name_span }, span);
            }
        }
        Ok(e)
    }
}
