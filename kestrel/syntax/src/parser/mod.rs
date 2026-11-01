mod angles;
mod annot;
mod branch;
mod brackets;
mod closure;
mod depth;
mod expr;
mod generics;
mod habits;
mod iface;
mod item;
mod members;
mod primary;
mod stmt;

use std::collections::HashMap;

use crate::ast::{BindId, Binding, Expr, ExprId, ExprKind, Program, ProveAnnot, TypeExpr, TypeKind};
use crate::diag::Diagnostic;
use crate::span::Span;
use crate::token::{Tok, Token};

pub(crate) type PResult<T> = Result<T, ()>;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
    next_expr: u32,
    next_bind: u32,
    /// Constructs open around the current token, and type levels (see `depth`).
    depth: u32,
    type_depth: u32,
    /// Each expression's height, by id from `first_expr` (see `depth`).
    heights: Vec<u32>,
    first_expr: u32,
    /// Per block being parsed, innermost last: the height of the tallest thing in it so far.
    block_heights: Vec<u32>,
    /// Set when code nested too deep: how many diagnostics to keep (the rest came after it).
    too_deep_at: Option<usize>,
    /// Set while parsing the head of an `if`, `while`, `for`, `match` or `in`, outside any bracket
    /// of its own: `(cond) {` is then a parenthesised expression and its block, not a closure
    /// (see `closure`).
    no_closure_head: bool,
    /// For each open bracket token (by index), the index of its closing one; found by scanning
    /// for a closure and kept (see `closure`).
    closes: HashMap<usize, usize>,
    /// The file's source text and where its bytes start in the program (see `parse_at`): the
    /// text of an `@assume`'s condition, and the line breaks after an annotation, come from it.
    /// Empty (from `new` and `with_ids`) when the caller has none: both then fall back to empty.
    src: String,
    base: u32,
    /// The `@prove`s read so far, one per target (see `annot`).
    proves: Vec<ProveAnnot>,
    /// The ids of the integer literals written in hex.
    hex_literals: Vec<ExprId>,
    /// Where a `..` range would be if one were written here, for its message (see `habits`).
    range_at: habits::RangeAt,
    /// The first field of each enum variant read so far, for a `match` arm's message (see
    /// `habits`); `None` when enums that share the variant's name start with different fields.
    first_fields: HashMap<String, Option<String>>,
    /// Where error recovery stops early: the token after a type written with angle brackets,
    /// when a line break follows it that the `>` hid (see `angles`).
    resume_at: Option<usize>,
}

/// What `Parser::attempt` puts back on a failure (see `snapshot`).
#[derive(Clone, Copy)]
struct Snapshot {
    pos: usize,
    diags: usize,
    depth: u32,
    type_depth: u32,
    too_deep_at: Option<usize>,
    hex_literals: usize,
    proves: usize,
    blocks: usize,
    block_top: Option<u32>,
}

impl Parser {
    /// `toks` must end with `Tok::Eof` (as `lex` guarantees).
    pub fn new(toks: Vec<Token>) -> Self {
        Self::with_ids(toks, 0, 0)
    }

    /// Like `new`, numbering expressions and bindings from these values
    /// (so several files can share one set of side tables).
    pub fn with_ids(toks: Vec<Token>, next_expr: u32, next_bind: u32) -> Self {
        Parser {
            toks,
            pos: 0,
            diags: Vec::new(),
            next_expr,
            next_bind,
            depth: 0,
            type_depth: 0,
            heights: Vec::new(),
            first_expr: next_expr,
            block_heights: Vec::new(),
            too_deep_at: None,
            no_closure_head: false,
            closes: HashMap::new(),
            src: String::new(),
            base: 0,
            proves: Vec::new(),
            hex_literals: Vec::new(),
            range_at: habits::RangeAt::Plain,
            first_fields: HashMap::new(),
            resume_at: None,
        }
    }

    /// Gives the parser the text of the file its tokens come from, whose bytes start at `base`.
    pub fn with_source(mut self, src: &str, base: u32) -> Self {
        self.src = src.to_string();
        self.base = base;
        self
    }

    pub fn parse_program(mut self) -> (Program, Vec<Diagnostic>) {
        let items = self.items();
        let proves = std::mem::take(&mut self.proves);
        let hex_literals = std::mem::take(&mut self.hex_literals);
        let program = Program {
            items,
            expr_count: self.next_expr,
            bind_count: self.next_bind,
            builtin_ranges: Vec::new(),
            proves,
            hex_literals,
        };
        (program, self.finish_diags())
    }

