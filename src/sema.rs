use crate::ast;
use crate::diagnostic::Diagnostic;
use crate::hir::{self, BinaryOp, Expr, ExprKind, Local, Stmt, Type, UnaryOp};
use crate::token::Span;
use std::collections::BTreeMap;

pub fn check(
    source_name: &str,
    source: &str,
    program: ast::Program,
) -> Result<hir::Program, Vec<Diagnostic>> {
    match Checker::new(source_name, source, &program).run(program) {
        Ok(program) => Ok(program),
        Err(error) => Err(vec![error]),
    }
}

#[derive(Clone)]
struct Signature {
    id: usize,
    params: Vec<Type>,
    result: Type,
}

struct Checker<'a> {
    source_name: &'a str,
    source: &'a str,
    signatures: BTreeMap<String, Signature>,
}

impl<'a> Checker<'a> {
    fn new(source_name: &'a str, source: &'a str, program: &ast::Program) -> Self {
        let mut signatures = BTreeMap::new();
        for (id, function) in program.functions.iter().enumerate() {
            signatures
                .entry(function.name.clone())
                .or_insert(Signature {
                    id,
                    params: function
                        .params
                        .iter()
                        .map(|param| lower_type(param.ty))
                        .collect(),
                    result: function.result.map_or(Type::Unit, lower_type),
                });
        }
        Self {
            source_name,
            source,
            signatures,
        }
    }

    fn run(&self, program: ast::Program) -> Result<hir::Program, Diagnostic> {
        self.validate_functions(&program)?;
        let mut functions = Vec::new();
        for (id, function) in program.functions.into_iter().enumerate() {
            functions.push(FunctionChecker::new(self, id, function).run()?);
        }
        Ok(hir::Program { functions })
    }

    fn validate_functions(&self, program: &ast::Program) -> Result<(), Diagnostic> {
        let mut names = BTreeMap::new();
        for function in &program.functions {
            if names
                .insert(function.name.as_str(), function.span)
                .is_some()
            {
                return Err(self.error(
                    function.span,
                    format!("duplicate function `{}`", function.name),
                ));
            }
            let mut params = BTreeMap::new();
            for param in &function.params {
                if params.insert(param.name.as_str(), param.span).is_some() {
                    return Err(
                        self.error(param.span, format!("duplicate parameter `{}`", param.name))
                    );
                }
            }
        }
        let Some(main) = program
            .functions
            .iter()
            .find(|function| function.name == "main")
        else {
            return Err(self.error(Span::default(), "program requires a `main` function"));
        };
        if !main.params.is_empty() || !matches!(main.result, None | Some(ast::TypeName::Int)) {
            return Err(self.error(
                main.span,
                "`main` must take no parameters and return nothing or `int`",
            ));
        }
        Ok(())
    }

    fn error(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(self.source_name, self.source, span, message)
    }
}

struct FunctionChecker<'a> {
    checker: &'a Checker<'a>,
    id: usize,
    function: ast::Function,
    result: Type,
    params: Vec<Local>,
    locals: Vec<Local>,
    scopes: Vec<BTreeMap<String, usize>>,
}

impl<'a> FunctionChecker<'a> {
    fn new(checker: &'a Checker<'a>, id: usize, function: ast::Function) -> Self {
        let result = function.result.map_or(Type::Unit, lower_type);
        Self {
            checker,
            id,
            function,
            result,
            params: Vec::new(),
            locals: Vec::new(),
            scopes: vec![BTreeMap::new()],
        }
    }

    fn run(mut self) -> Result<hir::Function, Diagnostic> {
        for param in self.function.params.clone() {
            let local = self.add_local(param.name, lower_type(param.ty), false, param.span)?;
            self.params.push(self.locals[local].clone());
        }
        let body_ast = std::mem::take(&mut self.function.body);
        let (body, returns) = self.block(body_ast, false)?;
        if self.result != Type::Unit && !returns {
            return Err(self.error(
                self.function.span,
                format!(
                    "function `{}` may exit without returning {}",
                    self.function.name,
                    type_name(self.result)
                ),
            ));
        }
        Ok(hir::Function {
            id: self.id,
            name: self.function.name,
            params: self.params,
            result: self.result,
            locals: self.locals,
            body,
        })
    }

    fn block(&mut self, block: ast::Block, scoped: bool) -> Result<(hir::Block, bool), Diagnostic> {
        if scoped {
            self.scopes.push(BTreeMap::new());
        }
        let mut output = Vec::new();
        let mut returns = false;
        for statement in block {
            let (lowered, statement_returns) = self.statement(statement)?;
            output.push(lowered);
            returns |= statement_returns;
        }
        if scoped {
            self.scopes.pop();
        }
        Ok((output, returns))
    }

