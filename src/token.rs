#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Int(i64),
    String(String),
    Fn,
    Let,
    Var,
    Return,
    If,
    Else,
    For,
    In,
    True,
    False,
    IntTy,
    BoolTy,
    StringTy,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,
    Newline,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Eq,
    PlusEq,
    MinusEq,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AndAnd,
    OrOr,
    Arrow,
    DotDot,
    Eof,
}

impl TokenKind {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Ident(_) => "identifier",
            Self::Int(_) => "integer",
            Self::String(_) => "string",
            Self::Eof => "end of file",
            Self::Newline => "newline",
            _ => "token",
        }
    }
}
