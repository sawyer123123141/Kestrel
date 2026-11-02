//! Lowering `fail(message)` and `exit(code)`: statements that end their block, as `return` does.
//! Nothing after them runs, so no drop or arena pop goes before them.

use kestrel_check::Builtin;
use kestrel_syntax::ast;

use super::FnLowerer;
use crate::ir::*;

impl FnLowerer<'_> {
    /// `fail(message)` or `exit(code)` (built-in `b`) at `e`.
    pub(super) fn stop_stmt(&mut self, b: Builtin, e: &ast::Expr, args: &[ast::Arg], out: &mut Vec<Stmt>) {
        let line = self.line(e.span);
        let value = self.expr(&args[0].value);
        out.push(match b {
            Builtin::Fail => Stmt::Fail { message: value, line },
            Builtin::Exit => Stmt::Exit { code: value, line },
            other => unreachable!("{other:?} doesn't stop the program"),
        });
    }
}