    /// Where the parser is, so `restore` can put it back: the position and everything a parse
    /// since then may have added or changed. Ids handed out stay unused, and `heights` keeps its
    /// entries (it is indexed by id, and each id has one).
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            pos: self.pos,
            diags: self.diags.len(),
            depth: self.depth,
            type_depth: self.type_depth,
            too_deep_at: self.too_deep_at,
            hex_literals: self.hex_literals.len(),
            proves: self.proves.len(),
            blocks: self.block_heights.len(),
            block_top: self.block_heights.last().copied(),
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.pos = s.pos;
        self.resume_at = None;
        self.diags.truncate(s.diags);
        self.depth = s.depth;
        self.type_depth = s.type_depth;
        self.too_deep_at = s.too_deep_at;
        self.hex_literals.truncate(s.hex_literals);
        self.proves.truncate(s.proves);
        // A failed closure body leaves its `open_block` unpopped.
        self.block_heights.truncate(s.blocks);
        if let (Some(top), Some(saved)) = (self.block_heights.last_mut(), s.block_top) {
            *top = saved;
        }
    }

    /// Runs `f`; when it fails, puts the parser back as it was (position, diagnostics, depths,
    /// hex literals, `@prove`s, block heights) and gives `None`: the failure leaves no trace.
    pub(crate) fn attempt<T>(&mut self, f: impl FnOnce(&mut Self) -> PResult<T>) -> Option<T> {
        let saved = self.snapshot();
        match f(self) {
            Ok(v) => Some(v),
            Err(()) => {
                self.restore(saved);
                None
            }
        }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    /// Consumes the current token. Never moves past `Eof`.
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok, what: &str) -> PResult<Span> {
        if self.at(t) {
            Ok(self.bump().span)
        } else {
            let found = self.peek().describe();
            self.error_here(format!("expected {what}, found {found}"));
            Err(())
        }
    }

    fn expect_ident(&mut self, what: &str) -> PResult<(String, Span)> {
        if let Tok::Ident(name) = self.peek().clone() {
            let span = self.bump().span;
            Ok((name, span))
        } else {
            let found = self.peek().describe();
            self.error_here(format!("expected {what}, found {found}"));
            Err(())
        }
    }

    /// `a.b.c`: names joined by dots (a module's name, or an interface's full name), and the span
    /// of them all.
    fn expect_dotted(&mut self, what: &str) -> PResult<(String, Span)> {
        let (mut name, mut span) = self.expect_ident(what)?;
        while self.eat(&Tok::Dot) {
            let (part, end) = self.expect_ident(what)?;
            name = format!("{name}.{part}");
            span = span.to(end);
        }
        Ok((name, span))
    }

    fn skip_newlines(&mut self) {
        while self.at(&Tok::Newline) || self.at(&Tok::Semi) {
            self.bump();
        }
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(span, msg));
    }

    fn error_here(&mut self, msg: impl Into<String>) {
        let span = self.span();
        self.error(span, msg);
    }

    fn expr_node(&mut self, kind: ExprKind, span: Span) -> Expr {
        self.expr_node_holding(kind, span, 0)
    }

    /// `expr_node` for an expression that holds blocks (`if`, `match`): `inner` is their height.
    fn expr_node_holding(&mut self, kind: ExprKind, span: Span, inner: u32) -> Expr {
        self.note_height(&kind, span, inner);
        let id = ExprId(self.next_expr);
        self.next_expr += 1;
        Expr { id, kind, span }
    }

    fn binding(&mut self, name: String, span: Span) -> Binding {
        let id = BindId(self.next_bind);
        self.next_bind += 1;
        Binding { id, name, span }
    }

    /// `name: vec3()` where `=` should follow the type: the value was written where the type
    /// goes. `decl` is what comes before the `:` (`var velocity`, or a field's name).
    fn value_in_type_place<T>(&mut self, decl: &str, ty: &TypeExpr) -> PResult<T> {
        let mut d = Diagnostic::error(self.span(), "a type goes after `:`, and the value after `=`");
        // A built-in scalar has no `int()`-style value to suggest.
        const SCALARS: &[&str] = &["int", "i64", "u8", "float", "f64", "bool", "string", "GpuHandle"];
        if matches!(&ty.kind, TypeKind::Named { name, .. } if !SCALARS.contains(&name.as_str())) {
            let t = type_text(ty);
            d = d.with_help(format!("write `{decl}: {t} = {t}()`"));
        }
        self.diags.push(d);
        Err(())
    }

    /// Whether error recovery stops at the current token (see `resume_at`).
    fn resumes_here(&mut self) -> bool {
        let here = self.resume_at == Some(self.pos);
        if here {
            self.resume_at = None;
        }
        here
    }

    /// Skips to the end of the current statement: a newline or `;` at this
    /// nesting level, or the `}` that closes the enclosing block.
    fn recover_stmt(&mut self) {
        let mut depth = 0i32;
        loop {
            if depth == 0 && self.resumes_here() {
                return;
            }
            match self.peek() {
                Tok::Eof => return,
                Tok::LBrace | Tok::LParen | Tok::LBracket => depth += 1,
                Tok::RBrace if depth == 0 => return,
                Tok::RBrace | Tok::RParen | Tok::RBracket => depth = (depth - 1).max(0),
                Tok::Newline | Tok::Semi if depth == 0 => return,
                _ => {}
            }
            self.bump();
        }
    }

    /// Skips to the next top-level item: a newline outside any braces.
    fn recover_item(&mut self) {
        let mut depth = 0i32;
        loop {
            if depth <= 0 && self.resumes_here() {
                return;
            }
            match self.peek() {
                Tok::Eof => return,
                Tok::LBrace => depth += 1,
                Tok::RBrace => depth -= 1,
                Tok::Newline if depth <= 0 => return,
                _ => {}
            }
            self.bump();
        }
    }
}

/// A type as written: `vec3`, `list[int]`, `[int]`, `Color?`.
fn type_text(t: &TypeExpr) -> String {
    match &t.kind {
        TypeKind::Named { name, args } if args.is_empty() => name.clone(),
        TypeKind::Named { name, args } => format!("{name}[{}]", args.iter().map(type_text).collect::<Vec<_>>().join(", ")),
        TypeKind::Array(inner) => format!("[{}]", type_text(inner)),
        TypeKind::Optional(inner) => format!("{}?", type_text(inner)),
        TypeKind::Fn { .. } => t.to_sexpr(),
    }
}
