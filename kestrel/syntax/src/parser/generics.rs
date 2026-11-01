//! Placeholder lists: `[T]`, `[T: Damageable + Named, K: key]` after a struct, enum or function
//! name. A rule is an identifier here (an interface or one of `equatable`, `ordered`, `key`);
//! the checker tells them apart.

use super::{PResult, Parser};
use crate::ast::TypeParam;
use crate::token::Tok;

impl Parser {
    /// `[T, K: key]` after a name; empty when the next token isn't `[`.
    pub(super) fn type_params(&mut self) -> PResult<Vec<TypeParam>> {
        if self.at(&Tok::Lt) {
            self.angle_type_params();
            return Err(());
        }
        if !self.at(&Tok::LBracket) {
            return Ok(Vec::new());
        }
        self.nested_type(|p| {
            p.bump();
            let mut params = Vec::new();
            loop {
                let (name, span) = p.expect_ident("a placeholder name")?;
                let mut rules = Vec::new();
                if p.eat(&Tok::Colon) {
                    loop {
                        rules.push(p.expect_dotted("a rule: an interface name, `equatable`, `ordered` or `key`")?);
                        if !p.eat(&Tok::Plus) {
                            break;
                        }
                    }
                }
                params.push(TypeParam { name, span, rules });
                if !p.eat(&Tok::Comma) {
                    break;
                }
            }
            p.expect(&Tok::RBracket, "`]` or `,`")?;
            Ok(params)
        })
    }

    /// A placeholder list where none is allowed: says so with `msg` at the `[`, then reads past
    /// the list (so the rest of the declaration still parses). Does nothing without a `[`.
    pub(super) fn refuse_type_params(&mut self, msg: &str) -> PResult<()> {
        if self.at(&Tok::LBracket) {
            self.error_here(msg);
            self.type_params()?;
        }
        Ok(())
    }

    /// A method's own placeholders (`convert[U](self)`) are refused for now (R8).
    pub(super) fn refuse_method_params(&mut self) -> PResult<()> {
        self.refuse_type_params("methods can't have placeholders of their own yet; use a generic function that takes the struct instead")
    }
}
