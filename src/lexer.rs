use crate::diagnostic::Diagnostic;
use crate::token::{Span, Token, TokenKind};

pub fn lex(source_name: &str, source: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let mut lexer = Lexer {
        source_name,
        source,
        pos: 0,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    lexer.run();
    if lexer.errors.is_empty() {
        Ok(lexer.tokens)
    } else {
        Err(lexer.errors)
    }
}

struct Lexer<'a> {
    source_name: &'a str,
    source: &'a str,
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<Diagnostic>,
}

impl Lexer<'_> {
    fn run(&mut self) {
        while self.pos < self.source.len() {
            let start = self.pos;
            let byte = self.byte(0);
            match byte {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' => {
                    self.pos += 1;
                    self.push(TokenKind::Newline, start);
                }
                b'/' if self.byte(1) == b'/' => self.comment(),
                b'0'..=b'9' => self.number(start),
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.identifier(start),
                b'"' => self.string(start),
                b'(' => self.one(TokenKind::LParen, start),
                b')' => self.one(TokenKind::RParen, start),
                b'{' => self.one(TokenKind::LBrace, start),
                b'}' => self.one(TokenKind::RBrace, start),
                b',' => self.one(TokenKind::Comma, start),
                b':' => self.one(TokenKind::Colon, start),
                b';' => self.one(TokenKind::Semicolon, start),
                b'+' => self.either(b'=', TokenKind::PlusEq, TokenKind::Plus, start),
                b'-' if self.byte(1) == b'>' => self.two(TokenKind::Arrow, start),
                b'-' => self.either(b'=', TokenKind::MinusEq, TokenKind::Minus, start),
                b'*' => self.one(TokenKind::Star, start),
                b'/' => self.one(TokenKind::Slash, start),
                b'%' => self.one(TokenKind::Percent, start),
                b'!' => self.either(b'=', TokenKind::BangEq, TokenKind::Bang, start),
                b'=' => self.either(b'=', TokenKind::EqEq, TokenKind::Eq, start),
                b'<' => self.either(b'=', TokenKind::LtEq, TokenKind::Lt, start),
                b'>' => self.either(b'=', TokenKind::GtEq, TokenKind::Gt, start),
                b'&' if self.byte(1) == b'&' => self.two(TokenKind::AndAnd, start),
                b'|' if self.byte(1) == b'|' => self.two(TokenKind::OrOr, start),
                b'.' if self.byte(1) == b'.' => self.two(TokenKind::DotDot, start),
                _ => {
                    self.pos += source_char_len(self.source, self.pos);
                    self.error(Span::new(start, self.pos), "unexpected character");
                }
            }
        }
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.pos, self.pos),
        });
    }

    fn byte(&self, offset: usize) -> u8 {
        self.source
            .as_bytes()
            .get(self.pos + offset)
            .copied()
            .unwrap_or(0)
    }

    fn one(&mut self, kind: TokenKind, start: usize) {
        self.pos += 1;
        self.push(kind, start);
    }

    fn two(&mut self, kind: TokenKind, start: usize) {
        self.pos += 2;
        self.push(kind, start);
    }

    fn either(&mut self, next: u8, two: TokenKind, one: TokenKind, start: usize) {
        if self.byte(1) == next {
            self.two(two, start);
        } else {
            self.one(one, start);
        }
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
        });
    }

    fn comment(&mut self) {
        while self.pos < self.source.len() && self.byte(0) != b'\n' {
            self.pos += 1;
        }
    }

    fn number(&mut self, start: usize) {
        while self.byte(0).is_ascii_digit() {
            self.pos += 1;
        }
        let text = &self.source[start..self.pos];
        match text.parse::<i64>() {
            Ok(value) => self.push(TokenKind::Int(value), start),
            Err(_) => self.error(
                Span::new(start, self.pos),
                "integer literal is outside i64 range",
            ),
        }
    }

    fn identifier(&mut self, start: usize) {
        while self.byte(0).is_ascii_alphanumeric() || self.byte(0) == b'_' {
            self.pos += 1;
        }
        let text = &self.source[start..self.pos];
        let kind = match text {
            "fn" => TokenKind::Fn,
            "let" => TokenKind::Let,
            "var" => TokenKind::Var,
            "return" => TokenKind::Return,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "int" => TokenKind::IntTy,
            "bool" => TokenKind::BoolTy,
            "string" => TokenKind::StringTy,
            _ => TokenKind::Ident(text.to_owned()),
        };
        self.push(kind, start);
    }

    fn string(&mut self, start: usize) {
        self.pos += 1;
        let mut value = String::new();
        let mut terminated = false;
        while self.pos < self.source.len() {
            match self.byte(0) {
                b'"' => {
                    self.pos += 1;
                    terminated = true;
                    break;
                }
                b'\n' | b'\r' => break,
                b'\\' => {
                    let escape_start = self.pos;
                    self.pos += 1;
                    let escaped = match self.byte(0) {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'"' => '"',
                        b'\\' => '\\',
                        _ => {
                            self.pos += 1;
                            self.error(Span::new(escape_start, self.pos), "unknown string escape");
                            continue;
                        }
                    };
                    self.pos += 1;
                    value.push(escaped);
                }
                _ => {
                    if let Some(ch) = self.source[self.pos..].chars().next() {
                        value.push(ch);
                        self.pos += ch.len_utf8();
                    }
                }
            }
        }
        if terminated {
            self.push(TokenKind::String(value), start);
        } else {
            self.error(Span::new(start, self.pos), "unterminated string literal");
        }
    }

    fn error(&mut self, span: Span, message: &str) {
        self.errors.push(Diagnostic::new(
            self.source_name,
            self.source,
            span,
            message,
        ));
    }
}

fn source_char_len(source: &str, pos: usize) -> usize {
    source[pos..].chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_keywords_operators_and_utf8_string() {
        let tokens = lex("test", "var x = \"hé\"\nx += 2").expect("lex");
        assert!(matches!(tokens[0].kind, TokenKind::Var));
        assert!(
            tokens
                .iter()
                .any(|token| matches!(token.kind, TokenKind::PlusEq))
        );
        assert!(
            tokens
                .iter()
                .any(|token| matches!(&token.kind, TokenKind::String(s) if s == "hé"))
        );
    }

    #[test]
    fn rejects_large_integer() {
        let errors = lex("test", "999999999999999999999999").expect_err("must fail");
        assert!(errors[0].message.contains("outside i64"));
    }
}
