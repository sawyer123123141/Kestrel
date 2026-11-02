//! Interface declarations, and the `: A, B` list after a struct's or enum's name.

use super::{PResult, Parser};
use crate::ast::{Block, FnDecl, IfaceRef, InterfaceDecl, Item};
use crate::span::Span;
use crate::token::Tok;

const METHOD_PLACEHOLDERS: &str =
    "interface methods can't have placeholders of their own; put the placeholder on the interface, like `interface Handler[E]`";

impl Parser {
    /// `: A, B` after a type's name: the interfaces it implements (by full name too,
    /// `rollback.Rollback`, and with types, `Handler[Click]`). Empty without the colon.
    pub(super) fn iface_list(&mut self) -> PResult<Vec<IfaceRef>> {
        let mut list = Vec::new();
        if self.eat(&Tok::Colon) {
            loop {
                list.push(self.iface_ref("an interface name")?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        Ok(list)
    }

    /// `Name`, `module.Name` or either with its types, `Handler[Click]`: an interface (or a rule
    /// word) where a list or a rule names it.
    pub(super) fn iface_ref(&mut self, what: &str) -> PResult<IfaceRef> {
        let (name, span) = self.expect_dotted(what)?;
        let args = if self.at(&Tok::LBracket) { self.bracket_types()?.0 } else { Vec::new() };
        Ok(IfaceRef { name, span, args })
    }

    /// `interface Name[P] { [pure] name(self, params) [-> Type] ... }`, one signature per line.
    pub(super) fn interface_decl(&mut self, is_pub: bool, start: Span) -> PResult<Item> {
        self.bump();
        let (name, name_span) = self.expect_ident("an interface name")?;
        // `interface Named<T>` gets the message `struct Stack<T>` gets.
        let type_params = self.type_params()?;
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
        Ok(Item::Interface(InterfaceDecl { is_pub, name, name_span, type_params, methods, span: start.to(end) }))
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
        self.refuse_type_params(METHOD_PLACEHOLDERS)?;
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
