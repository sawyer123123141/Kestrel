use super::{PResult, Parser};
use crate::ast::{Block, ConstDecl, FnDecl, FnTypeParam, Item, Param, TypeExpr, TypeKind, UseClause, UseDecl};
use crate::span::Span;
use crate::token::Tok;

impl Parser {
    pub(crate) fn items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(&Tok::Eof) {
            match self.item() {
                Ok(item) => {
                    items.push(item);
                    let ended = self.at(&Tok::Eof) || self.eat(&Tok::Newline) || self.eat(&Tok::Semi);
                    if !ended {
                        let found = self.peek().describe();
                        self.error_here(format!("expected end of line, found {found}"));
                        self.recover_item();
                    }
                }
                Err(()) => self.recover_item(),
            }
            self.skip_newlines();
        }
        items
    }

    fn item(&mut self) -> PResult<Item> {
        let pending = self.annots_before_item()?;
        let item = self.item_after_annots()?;
        self.attach_to_item(pending, &item);
        Ok(item)
    }

    fn item_after_annots(&mut self) -> PResult<Item> {
        let start = self.span();
        let is_pub = self.eat(&Tok::Pub);
        match self.peek().clone() {
            Tok::Let => self.const_decl(is_pub, start),
            Tok::Pure => {
                self.bump();
                if self.eat(&Tok::Native) {
                    self.native_decl(is_pub, true, start)
                } else {
                    self.fn_decl(is_pub, true, start)
                }
            }
            Tok::Native => {
                self.bump();
                self.native_decl(is_pub, false, start)
            }
            Tok::Struct => self.struct_decl(is_pub, start),
            Tok::Ident(kw) if kw == "fn" || (matches!(kw.as_str(), "func" | "def" | "function") && matches!(self.peek_at(1), Tok::Ident(_))) => {
                self.error_here(format!("Kestrel functions don't use `{kw}`: write `name(params) {{ ... }}`"));
                Err(())
            }
            Tok::Ident(_) => self.fn_decl(is_pub, false, start),
            Tok::Var => {
                self.error_here("global variables aren't supported yet");
                Err(())
            }
            Tok::Enum => self.enum_decl(is_pub, start),
            Tok::Interface => self.interface_decl(is_pub, start),
            Tok::Use if is_pub => {
                self.error_here("`use` can't be `pub`");
                Err(())
            }
            Tok::Use => self.use_decl(start),
            other => {
                self.error_here(format!("expected a function or `let`, found {}", other.describe()));
                Err(())
            }
        }
    }

    /// `use m`, `use { a, b from m }`; braces may hold several clauses,
    /// one per line. A list without `from` names whole modules. A module's name may be names
    /// joined by dots (`use net.packet`), which name folders.
    fn use_decl(&mut self, start: Span) -> PResult<Item> {
        self.bump();
        let mut clauses = Vec::new();
        if !self.eat(&Tok::LBrace) {
            let (module, module_span) = self.expect_dotted("a module name")?;
            clauses.push(UseClause { module, module_span, names: Vec::new() });
            return Ok(Item::Use(UseDecl { clauses, span: start.to(module_span) }));
        }
        loop {
            self.skip_newlines();
            if self.at(&Tok::RBrace) {
                break;
            }
            let mut names = vec![self.expect_dotted("a name or a module")?];
            while self.eat(&Tok::Comma) {
                names.push(self.expect_dotted("a name or a module")?);
            }
            if self.eat(&Tok::From) {
                if let Some((name, span)) = names.iter().find(|(n, _)| n.contains('.')) {
                    let msg = format!("`{name}` is a module's name; a name before `from` is one item of the module after it, like `use {{ Packet from net.packet }}`");
                    self.error(*span, msg);
                    return Err(());
                }
                let (module, module_span) = self.expect_dotted("a module name")?;
                clauses.push(UseClause { module, module_span, names });
            } else {
                clauses.extend(names.into_iter().map(|(module, module_span)| UseClause { module, module_span, names: Vec::new() }));
            }
            if !(self.eat(&Tok::Newline) || self.eat(&Tok::Semi)) && !self.at(&Tok::RBrace) {
                let found = self.peek().describe();
                self.error_here(format!("expected `from`, a new line or `}}`, found {found}"));
                return Err(());
            }
        }
        let end = self.expect(&Tok::RBrace, "`}`")?;
        Ok(Item::Use(UseDecl { clauses, span: start.to(end) }))
    }

    fn fn_decl(&mut self, is_pub: bool, is_pure: bool, start: Span) -> PResult<Item> {
        let (name, name_span) = self.expect_ident("a function name")?;
        Ok(Item::Fn(self.fn_rest(name, name_span, is_pub, is_pure, start, None)?))
    }

    /// Everything after a function's name. `owner` is the struct a method belongs to.
    pub(super) fn fn_rest(
        &mut self,
        name: String,
        name_span: Span,
        is_pub: bool,
        is_pure: bool,
        start: Span,
        owner: Option<&str>,
    ) -> PResult<FnDecl> {
        let type_params = self.type_params()?;
        let (params, ret, _) = self.signature(owner)?;
        let body = self.fn_body()?.0;
        let span = start.to(body.span);
        Ok(FnDecl { name, name_span, is_pure, is_pub, is_native: false, type_params, params, ret, body, span })
    }

    /// `(params) -> Type`, the return type being optional. Also gives the span
    /// of the signature's last token.
    pub(super) fn signature(&mut self, owner: Option<&str>) -> PResult<(Vec<Param>, Option<TypeExpr>, Span)> {
        self.expect(&Tok::LParen, "`(`")?;
        let mut params = Vec::new();
        while !self.at(&Tok::RParen) {
            params.push(self.param(params.len(), owner)?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        let mut end = self.expect(&Tok::RParen, "`)` or `,`")?;
        let ret = if self.eat(&Tok::Arrow) {
            let ty = self.type_expr()?;
            end = ty.span;
            Some(ty)
        } else {
            None
        };
        Ok((params, ret, end))
    }

    /// `native name(params) -> Type`, after `native`: a function Rust implements,
    /// so it has no block. Its body is an empty block at the end of the signature.
    fn native_decl(&mut self, is_pub: bool, is_pure: bool, start: Span) -> PResult<Item> {
        let (name, name_span) = self.expect_ident("a function name")?;
        if self.at(&Tok::LBracket) {
            // Reported and dropped: with no item, the driver's check for user natives has
            // nothing to add, and `recover_item` skips the rest of the declaration.
            self.refuse_type_params("native functions can't have placeholders")?;
            return Err(());
        }
        let (params, ret, end) = self.signature(None)?;
        if self.at(&Tok::LBrace) {
            self.error_here("native functions have no body; remove the `{ }`");
            return Err(());
        }
        let body = Block { stmts: Vec::new(), span: Span { start: end.end, end: end.end } };
        let type_params = Vec::new();
        Ok(Item::Fn(FnDecl { name, name_span, is_pure, is_pub, is_native: true, type_params, params, ret, body, span: start.to(end) }))
    }

    fn param(&mut self, index: usize, owner: Option<&str>) -> PResult<Param> {
        let is_var = self.eat(&Tok::Var);
        if self.at(&Tok::SelfKw) {
            let span = self.bump().span;
            let Some(owner) = owner.filter(|_| index == 0) else {
                self.error(span, "`self` is only allowed as the first parameter of a method");
                return Err(());
            };
            let bind = self.binding("self".to_string(), span);
            let ty = TypeExpr { kind: TypeKind::Named { name: owner.to_string(), args: Vec::new() }, span };
            return Ok(Param { bind, is_var, is_self: true, ty });
        }
        let (name, span) = self.expect_ident("a parameter name")?;
        let bind = self.binding(name, span);
        self.expect(&Tok::Colon, "`:` and a type")?;
        let ty = self.type_expr()?;
        Ok(Param { bind, is_var, is_self: false, ty })
    }

    fn const_decl(&mut self, is_pub: bool, start: Span) -> PResult<Item> {
        self.bump();
        let (name, name_span) = self.expect_ident("a constant name")?;
        let bind = self.binding(name, name_span);
        let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
        if let (Some(t), true) = (&ty, self.at(&Tok::LParen)) {
            return self.value_in_type_place(&format!("let {}", bind.name), t);
        }
        self.expect(&Tok::Assign, "`=`")?;
        let value = self.expr()?;
        let span = start.to(value.span);
        Ok(Item::Const(ConstDecl { is_pub, bind, ty, value, span }))
    }

    /// `(A, var B) -> R`, with `pure` before it for a pure function type. The result is optional:
    /// without `->` the function gives nothing.
    #[inline(never)]
    fn fn_type(&mut self) -> PResult<TypeExpr> {
        if self.peek_at(1) == &Tok::Lt && !self.at(&Tok::LParen) {
            return self.angle_type();
        }
        let start = self.span();
        let is_pure = self.eat(&Tok::Pure);
        let (params, mut end) = self.nested_type(|p| {
            p.expect(&Tok::LParen, "`(`")?;
            let mut params = Vec::new();
            while !p.at(&Tok::RParen) {
                let is_var = p.eat(&Tok::Var);
                params.push(FnTypeParam { is_var, ty: p.type_expr()? });
                if !p.eat(&Tok::Comma) {
                    break;
                }
            }
            Ok((params, p.expect(&Tok::RParen, "`)` or `,`")?))
        })?;
        let ret = if self.eat(&Tok::Arrow) {
            let ret = self.nested_type(Self::type_expr)?;
            end = ret.span;
            Some(Box::new(ret))
        } else {
            None
        };
        let mut ty = TypeExpr { kind: TypeKind::Fn { params, ret, is_pure }, span: start.to(end) };
        if self.at(&Tok::Question) {
            let end = self.bump().span;
            ty = TypeExpr { span: ty.span.to(end), kind: TypeKind::Optional(Box::new(ty)) };
        }
        Ok(ty)
    }

    pub(crate) fn type_expr(&mut self) -> PResult<TypeExpr> {
        if self.at(&Tok::LParen) || (self.at(&Tok::Pure) && self.peek_at(1) == &Tok::LParen) || self.peek_at(1) == &Tok::Lt {
            // A function type (or another language's `list<Item>`) is parsed apart: this frame is
            // on the stack once per nesting level.
            return self.fn_type();
        }
        let start = self.span();
        let mut ty = if self.at(&Tok::LBracket) {
            let (inner, end) = self.nested_type(|p| {
                p.bump();
                let inner = p.type_expr()?;
                Ok((inner, p.expect(&Tok::RBracket, "`]`")?))
            })?;
            TypeExpr { kind: TypeKind::Array(Box::new(inner)), span: start.to(end) }
        } else {
            let (mut name, span) = self.expect_ident("a type")?;
            let mut end = span;
            // A full name, `net.packet.Packet`: the linker finds the module.
            while self.at(&Tok::Dot) && matches!(self.peek_at(1), Tok::Ident(_)) {
                self.bump();
                let (part, part_span) = self.expect_ident("a type")?;
                name = format!("{name}.{part}");
                end = part_span;
            }
            let mut args = Vec::new();
            if self.at(&Tok::LBracket) {
                (args, end) = self.bracket_types()?;
            }
            TypeExpr { kind: TypeKind::Named { name, args }, span: start.to(end) }
        };
        if self.at(&Tok::Question) {
            let end = self.bump().span;
            ty = TypeExpr { span: ty.span.to(end), kind: TypeKind::Optional(Box::new(ty)) };
        }
        Ok(ty)
    }

    /// `[A, B]` after a type's or an interface's name, at the `[`: the types and the `]`.
    pub(super) fn bracket_types(&mut self) -> PResult<(Vec<TypeExpr>, Span)> {
        self.nested_type(|p| {
            p.bump();
            let mut args = Vec::new();
            loop {
                args.push(p.type_expr()?);
                if !p.eat(&Tok::Comma) {
                    break;
                }
            }
            Ok((args, p.expect(&Tok::RBracket, "`]`")?))
        })
    }
}