    fn statement(&mut self, statement: ast::Stmt) -> Result<(Stmt, bool), Diagnostic> {
        let span = statement.span;
        match statement.kind {
            ast::StmtKind::Let {
                mutable,
                name,
                value,
            } => {
                let value = self.expression(value)?;
                if value.ty == Type::Unit {
                    return Err(self.error(span, "cannot bind a no-value expression"));
                }
                let local = self.add_local(name, value.ty, mutable, span)?;
                Ok((Stmt::Let { local, value }, false))
            }
            ast::StmtKind::Assign { name, op, value } => {
                let local = self.lookup(&name, span)?;
                let info = self.locals[local].clone();
                if !info.mutable {
                    return Err(self.error(span, format!("cannot assign to immutable `{name}`")));
                }
                if op != ast::AssignOp::Set && info.ty != Type::Int {
                    return Err(self.error(span, "compound assignment requires `int`"));
                }
                let value = self.expression(value)?;
                self.expect_type(value.ty, info.ty, span)?;
                Ok((
                    Stmt::Assign {
                        local,
                        op: lower_assign(op),
                        value,
                    },
                    false,
                ))
            }
            ast::StmtKind::Return(value) => {
                let value = value.map(|expr| self.expression(expr)).transpose()?;
                let actual = value.as_ref().map_or(Type::Unit, |expr| expr.ty);
                self.expect_type(actual, self.result, span)?;
                Ok((Stmt::Return(value), true))
            }
            ast::StmtKind::Expr(expr) => {
                let expr = self.expression(expr)?;
                if expr.ty != Type::Unit {
                    return Err(self.error(
                        span,
                        "unused value; only no-value calls are expression statements",
                    ));
                }
                Ok((Stmt::Expr(expr), false))
            }
            ast::StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                let condition = self.expression(condition)?;
                self.expect_type(condition.ty, Type::Bool, span)?;
                let (then_block, then_returns) = self.block(then_block, true)?;
                let has_else = !else_block.is_empty();
                let (else_block, else_returns) = self.block(else_block, true)?;
                Ok((
                    Stmt::If {
                        condition,
                        then_block,
                        else_block,
                    },
                    has_else && then_returns && else_returns,
                ))
            }
            ast::StmtKind::For {
                name,
                start,
                end,
                body,
            } => {
                let start = self.expression(start)?;
                let end = self.expression(end)?;
                self.expect_type(start.ty, Type::Int, span)?;
                self.expect_type(end.ty, Type::Int, span)?;
                self.scopes.push(BTreeMap::new());
                let local = self.add_local(name, Type::Int, false, span)?;
                let (body, _) = self.block(body, true)?;
                self.scopes.pop();
                Ok((
                    Stmt::For {
                        local,
                        start,
                        end,
                        body,
                    },
                    false,
                ))
            }
        }
    }

    fn expression(&mut self, expression: ast::Expr) -> Result<Expr, Diagnostic> {
        let span = expression.span;
        match expression.kind {
            ast::ExprKind::Int(value) => Ok(expr(ExprKind::Int(value), Type::Int)),
            ast::ExprKind::Bool(value) => Ok(expr(ExprKind::Bool(value), Type::Bool)),
            ast::ExprKind::String(value) => Ok(expr(ExprKind::String(value), Type::String)),
            ast::ExprKind::Name(name) => {
                let local = self.lookup(&name, span)?;
                Ok(expr(ExprKind::Local(local), self.locals[local].ty))
            }
            ast::ExprKind::Unary { op, expr: inner } => {
                let inner = self.expression(*inner)?;
                let (expected, op) = match op {
                    ast::UnaryOp::Neg => (Type::Int, UnaryOp::Neg),
                    ast::UnaryOp::Not => (Type::Bool, UnaryOp::Not),
                };
                self.expect_type(inner.ty, expected, span)?;
                Ok(expr(
                    ExprKind::Unary {
                        op,
                        expr: Box::new(inner),
                    },
                    expected,
                ))
            }
            ast::ExprKind::Binary { op, left, right } => self.binary(op, *left, *right, span),
            ast::ExprKind::Call { name, args } => self.call(name, args, span),
        }
    }

    fn binary(
        &mut self,
        op: ast::BinaryOp,
        left: ast::Expr,
        right: ast::Expr,
        span: Span,
    ) -> Result<Expr, Diagnostic> {
        let left = self.expression(left)?;
        let right = self.expression(right)?;
        let (operand, result) = match op {
            ast::BinaryOp::Or | ast::BinaryOp::And => (Type::Bool, Type::Bool),
            ast::BinaryOp::Eq | ast::BinaryOp::Ne => {
                if !matches!(left.ty, Type::Int | Type::Bool) {
                    return Err(self.error(span, "equality supports only `int` and `bool` in v0"));
                }
                (left.ty, Type::Bool)
            }
            ast::BinaryOp::Lt | ast::BinaryOp::Le | ast::BinaryOp::Gt | ast::BinaryOp::Ge => {
                (Type::Int, Type::Bool)
            }
            _ => (Type::Int, Type::Int),
        };
        self.expect_type(left.ty, operand, span)?;
        self.expect_type(right.ty, operand, span)?;
        Ok(expr(
            ExprKind::Binary {
                op: lower_binary(op),
                left: Box::new(left),
                right: Box::new(right),
            },
            result,
        ))
    }

    fn call(&mut self, name: String, args: Vec<ast::Expr>, span: Span) -> Result<Expr, Diagnostic> {
        let lowered = args
            .into_iter()
            .map(|arg| self.expression(arg))
            .collect::<Result<Vec<_>, _>>()?;
        if name == "print" {
            if lowered.iter().any(|arg| arg.ty == Type::Unit) {
                return Err(self.error(span, "cannot print a no-value expression"));
            }
            return Ok(expr(ExprKind::Print(lowered), Type::Unit));
        }
        let Some(signature) = self.checker.signatures.get(&name) else {
            return Err(self.error(span, format!("unknown function `{name}`")));
        };
        if lowered.len() != signature.params.len() {
            return Err(self.error(
                span,
                format!(
                    "function `{name}` expects {} arguments, found {}",
                    signature.params.len(),
                    lowered.len()
                ),
            ));
        }
        for (argument, expected) in lowered.iter().zip(&signature.params) {
            self.expect_type(argument.ty, *expected, span)?;
        }
        Ok(expr(
            ExprKind::Call {
                function: signature.id,
                name,
                args: lowered,
            },
            signature.result,
        ))
    }

    fn add_local(
        &mut self,
        name: String,
        ty: Type,
        mutable: bool,
        span: Span,
    ) -> Result<usize, Diagnostic> {
        if self
            .scopes
            .last()
            .is_some_and(|scope| scope.contains_key(&name))
        {
            return Err(self.error(span, format!("duplicate local `{name}` in this scope")));
        }
        let id = self.locals.len();
        self.locals.push(Local {
            id,
            name: name.clone(),
            ty,
            mutable,
        });
        self.scopes.last_mut().expect("scope").insert(name, id);
        Ok(id)
    }

    fn lookup(&self, name: &str, span: Span) -> Result<usize, Diagnostic> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| self.error(span, format!("unknown local `{name}`")))
    }

    fn expect_type(&self, actual: Type, expected: Type, span: Span) -> Result<(), Diagnostic> {
        if actual == expected {
            Ok(())
        } else {
            Err(self.error(
                span,
                format!(
                    "expected {}, found {}",
                    type_name(expected),
                    type_name(actual)
                ),
            ))
        }
    }

    fn error(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.checker.error(span, message)
    }
}

