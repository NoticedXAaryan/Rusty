use crate::hir::{self, BinaryOp, Expr, ExprKind, Stmt, Type, UnaryOp};
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct Program {
    pub functions: Vec<hir::Function>,
}

impl Program {
    pub fn from_hir(program: &hir::Program) -> Self {
        Self {
            functions: program.functions.clone(),
        }
    }

    pub fn serialize(&self) -> String {
        let mut out = String::from("(rir v0\n");
        for function in &self.functions {
            serialize_function(&mut out, function);
        }
        out.push_str(")\n");
        out
    }
}

fn serialize_function(out: &mut String, function: &hir::Function) {
    let _ = write!(
        out,
        "  (fn {} @{} (params",
        quoted(&function.name),
        function.id
    );
    for param in &function.params {
        let _ = write!(
            out,
            " (local @{} {} {} {})",
            param.id,
            quoted(&param.name),
            type_text(param.ty),
            param.mutable
        );
    }
    let _ = writeln!(out, ") (result {})", type_text(function.result));
    out.push_str("    (locals");
    for local in function.locals.iter().skip(function.params.len()) {
        let _ = write!(
            out,
            " (local @{} {} {} {})",
            local.id,
            quoted(&local.name),
            type_text(local.ty),
            local.mutable
        );
    }
    out.push_str(")\n    (body\n");
    serialize_block(out, &function.body, 6);
    out.push_str("    ))\n  )\n");
}

fn serialize_block(out: &mut String, block: &[Stmt], indent: usize) {
    for statement in block {
        out.push_str(&" ".repeat(indent));
        serialize_statement(out, statement, indent);
        out.push('\n');
    }
}

fn serialize_statement(out: &mut String, statement: &Stmt, indent: usize) {
    match statement {
        Stmt::Let { local, value } => {
            let _ = write!(out, "(let @{} ", local);
            serialize_expr(out, value);
            out.push(')');
        }
        Stmt::Assign { local, op, value } => {
            let op = match op {
                hir::AssignOp::Set => "set",
                hir::AssignOp::Add => "add-set",
                hir::AssignOp::Sub => "sub-set",
            };
            let _ = write!(out, "({op} @{local} ");
            serialize_expr(out, value);
            out.push(')');
        }
        Stmt::Return(value) => {
            out.push_str("(return");
            if let Some(value) = value {
                out.push(' ');
                serialize_expr(out, value);
            }
            out.push(')');
        }
        Stmt::Expr(value) => {
            out.push_str("(expr ");
            serialize_expr(out, value);
            out.push(')');
        }
        Stmt::If {
            condition,
            then_block,
            else_block,
        } => {
            out.push_str("(if ");
            serialize_expr(out, condition);
            out.push('\n');
            out.push_str(&" ".repeat(indent + 2));
            out.push_str("(then\n");
            serialize_block(out, then_block, indent + 4);
            out.push_str(&" ".repeat(indent + 2));
            out.push_str(")\n");
            out.push_str(&" ".repeat(indent + 2));
            out.push_str("(else\n");
            serialize_block(out, else_block, indent + 4);
            out.push_str(&" ".repeat(indent + 2));
            out.push_str("))");
        }
        Stmt::For {
            local,
            start,
            end,
            body,
        } => {
            let _ = write!(out, "(for @{} ", local);
            serialize_expr(out, start);
            out.push(' ');
            serialize_expr(out, end);
            out.push('\n');
            serialize_block(out, body, indent + 2);
            out.push_str(&" ".repeat(indent));
            out.push(')');
        }
    }
}

fn serialize_expr(out: &mut String, expression: &Expr) {
    let _ = write!(out, "({} ", type_text(expression.ty));
    match &expression.kind {
        ExprKind::Int(value) => {
            let _ = write!(out, "int {value}");
        }
        ExprKind::Bool(value) => {
            let _ = write!(out, "bool {value}");
        }
        ExprKind::String(value) => {
            let _ = write!(out, "string {}", quoted(value));
        }
        ExprKind::Local(local) => {
            let _ = write!(out, "local @{local}");
        }
        ExprKind::Unary { op, expr } => {
            let op = match op {
                UnaryOp::Neg => "neg",
                UnaryOp::Not => "not",
            };
            let _ = write!(out, "{op} ");
            serialize_expr(out, expr);
        }
        ExprKind::Binary { op, left, right } => {
            let _ = write!(out, "{} ", binary_text(*op));
            serialize_expr(out, left);
            out.push(' ');
            serialize_expr(out, right);
        }
        ExprKind::Call {
            function,
            name,
            args,
        } => {
            let _ = write!(out, "call @{} {}", function, quoted(name));
            for arg in args {
                out.push(' ');
                serialize_expr(out, arg);
            }
        }
        ExprKind::Print(args) => {
            out.push_str("print");
            for arg in args {
                out.push(' ');
                serialize_expr(out, arg);
            }
        }
        ExprKind::RawDiv { left, right } => {
            out.push_str("div-raw ");
            serialize_expr(out, left);
            out.push(' ');
            serialize_expr(out, right);
        }
        ExprKind::RawRem { left, right } => {
            out.push_str("rem-raw ");
            serialize_expr(out, left);
            out.push(' ');
            serialize_expr(out, right);
        }
    }
    out.push(')');
}

fn binary_text(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Or => "or",
        BinaryOp::And => "and",
        BinaryOp::Eq => "eq",
        BinaryOp::Ne => "ne",
        BinaryOp::Lt => "lt",
        BinaryOp::Le => "le",
        BinaryOp::Gt => "gt",
        BinaryOp::Ge => "ge",
        BinaryOp::Add => "add-wrap",
        BinaryOp::Sub => "sub-wrap",
        BinaryOp::Mul => "mul-wrap",
        BinaryOp::Div => "div-guarded",
        BinaryOp::Rem => "rem-guarded",
    }
}

pub fn type_text(ty: Type) -> &'static str {
    match ty {
        Type::Int => "int",
        Type::Bool => "bool",
        Type::String => "string",
        Type::Unit => "unit",
    }
}

fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use crate::{analyze, rir::Program};

    #[test]
    fn serialization_is_canonical() {
        let hir = analyze("test", "fn main() { let x = 1\nprint(x) }").unwrap();
        let first = Program::from_hir(&hir).serialize();
        let second = Program::from_hir(&hir).serialize();
        assert_eq!(first, second);
        assert!(first.starts_with("(rir v0\n"));
        assert!(first.contains("(local @0 \"x\" int false)"));
    }
}
