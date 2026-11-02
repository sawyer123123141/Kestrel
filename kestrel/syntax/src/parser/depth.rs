//! A limit on how deep code nests. The compiler's passes recurse over the syntax tree, so a tree
//! nested deep enough (generated code: 50,000 parentheses, a sum of 100,000 terms) would overflow
//! their stacks. The parser refuses anything nested more than `MAX_DEPTH` levels, so no later
//! pass sees a deeper tree, and stops parsing the file at the first such error.
//!
//! Two things are counted. `depth` is the number of constructs open around the current token
//! (brackets, blocks other than a function's body, unary operators, `if`, `match`, string
//! interpolation), which bounds the parser's own recursion. And each expression has a height: the
//! levels of expressions and blocks below it, which also counts what the parser builds in loops,
//! like `a + b + c` (left-nested) and `a.b.c` or `a[0][0]` (postfix chains). An expression nests
//! too deep when the constructs open around it plus its height pass the limit.

use super::{PResult, Parser};
use crate::ast::{Expr, ExprKind, InterpPart};
use crate::diag::Diagnostic;
use crate::span::Span;

/// The most levels an expression, a block or a type may nest.
const MAX_DEPTH: u32 = 1000;

const EXPR_TOO_DEEP: &str = "this expression nests more than 1,000 levels deep; split it into smaller parts with `let`";
const TYPE_TOO_DEEP: &str = "this type nests more than 1,000 levels deep; simplify it";

impl Parser {
    /// Runs `f` one level deeper: inside a bracket, block, unary operator, `if`, `match` or
    /// string interpolation that starts at the current token.
    pub(super) fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        if self.depth >= MAX_DEPTH {
            self.too_deep(self.span(), EXPR_TOO_DEEP);
            return Err(());
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }

    /// Runs `f` one level deeper inside a type (`[T]`, `list[T]`).
    pub(super) fn nested_type<T>(&mut self, f: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        if self.type_depth >= MAX_DEPTH {
            self.too_deep(self.span(), TYPE_TOO_DEEP);
            return Err(());
        }
        self.type_depth += 1;
        let result = f(self);
        self.type_depth -= 1;
        result
    }

    /// Reports code nested too deep, once, and stops parsing: everything after this point is
    /// skipped (the current token becomes the end of the file), and later errors are dropped.
    fn too_deep(&mut self, span: Span, msg: &str) {
        if self.too_deep_at.is_none() {
            self.diags.push(Diagnostic::error(span, msg));
            self.too_deep_at = Some(self.diags.len());
        }
        self.pos = self.toks.len() - 1;
    }

    /// Whether parsing stopped at code nested too deep.
    pub(super) fn stopped(&self) -> bool {
        self.too_deep_at.is_some()
    }

    /// The diagnostics up to the one for code nested too deep, if there is one.
    pub(super) fn finish_diags(&mut self) -> Vec<Diagnostic> {
        let mut diags = std::mem::take(&mut self.diags);
        if let Some(n) = self.too_deep_at {
            diags.truncate(n);
        }
        diags
    }

    /// The height of `e`: 0 for a name or literal, one more than its tallest part otherwise.
    pub(super) fn height(&self, e: &Expr) -> u32 {
        self.heights[(e.id.0 - self.first_expr) as usize]
    }

    /// Records the height of a new expression of `kind` (`inner` is the height of the blocks it
    /// holds, for `if` and `match`), and reports it when it nests too deep.
    pub(super) fn note_height(&mut self, kind: &ExprKind, span: Span, inner: u32) {
        let parts = match kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Str(_) | ExprKind::Ident(_) => 0,
            ExprKind::Interp(parts) => self.tallest(parts.iter().filter_map(|p| match p {
                InterpPart::Expr(e) => Some(e),
                InterpPart::Lit(_) => None,
            })),
            ExprKind::Unary { expr, .. } | ExprKind::Try(expr) => self.height(expr) + 1,
            ExprKind::Binary { lhs, rhs, .. } => self.height(lhs).max(self.height(rhs)) + 1,
            ExprKind::Call { args, .. } | ExprKind::TypeCall { args, .. } => self.tallest(args.iter().map(|a| &a.value)),
            ExprKind::If { cond, .. } => self.height(cond) + 1,
            ExprKind::CallValue { callee, args } => self.tallest(std::iter::once(&**callee).chain(args.iter().map(|a| &a.value))),
            ExprKind::Field { base, .. } => self.height(base) + 1,
            ExprKind::MethodCall { receiver, args, .. } => self.tallest(std::iter::once(&**receiver).chain(args.iter().map(|a| &a.value))),
            ExprKind::Match { scrutinee, .. } => self.height(scrutinee) + 1,
            ExprKind::Index { base, index } => self.height(base).max(self.height(index)) + 1,
            ExprKind::Brackets { base, index, .. } => index.iter().map(|i| self.height(i)).fold(self.height(base), u32::max) + 1,
            ExprKind::TypeApply { .. } => 0,
            ExprKind::ArrayLit(items) => self.tallest(items.iter()),
            ExprKind::MapLit(entries) => self.tallest(entries.iter().flat_map(|(k, v)| [k, v])),
            // The body is a block of its own: its height comes in as `inner`.
            ExprKind::Closure(_) => 0,
        };
        let h = parts.max(inner);
        self.heights.push(h);
        if let Some(top) = self.block_heights.last_mut() {
            *top = (*top).max(h);
        }
        if self.depth + h > MAX_DEPTH {
            // Where the expression ends is where it got too deep: a chain's last term.
            let end = Span { start: span.end.saturating_sub(1).max(span.start), end: span.end };
            self.too_deep(end, EXPR_TOO_DEEP);
        }
    }

    /// One more than the tallest of `parts`, or 0 when there are none.
    fn tallest<'e>(&self, parts: impl Iterator<Item = &'e Expr>) -> u32 {
        parts.map(|e| self.height(e) + 1).max().unwrap_or(0)
    }

    /// Starts a block: its height is one more than the tallest expression or block inside it.
    pub(super) fn open_block(&mut self) {
        self.block_heights.push(0);
    }

    /// Ends the block `open_block` started; returns its height, which also counts toward the
    /// block around it.
    pub(super) fn close_block(&mut self) -> u32 {
        let h = self.block_heights.pop().expect("a block is open") + 1;
        if let Some(top) = self.block_heights.last_mut() {
            *top = (*top).max(h);
        }
        h
    }
}