fn expr(kind: ExprKind, ty: Type) -> Expr {
    Expr { kind, ty }
}
fn lower_type(ty: ast::TypeName) -> Type {
    match ty {
        ast::TypeName::Int => Type::Int,
        ast::TypeName::Bool => Type::Bool,
        ast::TypeName::String => Type::String,
    }
}
fn lower_assign(op: ast::AssignOp) -> hir::AssignOp {
    match op {
        ast::AssignOp::Set => hir::AssignOp::Set,
        ast::AssignOp::Add => hir::AssignOp::Add,
        ast::AssignOp::Sub => hir::AssignOp::Sub,
    }
}
fn lower_binary(op: ast::BinaryOp) -> BinaryOp {
    match op {
        ast::BinaryOp::Or => BinaryOp::Or,
        ast::BinaryOp::And => BinaryOp::And,
        ast::BinaryOp::Eq => BinaryOp::Eq,
        ast::BinaryOp::Ne => BinaryOp::Ne,
        ast::BinaryOp::Lt => BinaryOp::Lt,
        ast::BinaryOp::Le => BinaryOp::Le,
        ast::BinaryOp::Gt => BinaryOp::Gt,
        ast::BinaryOp::Ge => BinaryOp::Ge,
        ast::BinaryOp::Add => BinaryOp::Add,
        ast::BinaryOp::Sub => BinaryOp::Sub,
        ast::BinaryOp::Mul => BinaryOp::Mul,
        ast::BinaryOp::Div => BinaryOp::Div,
        ast::BinaryOp::Rem => BinaryOp::Rem,
    }
}
fn type_name(ty: Type) -> &'static str {
    match ty {
        Type::Int => "`int`",
        Type::Bool => "`bool`",
        Type::String => "`string`",
        Type::Unit => "no value",
    }
}

#[cfg(test)]
mod tests {
    use crate::analyze;

    #[test]
    fn rejects_assignment_to_let() {
        let errors = analyze("test", "fn main() { let x = 1\nx += 1 }").unwrap_err();
        assert!(errors[0].message.contains("immutable"));
    }

    #[test]
    fn resolves_shadowed_locals_to_distinct_ids() {
        let hir = analyze(
            "test",
            "fn main() { let x = 1\nif true { let x = 2\nprint(x) }\nprint(x) }",
        )
        .unwrap();
        assert_eq!(hir.functions[0].locals.len(), 2);
    }
}
