use super::habits::RangeAt;
use super::{PResult, Parser};
use crate::ast::{AssignOp, Binding, Block, Stmt};
use crate::span::Span;
use crate::token::Tok;

impl Parser {
    /// A block inside another one: a level deeper (see `depth`).
    pub(crate) fn block(&mut self) -> PResult<Block> {
        Ok(self.block_with_height()?.0)
    }

    /// `block`, and its height.
    pub(crate) fn block_with_height(&mut self) -> PResult<(Block, u32)> {
        self.nested(Self::fn_body)
    }

    /// A function's body: the outermost block, so not a level of its own. Also gives its height.
    pub(crate) fn fn_body(&mut self) -> PResult<(Block, u32)> {
        self.open_block();
        let block = self.inside_brackets(Self::block_contents);
        let h = self.close_block();
        Ok((block?, h))
    }

    fn block_contents(&mut self) -> PResult<Block> {
        let start = self.expect(&Tok::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            match self.stmt() {
                Ok(s) => {
                    // `@assume(a) @assume(b)`: an assumption may be followed by another annotation.
                    let more_annots = matches!(s, Stmt::Assume { .. }) && self.at(&Tok::At);
                    stmts.push(s);
                    let ended = more_annots || self.at(&Tok::RBrace) || self.eat(&Tok::Newline) || self.eat(&Tok::Semi);
                    if !ended {
                        let found = self.peek().describe();
                        self.error_here(format!("expected end of line, found {found}"));
                        self.recover_stmt();
                    }
                }
                Err(()) => self.recover_stmt(),
            }
            self.skip_newlines();
        }
        let end = self.expect(&Tok::RBrace, "`}`")?;
        Ok(Block { stmts, span: start.to(end) })
    }

