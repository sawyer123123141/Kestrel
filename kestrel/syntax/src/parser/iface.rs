//! Interface declarations, and the `: A, B` list after a struct's or enum's name.

use super::{PResult, Parser};
use crate::ast::{Block, FnDecl, InterfaceDecl, Item};
use crate::span::Span;
use crate::token::Tok;

const IFACE_PLACEHOLDERS: &str = "interfaces don't take placeholders yet; write the interface's methods with concrete types";

impl Parser {
    /// `: A, B` after a type's name: the interfaces it implements (by full name too,
    /// `rollback.Rollback`). Empty without the colon.
    pub(super) fn iface_list(&mut self) -> PResult<Vec<(String, Span)>> {
        let mut list = Vec::new();
        if self.eat(&Tok::Colon) {
            loop {
                list.push(self.expect_dotted("an interface name")?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        Ok(list)
    }

    /// `interface Name { [pure] name(self, params) [-> Type] ... }`, one signature per line.
    pub(super) fn interface_decl(&mut self, is_pub: bool, start: Span) -> PResult<Item> {
        self.bump();
        let (name, name_span) = self.expect_ident("an interface name")?;
        if self.at(&Tok::Lt) {
            self.error_here(IFACE_PLACEHOLDERS);
            return Err(());
        }
        self.refuse_type_params(IFACE_PLACEHOLDERS)?;
        self.expect(&Tok::LBrace, "`{`")?;
        let mut methods = Vec::new();
        self.skip_newlines();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            match self.iface_method(&name) {
                Ok(m) => {
                    methods.push(m);
                    let ended = self.at(&Tok::RBrace) || self.eat(&Tok::Newline) || self.eat(&Tok::Semi);
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
        Ok(Item::Interface(InterfaceDecl { is_pub, name, name_span, methods, span: start.to(end) }))
    }

    /// One method signature, after any annotations.
    fn iface_method(&mut self, owner: &str) -> PResult<FnDecl> {
        let pending = self.annots_before_item()?;
        let method = self.iface_method_after_annots(owner)?;
        self.attach_to_fn(pending, &method);
        Ok(method)
    }

    /// A method signature, which has no body: its `FnDecl` body is empty, at the signature's end.
    fn iface_method_after_annots(&mut self, owner: &str) -> PResult<FnDecl> {
        let start = self.span();
        let is_pure = self.eat(&Tok::Pure);
        if self.at(&Tok::Native) {
            self.error_here("native functions can't be methods");
            return Err(());
        }
        let (name, name_span) = self.expect_ident("a method name")?;
        if !self.at(&Tok::LParen) {
            let found = self.peek().describe();
            self.error_here(format!("expected `(` for a method, found {found}; interfaces have methods only"));
            return Err(());
        }
        let (params, ret, end) = self.signature(Some(owner))?;
        if self.at(&Tok::LBrace) {
            self.error_here("interface methods have no body; remove the `{ }`");
            return Err(());
        }
        let body = Block { stmts: Vec::new(), span: Span { start: end.end, end: end.end } };
        let type_params = Vec::new();
        Ok(FnDecl { name, name_span, is_pure, is_pub: false, is_native: false, type_params, params, ret, body, span: start.to(end) })
    }
}
