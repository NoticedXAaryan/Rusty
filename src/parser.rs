use crate::ast::{self, AssignOp, BinaryOp, Expr, ExprKind, Stmt, StmtKind, TypeName, UnaryOp};
use crate::diagnostic::Diagnostic;
use crate::token::{Span, Token, TokenKind};

pub fn parse(
    source_name: &str,
    source: &str,
    tokens: Vec<Token>,
) -> Result<ast::Program, Vec<Diagnostic>> {
    let mut parser = Parser {
        source_name,
        source,
        tokens,
        pos: 0,
    };
    match parser.program() {
        Ok(program) => Ok(program),
        Err(error) => Err(vec![error]),
    }
}

type PResult<T> = Result<T, Diagnostic>;

struct Parser<'a> {
    source_name: &'a str,
    source: &'a str,
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser<'_> {
    fn program(&mut self) -> PResult<ast::Program> {
        let mut functions = Vec::new();
        self.separators();
        while !self.at(&TokenKind::Eof) {
            functions.push(self.function()?);
            self.separators();
        }
        if functions.is_empty() {
            return Err(self.error(self.current().span, "expected at least one function"));
        }
        Ok(ast::Program { functions })
    }

    fn function(&mut self) -> PResult<ast::Function> {
        let start = self.expect(&TokenKind::Fn, "expected `fn`")?.span.start;
        let (name, _) = self.identifier("expected function name")?;
        self.expect(&TokenKind::LParen, "expected `(` after function name")?;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            loop {
                let (param_name, span) = self.identifier("expected parameter name")?;
                self.expect(&TokenKind::Colon, "expected `:` after parameter name")?;
                let ty = self.type_name()?;
                params.push(ast::Param {
                    name: param_name,
                    ty,
                    span,
                });
                if !self.take(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen, "expected `)` after parameters")?;
        let result = if self.take(&TokenKind::Arrow) {
            Some(self.type_name()?)
        } else {
            None
        };
        self.separators();
        let (body, end) = self.block()?;
        Ok(ast::Function {
            name,
            params,
            result,
            body,
            span: Span::new(start, end),
        })
    }

    fn type_name(&mut self) -> PResult<TypeName> {
        let kind = self.current().kind.clone();
        let ty = match kind {
            TokenKind::IntTy => TypeName::Int,
            TokenKind::BoolTy => TypeName::Bool,
            TokenKind::StringTy => TypeName::String,
            _ => {
                return Err(self.error(
                    self.current().span,
                    "expected type (`int`, `bool`, or `string`)",
                ));
            }
        };
        self.pos += 1;
        Ok(ty)
    }

    fn block(&mut self) -> PResult<(ast::Block, usize)> {
        self.expect(&TokenKind::LBrace, "expected `{`")?;
        self.separators();
        let mut statements = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(self.error(self.current().span, "unterminated block; expected `}`"));
            }
            statements.push(self.statement()?);
            if !self.at(&TokenKind::RBrace) && !self.is_separator() {
                return Err(self.error(
                    self.current().span,
                    "expected newline or `;` after statement",
                ));
            }
            self.separators();
        }
        let end = self.expect(&TokenKind::RBrace, "expected `}`")?.span.end;
        Ok((statements, end))
    }

    fn statement(&mut self) -> PResult<Stmt> {
        let start = self.current().span.start;
        let kind = match self.current().kind.clone() {
            TokenKind::Let => {
                self.pos += 1;
                self.let_statement(false)?
            }
            TokenKind::Var => {
                self.pos += 1;
                self.let_statement(true)?
            }
            TokenKind::Return => {
                self.pos += 1;
                self.return_statement()?
            }
            TokenKind::If => {
                self.pos += 1;
                self.if_statement()?
            }
            TokenKind::For => {
                self.pos += 1;
                self.for_statement()?
            }
            TokenKind::Ident(name) if self.is_assignment() => {
                self.pos += 1;
                self.assignment(name)?
            }
            _ => StmtKind::Expr(self.expression(0)?),
        };
        let end = self.previous().span.end;
        Ok(Stmt {
            kind,
            span: Span::new(start, end),
        })
    }

    fn let_statement(&mut self, mutable: bool) -> PResult<StmtKind> {
        let (name, _) = self.identifier("expected local name")?;
        self.expect(&TokenKind::Eq, "expected `=` in declaration")?;
        let value = self.expression(0)?;
        Ok(StmtKind::Let {
            mutable,
            name,
            value,
        })
    }

    fn return_statement(&mut self) -> PResult<StmtKind> {
        if self.is_separator() || self.at(&TokenKind::RBrace) {
            Ok(StmtKind::Return(None))
        } else {
            Ok(StmtKind::Return(Some(self.expression(0)?)))
        }
    }

    fn if_statement(&mut self) -> PResult<StmtKind> {
        let condition = self.expression(0)?;
        self.separators();
        let (then_block, _) = self.block()?;
        let saved = self.pos;
        self.separators();
        let else_block = if self.take(&TokenKind::Else) {
            self.separators();
            if self.take(&TokenKind::If) {
                let start = self.previous().span.start;
                let nested = self.if_statement()?;
                vec![Stmt {
                    kind: nested,
                    span: Span::new(start, self.previous().span.end),
                }]
            } else {
                self.block()?.0
            }
        } else {
            self.pos = saved;
            Vec::new()
        };
        Ok(StmtKind::If {
            condition,
            then_block,
            else_block,
        })
    }

    fn for_statement(&mut self) -> PResult<StmtKind> {
        let (name, _) = self.identifier("expected loop variable")?;
        self.expect(&TokenKind::In, "expected `in` after loop variable")?;
        let start = self.expression(2)?;
        self.expect(&TokenKind::DotDot, "expected `..` in range")?;
        let end = self.expression(0)?;
        self.separators();
        let body = self.block()?.0;
        Ok(StmtKind::For {
            name,
            start,
            end,
            body,
        })
    }

    fn is_assignment(&self) -> bool {
        self.tokens.get(self.pos + 1).is_some_and(|token| {
            matches!(
                token.kind,
                TokenKind::Eq | TokenKind::PlusEq | TokenKind::MinusEq
            )
        })
    }

    fn assignment(&mut self, name: String) -> PResult<StmtKind> {
        let op = match self.current().kind {
            TokenKind::Eq => AssignOp::Set,
            TokenKind::PlusEq => AssignOp::Add,
            TokenKind::MinusEq => AssignOp::Sub,
            _ => unreachable!("checked assignment operator"),
        };
        self.pos += 1;
        let value = self.expression(0)?;
        Ok(StmtKind::Assign { name, op, value })
    }

    fn expression(&mut self, min_binding: u8) -> PResult<Expr> {
        let mut left = self.prefix()?;
        loop {
            let Some((left_bp, right_bp, op)) = infix_binding(&self.current().kind) else {
                break;
            };
            if left_bp < min_binding {
                break;
            }
            self.pos += 1;
            let right = self.expression(right_bp)?;
            let span = Span::new(left.span.start, right.span.end);
            left = Expr {
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }
        Ok(left)
    }

    fn prefix(&mut self) -> PResult<Expr> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Int(value) => {
                self.pos += 1;
                Ok(Expr {
                    kind: ExprKind::Int(value),
                    span: token.span,
                })
            }
            TokenKind::True | TokenKind::False => {
                self.pos += 1;
                Ok(Expr {
                    kind: ExprKind::Bool(matches!(token.kind, TokenKind::True)),
                    span: token.span,
                })
            }
            TokenKind::String(value) => {
                self.pos += 1;
                Ok(Expr {
                    kind: ExprKind::String(value),
                    span: token.span,
                })
            }
            TokenKind::Ident(name) => self.name_or_call(name, token.span),
            TokenKind::Minus | TokenKind::Bang => {
                self.pos += 1;
                let op = if matches!(token.kind, TokenKind::Minus) {
                    UnaryOp::Neg
                } else {
                    UnaryOp::Not
                };
                let expr = self.expression(13)?;
                let span = Span::new(token.span.start, expr.span.end);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op,
                        expr: Box::new(expr),
                    },
                    span,
                })
            }
            TokenKind::LParen => {
                self.pos += 1;
                let mut expr = self.expression(0)?;
                let end = self.expect(&TokenKind::RParen, "expected `)`")?.span.end;
                expr.span = Span::new(token.span.start, end);
                Ok(expr)
            }
            _ => Err(self.error(
                token.span,
                format!("expected expression, found {}", token.kind.description()),
            )),
        }
    }

    fn name_or_call(&mut self, name: String, start_span: Span) -> PResult<Expr> {
        self.pos += 1;
        if !self.take(&TokenKind::LParen) {
            return Ok(Expr {
                kind: ExprKind::Name(name),
                span: start_span,
            });
        }
        let mut args = Vec::new();
        if !self.at(&TokenKind::RParen) {
            loop {
                args.push(self.expression(0)?);
                if !self.take(&TokenKind::Comma) {
                    break;
                }
            }
        }
        let end = self
            .expect(&TokenKind::RParen, "expected `)` after arguments")?
            .span
            .end;
        Ok(Expr {
            kind: ExprKind::Call { name, args },
            span: Span::new(start_span.start, end),
        })
    }

    fn identifier(&mut self, message: &str) -> PResult<(String, Span)> {
        let token = self.current().clone();
        if let TokenKind::Ident(name) = token.kind {
            self.pos += 1;
            Ok((name, token.span))
        } else {
            Err(self.error(token.span, message))
        }
    }

    fn separators(&mut self) {
        while self.is_separator() {
            self.pos += 1;
        }
    }

    fn is_separator(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Semicolon
        )
    }

    fn at(&self, kind: &TokenKind) -> bool {
        same_kind(&self.current().kind, kind)
    }

    fn take(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind, message: &str) -> PResult<Token> {
        if self.at(kind) {
            let token = self.current().clone();
            self.pos += 1;
            Ok(token)
        } else {
            Err(self.error(self.current().span, message))
        }
    }

    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }
    fn previous(&self) -> &Token {
        &self.tokens[self.pos.saturating_sub(1)]
    }
    fn error(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(self.source_name, self.source, span, message)
    }
}