    pub(super) fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        match self.peek() {
            Tok::At => self.annotated_stmt(),
            Tok::Let | Tok::Var => self.let_stmt(),
            Tok::For => self.for_stmt(false),
            Tok::While => {
                self.bump();
                let cond = self.in_head(Self::expr)?;
                let body = self.block()?;
                Ok(Stmt::While { span: start.to(body.span), cond, body })
            }
            Tok::Par => {
                self.bump();
                if !self.at(&Tok::For) {
                    self.error_here(format!("expected `for` after `par`, found {}", self.peek().describe()));
                    return Err(());
                }
                self.for_stmt(true)
            }
            Tok::Arena => {
                self.bump();
                let body = self.block()?;
                Ok(Stmt::Arena { span: start.to(body.span), body })
            }
            Tok::In => {
                self.bump();
                let arena = self.in_head(Self::expr)?;
                let body = self.block()?;
                Ok(Stmt::In { span: start.to(body.span), arena, body })
            }
            Tok::Break => {
                self.bump();
                Ok(Stmt::Break(start))
            }
            Tok::Continue => {
                self.bump();
                Ok(Stmt::Continue(start))
            }
            Tok::Return => {
                self.bump();
                let ends = matches!(self.peek(), Tok::Newline | Tok::Semi | Tok::RBrace | Tok::Eof);
                let value = if ends { None } else { Some(self.expr()?) };
                let span = value.as_ref().map_or(start, |v| start.to(v.span));
                Ok(Stmt::Return { value, span })
            }
            // `remove b`: `remove` isn't a keyword, so only a name right after it makes this statement.
            Tok::Ident(_) if self.at_elif() => {
                self.elif();
                Err(())
            }
            Tok::Ident(kw) if kw == "remove" && matches!(self.peek_at(1), Tok::Ident(_)) => {
                self.bump();
                let (name, name_span) = self.expect_ident("a loop item's name")?;
                Ok(Stmt::Remove { name, name_span, span: start.to(name_span) })
            }
            _ => self.expr_or_assign(),
        }
    }

    fn let_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let is_var = self.at(&Tok::Var);
        self.bump();
        if let (false, Tok::Ident(m), Tok::Ident(name)) = (is_var, self.peek(), self.peek_at(1)) {
            if m == "mut" {
                self.error_here(format!("Kestrel writes a variable that can change as `var {name}`, not `let mut {name}`"));
                return Err(());
            }
        }
        let (name, name_span) = self.expect_ident("a variable name")?;
        let bind = self.binding(name, name_span);
        let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
        if let (Some(t), true) = (&ty, self.at(&Tok::LParen)) {
            let kw = if is_var { "var" } else { "let" };
            return self.value_in_type_place(&format!("{kw} {}", bind.name), t);
        }
        self.expect(&Tok::Assign, "`=`")?;
        let value = self.expr()?;
        let (mut else_name, mut else_block) = (None, None);
        if self.eat(&Tok::Else) {
            // `else why { ... }`: the name the error gets in the block.
            if let Tok::Ident(name) = self.peek().clone() {
                let span = self.span();
                self.bump();
                else_name = Some(self.binding(name, span));
            }
            else_block = Some(self.block()?);
        }
        let end = else_block.as_ref().map_or(value.span, |b| b.span);
        Ok(Stmt::Let { is_var, bind, ty, span: start.to(end), value, else_name, else_block })
    }

    /// `for ...`, after `par` when `par` is true (only the `in` forms then).
    fn for_stmt(&mut self, par: bool) -> PResult<Stmt> {
        let start = self.span();
        self.bump();
        if self.at(&Tok::Var) {
            self.for_var(par);
        }
        let (name, name_span) = self.expect_ident("a loop variable name")?;
        let first = self.binding(name, name_span);
        if self.eat(&Tok::Comma) {
            let (name, name_span) = self.expect_ident("a loop variable name")?;
            let var = self.binding(name, name_span);
            return self.for_in(par, start, Some(first), var);
        }
        if self.at(&Tok::In) {
            return self.for_in(par, start, None, first);
        }
        if par {
            self.error_here("`par for` needs items to work on; loop over an array or list: `par for p in ps { ... }`");
            return Err(());
        }
        let var = first;
        self.expect(&Tok::From, "`from`")?;
        let head = RangeAt::ForHead { first: self.pos as u32, par: false };
        let lo = self.with_range_at(head, |p| p.in_head(Self::expr))?;
        self.expect(&Tok::To, "`to`")?;
        let hi = self.in_head(Self::expr)?;
        let body = self.block()?;
        Ok(Stmt::ForRange { var, start: lo, end: hi, span: start.to(body.span), body })
    }

    fn for_in(&mut self, par: bool, start: Span, index: Option<Binding>, var: Binding) -> PResult<Stmt> {
        self.expect(&Tok::In, "`in`")?;
        let head = RangeAt::ForHead { first: self.pos as u32, par };
        let iter = self.with_range_at(head, |p| p.in_head(Self::expr))?;
        let body = self.block()?;
        Ok(Stmt::ForIn { par, index, var, iter, span: start.to(body.span), body })
    }

    fn expr_or_assign(&mut self) -> PResult<Stmt> {
        let target = self.expr()?;
        let op = match self.peek() {
            Tok::Assign => AssignOp::Set,
            Tok::PlusEq => AssignOp::Add,
            Tok::MinusEq => AssignOp::Sub,
            Tok::StarEq => AssignOp::Mul,
            Tok::SlashEq => AssignOp::Div,
            Tok::PercentEq => AssignOp::Rem,
            Tok::AmpEq => AssignOp::BitAnd,
            Tok::PipeEq => AssignOp::BitOr,
            Tok::CaretEq => AssignOp::BitXor,
            Tok::ShlEq => AssignOp::Shl,
            Tok::ShrEq => AssignOp::Shr,
            Tok::StarPercentEq => AssignOp::MulWrap,
            Tok::PlusPercentEq => AssignOp::AddWrap,
            Tok::MinusPercentEq => AssignOp::SubWrap,
            _ => return Ok(Stmt::Expr(target)),
        };
        self.bump();
        let value = self.expr()?;
        Ok(Stmt::Assign { span: target.span.to(value.span), target, op, value })
    }
}
