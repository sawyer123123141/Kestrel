use crate::span::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(u64),
    /// An integer written in hex (`0xFF`): it may use its type's full width (see
    /// `Program::hex_literals`). A separate token, not a flag on `Token`, which would make every
    /// `Token` on the parser's stack bigger.
    Hex(u64),
    Float(f64),
    Str(String),
    /// A string with `{...}` interpolation.
    StrParts(Vec<StrPart>),
    // keywords
    Pure, Let, Var, If, Else, While, For, From, To, In, Par, Match, Struct, Enum, Interface, Arena,
    Return, Break, Continue, And, Or, Not, True, False, NoneKw, Use, Pub, SelfKw, Native, Try,
    // punctuation
    LParen, RParen, LBrace, RBrace, LBracket, RBracket, Comma, Colon, Dot, Arrow, Question,
    Semi, Newline, At,
    // operators
    Plus, Minus, Star, Slash, Percent, Assign, EqEq, NotEq, Lt, Gt, Le, Ge,
    PlusEq, MinusEq, StarEq, SlashEq, PercentEq,
    /// `*% +% -%` and `*%= +%= -%=`: arithmetic that wraps around and never checks.
    StarPercent, PlusPercent, MinusPercent, StarPercentEq, PlusPercentEq, MinusPercentEq,
    Amp, Pipe, Caret, Tilde, Shl, Shr, AmpEq, PipeEq, CaretEq, ShlEq, ShrEq,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    Lit(String),
    /// Source text of an interpolated expression and its byte offset in the file.
    Code { text: String, start: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub fn keyword(s: &str) -> Option<Tok> {
    Some(match s {
        "pure" => Tok::Pure,
        "let" => Tok::Let,
        "var" => Tok::Var,
        "if" => Tok::If,
        "else" => Tok::Else,
        "while" => Tok::While,
        "for" => Tok::For,
        "from" => Tok::From,
        "to" => Tok::To,
        "in" => Tok::In,
        "par" => Tok::Par,
        "match" => Tok::Match,
        "struct" => Tok::Struct,
        "enum" => Tok::Enum,
        "interface" => Tok::Interface,
        "arena" => Tok::Arena,
        "return" => Tok::Return,
        "break" => Tok::Break,
        "continue" => Tok::Continue,
        "and" => Tok::And,
        "or" => Tok::Or,
        "not" => Tok::Not,
        "true" => Tok::True,
        "false" => Tok::False,
        "none" => Tok::NoneKw,
        "use" => Tok::Use,
        "pub" => Tok::Pub,
        "self" => Tok::SelfKw,
        "native" => Tok::Native,
        "try" => Tok::Try,
        _ => return None,
    })
}

impl Tok {
    /// A line ending in this token continues onto the next line.
    pub fn continues_line(&self) -> bool {
        use Tok::*;
        matches!(
            self,
            Plus | Minus | Star | Slash | Percent | Assign | EqEq | NotEq | Lt | Gt | Le | Ge
                | PlusEq | MinusEq | StarEq | SlashEq | PercentEq | And | Or | Comma | Arrow
                | Dot | LBrace | Amp | Pipe | Caret | Tilde | Shl | Shr | AmpEq | PipeEq | CaretEq
                | ShlEq | ShrEq | StarPercent | PlusPercent | MinusPercent | StarPercentEq
                | PlusPercentEq | MinusPercentEq
        )
    }

    /// How the token reads in an error message.
    pub fn describe(&self) -> String {
        use Tok::*;
        match self {
            Ident(s) => format!("name `{s}`"),
            Int(n) | Hex(n) => format!("number `{n}`"),
            Float(f) => format!("number `{f}`"),
            Str(_) | StrParts(_) => "a string".into(),
            Newline => "end of line".into(),
            Eof => "end of file".into(),
            other => format!("`{}`", other.symbol()),
        }
    }

    fn symbol(&self) -> &'static str {
        use Tok::*;
        match self {
            Pure => "pure", Let => "let", Var => "var", If => "if", Else => "else",
            While => "while", For => "for", From => "from", To => "to", In => "in",
            Par => "par", Match => "match", Struct => "struct", Enum => "enum", Interface => "interface",
            Arena => "arena", Return => "return", Break => "break", Continue => "continue",
            And => "and", Or => "or", Not => "not", True => "true", False => "false", NoneKw => "none",
            Use => "use", Pub => "pub", SelfKw => "self", Native => "native", Try => "try",
            LParen => "(", RParen => ")", LBrace => "{", RBrace => "}", LBracket => "[",
            RBracket => "]", Comma => ",", Colon => ":", Dot => ".", Arrow => "->",
            Question => "?", Semi => ";", At => "@", Plus => "+", Minus => "-", Star => "*",
            Slash => "/", Percent => "%", Assign => "=", EqEq => "==", NotEq => "!=",
            Lt => "<", Gt => ">", Le => "<=", Ge => ">=", PlusEq => "+=", MinusEq => "-=",
            StarEq => "*=", SlashEq => "/=", PercentEq => "%=",
            Amp => "&", Pipe => "|", Caret => "^", Tilde => "~", Shl => "<<", Shr => ">>",
            AmpEq => "&=", PipeEq => "|=", CaretEq => "^=", ShlEq => "<<=", ShrEq => ">>=",
            StarPercent => "*%", PlusPercent => "+%", MinusPercent => "-%", StarPercentEq => "*%=",
            PlusPercentEq => "+%=", MinusPercentEq => "-%=",
            Ident(_) | Int(_) | Hex(_) | Float(_) | Str(_) | StrParts(_) | Newline | Eof => "",
        }
    }
}