fn same_kind(left: &TokenKind, right: &TokenKind) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

fn infix_binding(kind: &TokenKind) -> Option<(u8, u8, BinaryOp)> {
    let (precedence, op) = match kind {
        TokenKind::OrOr => (1, BinaryOp::Or),
        TokenKind::AndAnd => (3, BinaryOp::And),
        TokenKind::EqEq => (5, BinaryOp::Eq),
        TokenKind::BangEq => (5, BinaryOp::Ne),
        TokenKind::Lt => (7, BinaryOp::Lt),
        TokenKind::LtEq => (7, BinaryOp::Le),
        TokenKind::Gt => (7, BinaryOp::Gt),
        TokenKind::GtEq => (7, BinaryOp::Ge),
        TokenKind::Plus => (9, BinaryOp::Add),
        TokenKind::Minus => (9, BinaryOp::Sub),
        TokenKind::Star => (11, BinaryOp::Mul),
        TokenKind::Slash => (11, BinaryOp::Div),
        TokenKind::Percent => (11, BinaryOp::Rem),
        _ => return None,
    };
    Some((precedence, precedence + 1, op))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer;

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        let source = "fn main() { let x = 1 + 2 * 3 }";
        let program = parse("test", source, lexer::lex("test", source).unwrap()).unwrap();
        let StmtKind::Let { value, .. } = &program.functions[0].body[0].kind else {
            panic!()
        };
        let ExprKind::Binary { op, right, .. } = &value.kind else {
            panic!()
        };
        assert_eq!(*op, BinaryOp::Add);
        assert!(matches!(
            right.kind,
            ExprKind::Binary {
                op: BinaryOp::Mul,
                ..
            }
        ));
    }
}
