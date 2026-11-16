use crate::hir::{self, BinaryOp, Expr, ExprKind, Stmt, Type, UnaryOp};
use crate::rir::Program;
use std::fmt::Write;

pub fn emit(program: &Program) -> String {
    let strings = collect_strings(program);
    let mut out = String::new();
    out.push_str("; Rusty trusted reference backend\nsource_filename = \"rusty-v0\"\n\n");
    out.push_str("@.fmt.int = private unnamed_addr constant [5 x i8] c\"%lld\\00\"\n");
    out.push_str("@.fmt.str = private unnamed_addr constant [3 x i8] c\"%s\\00\"\n");
    out.push_str("@.space = private unnamed_addr constant [2 x i8] c\" \\00\"\n");
    out.push_str("@.newline = private unnamed_addr constant [2 x i8] c\"\\0A\\00\"\n");
    out.push_str("@.true = private unnamed_addr constant [5 x i8] c\"true\\00\"\n");
    out.push_str("@.false = private unnamed_addr constant [6 x i8] c\"false\\00\"\n");
    for (id, value) in strings.iter().enumerate() {
        let escaped = llvm_bytes(value.as_bytes());
        let _ = writeln!(
            out,
            "@.str.{id} = private unnamed_addr constant [{} x i8] c\"{}\\00\"",
            value.len() + 1,
            escaped
        );
    }
    out.push_str(
        "\ndeclare i32 @printf(ptr, ...)\ndeclare void @llvm.trap() cold noreturn nounwind\n\n",
    );
    emit_guarded_arithmetic(&mut out);
    for function in &program.functions {
        FunctionEmitter::new(&mut out, program, function, &strings).emit();
    }
    emit_main_wrapper(&mut out, program);
    out
}

fn emit_guarded_arithmetic(out: &mut String) {
    out.push_str("define internal i64 @rusty.div(i64 %a, i64 %b) {\nentry:\n");
    out.push_str("  %zero = icmp eq i64 %b, 0\n  %min = icmp eq i64 %a, -9223372036854775808\n");
    out.push_str("  %negone = icmp eq i64 %b, -1\n  %overflow = and i1 %min, %negone\n  %bad = or i1 %zero, %overflow\n");
    out.push_str("  br i1 %bad, label %trap, label %ok\ntrap:\n  call void @llvm.trap()\n  unreachable\nok:\n  %value = sdiv i64 %a, %b\n  ret i64 %value\n}\n\n");
    out.push_str("define internal i64 @rusty.rem(i64 %a, i64 %b) {\nentry:\n");
    out.push_str("  %zero = icmp eq i64 %b, 0\n  %min = icmp eq i64 %a, -9223372036854775808\n");
    out.push_str("  %negone = icmp eq i64 %b, -1\n  %overflow = and i1 %min, %negone\n  %bad = or i1 %zero, %overflow\n");
    out.push_str("  br i1 %bad, label %trap, label %ok\ntrap:\n  call void @llvm.trap()\n  unreachable\nok:\n  %value = srem i64 %a, %b\n  ret i64 %value\n}\n\n");
}

fn emit_main_wrapper(out: &mut String, program: &Program) {
    let main = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("checked main");
    out.push_str("define i32 @main() {\nentry:\n");
    match main.result {
        Type::Unit => {
            let _ = writeln!(out, "  call void @rusty.fn.{}()\n  ret i32 0", main.id);
        }
        Type::Int => {
            let _ = writeln!(
                out,
                "  %result = call i64 @rusty.fn.{}()\n  %code = trunc i64 %result to i32\n  ret i32 %code",
                main.id
            );
        }
        _ => unreachable!("main result checked"),
    }
    out.push_str("}\n");
}

fn collect_strings(program: &Program) -> Vec<String> {
    let mut strings = Vec::new();
    for function in &program.functions {
        collect_block(&function.body, &mut strings);
    }
    strings
}

fn collect_block(block: &[Stmt], strings: &mut Vec<String>) {
    for statement in block {
        match statement {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } | Stmt::Expr(value) => {
                collect_expr(value, strings)
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    collect_expr(value, strings);
                }
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
            } => {
                collect_expr(condition, strings);
                collect_block(then_block, strings);
                collect_block(else_block, strings);
            }
            Stmt::For {
                start, end, body, ..
            } => {
                collect_expr(start, strings);
                collect_expr(end, strings);
                collect_block(body, strings);
            }
        }
    }
}

fn collect_expr(expression: &Expr, strings: &mut Vec<String>) {
    match &expression.kind {
        ExprKind::String(value) => {
            if !strings.contains(value) {
                strings.push(value.clone());
            }
        }
        ExprKind::Unary { expr, .. } => collect_expr(expr, strings),
        ExprKind::Binary { left, right, .. } => {
            collect_expr(left, strings);
            collect_expr(right, strings);
        }
        ExprKind::Call { args, .. } | ExprKind::Print(args) => {
            for arg in args {
                collect_expr(arg, strings);
            }
        }
        ExprKind::RawDiv { left, right } | ExprKind::RawRem { left, right } => {
            collect_expr(left, strings);
            collect_expr(right, strings);
        }
        _ => {}
    }
}

struct FunctionEmitter<'a> {
    out: &'a mut String,
    program: &'a Program,
    function: &'a hir::Function,
    strings: &'a [String],
    temp: usize,
    label: usize,
    current_label: String,
}

impl<'a> FunctionEmitter<'a> {
    fn new(
        out: &'a mut String,
        program: &'a Program,
        function: &'a hir::Function,
        strings: &'a [String],
    ) -> Self {
        Self {
            out,
            program,
            function,
            strings,
            temp: 0,
            label: 0,
            current_label: "entry".to_owned(),
        }
    }

    fn emit(mut self) {
        let _ = write!(
            self.out,
            "define {} @rusty.fn.{}(",
            llvm_type(self.function.result),
            self.function.id
        );
        for (index, param) in self.function.params.iter().enumerate() {
            if index > 0 {
                self.out.push_str(", ");
            }
            let _ = write!(self.out, "{} %p{}", llvm_type(param.ty), index);
        }
        self.out.push_str(") {\nentry:\n");
        for local in &self.function.locals {
            let _ = writeln!(
                self.out,
                "  %local.{} = alloca {}",
                local.id,
                llvm_type(local.ty)
            );
        }
        for (index, param) in self.function.params.iter().enumerate() {
            let _ = writeln!(
                self.out,
                "  store {} %p{}, ptr %local.{}",
                llvm_type(param.ty),
                index,
                param.id
            );
        }
        let terminated = self.block(&self.function.body);
        if !terminated {
            match self.function.result {
                Type::Unit => self.out.push_str("  ret void\n"),
                _ => self.out.push_str("  unreachable\n"),
            }
        }
        self.out.push_str("}\n\n");
    }

    fn block(&mut self, block: &[Stmt]) -> bool {
        let mut terminated = false;
        for statement in block {
            if terminated {
                break;
            }
            terminated = self.statement(statement);
        }
        terminated
    }

    fn statement(&mut self, statement: &Stmt) -> bool {
        match statement {
            Stmt::Let { local, value } => {
                let value = self.expression(value);
                let _ = writeln!(
                    self.out,
                    "  store {} {}, ptr %local.{}",
                    llvm_type(value.ty),
                    value.repr,
                    local
                );
                false
            }
            Stmt::Assign { local, op, value } => {
                let right = self.expression(value);
                let repr = match op {
                    hir::AssignOp::Set => right.repr,
                    hir::AssignOp::Add | hir::AssignOp::Sub => {
                        let old = self.temp();
                        let _ = writeln!(self.out, "  {old} = load i64, ptr %local.{local}");
                        let result = self.temp();
                        let opcode = if matches!(op, hir::AssignOp::Add) {
                            "add"
                        } else {
                            "sub"
                        };
                        let _ =
                            writeln!(self.out, "  {result} = {opcode} i64 {old}, {}", right.repr);
                        result
                    }
                };
                let _ = writeln!(
                    self.out,
                    "  store {} {}, ptr %local.{}",
                    llvm_type(right.ty),
                    repr,
                    local
                );
                false
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    let value = self.expression(value);
                    let _ = writeln!(self.out, "  ret {} {}", llvm_type(value.ty), value.repr);
                } else {
                    self.out.push_str("  ret void\n");
                }
                true
            }
            Stmt::Expr(value) => {
                self.expression(value);
                false
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
            } => self.if_statement(condition, then_block, else_block),
            Stmt::For {
                local,
                start,
                end,
                body,
            } => {
                self.for_statement(*local, start, end, body);
                false
            }
        }
    }

    fn if_statement(&mut self, condition: &Expr, then_block: &[Stmt], else_block: &[Stmt]) -> bool {
        let condition = self.expression(condition);
        let then_label = self.new_label("if.then");
        let else_label = self.new_label("if.else");
        let end_label = self.new_label("if.end");
        let _ = writeln!(
            self.out,
            "  br i1 {}, label %{}, label %{}",
            condition.repr, then_label, else_label
        );
        self.start_label(&then_label);
        let then_terminated = self.block(then_block);
        if !then_terminated {
            let _ = writeln!(self.out, "  br label %{end_label}");
        }
        self.start_label(&else_label);
        let else_terminated = self.block(else_block);
        if !else_terminated {
            let _ = writeln!(self.out, "  br label %{end_label}");
        }
        if then_terminated && else_terminated {
            true
        } else {
            self.start_label(&end_label);
            false
        }
    }

    fn for_statement(&mut self, local: usize, start: &Expr, end: &Expr, body: &[Stmt]) {
        let start = self.expression(start);
        let end = self.expression(end);
        let _ = writeln!(self.out, "  store i64 {}, ptr %local.{}", start.repr, local);
        let cond_label = self.new_label("for.cond");
        let body_label = self.new_label("for.body");
        let end_label = self.new_label("for.end");
        let _ = writeln!(self.out, "  br label %{cond_label}");
        self.start_label(&cond_label);
        let current = self.temp();
        let _ = writeln!(self.out, "  {current} = load i64, ptr %local.{local}");
        let condition = self.temp();
        let _ = writeln!(
            self.out,
            "  {condition} = icmp slt i64 {current}, {}",
            end.repr
        );
        let _ = writeln!(
            self.out,
            "  br i1 {condition}, label %{body_label}, label %{end_label}"
        );
        self.start_label(&body_label);
        let body_terminated = self.block(body);
        if !body_terminated {
            let before = self.temp();
            let after = self.temp();
            let _ = writeln!(self.out, "  {before} = load i64, ptr %local.{local}");
            let _ = writeln!(self.out, "  {after} = add i64 {before}, 1");
            let _ = writeln!(
                self.out,
                "  store i64 {after}, ptr %local.{local}\n  br label %{cond_label}"
            );
        }
        self.start_label(&end_label);
    }

    fn expression(&mut self, expression: &Expr) -> Value {
        match &expression.kind {
            ExprKind::Int(value) => Value::new(value.to_string(), Type::Int),
            ExprKind::Bool(value) => Value::new(if *value { "true" } else { "false" }, Type::Bool),
            ExprKind::String(value) => {
                let id = self
                    .strings
                    .iter()
                    .position(|item| item == value)
                    .expect("collected string");
                Value::new(format!("@.str.{id}"), Type::String)
            }
            ExprKind::Local(local) => {
                let result = self.temp();
                let _ = writeln!(
                    self.out,
                    "  {result} = load {}, ptr %local.{local}",
                    llvm_type(expression.ty)
                );
                Value::new(result, expression.ty)
            }
            ExprKind::Unary { op, expr } => {
                let value = self.expression(expr);
                let result = self.temp();
                match op {
                    UnaryOp::Neg => {
                        let _ = writeln!(self.out, "  {result} = sub i64 0, {}", value.repr);
                    }
                    UnaryOp::Not => {
                        let _ = writeln!(self.out, "  {result} = xor i1 {}, true", value.repr);
                    }
                }
                Value::new(result, expression.ty)
            }
            ExprKind::Binary {
                op: BinaryOp::And,
                left,
                right,
            } => self.short_circuit(false, left, right),
            ExprKind::Binary {
                op: BinaryOp::Or,
                left,
                right,
            } => self.short_circuit(true, left, right),
            ExprKind::Binary { op, left, right } => self.binary(*op, left, right, expression.ty),
            ExprKind::RawDiv { left, right } | ExprKind::RawRem { left, right } => {
                let dividend = self.expression(left);
                let divisor = self.expression(right);
                let result = self.temp();
                let opcode = if matches!(expression.kind, ExprKind::RawDiv { .. }) {
                    "sdiv"
                } else {
                    "srem"
                };
                let _ = writeln!(
                    self.out,
                    "  {result} = {opcode} i64 {}, {}",
                    dividend.repr, divisor.repr
                );
                Value::new(result, expression.ty)
            }
            ExprKind::Call { function, args, .. } => {
                let function_info = &self.program.functions[*function];
                let args = args
                    .iter()
                    .map(|arg| self.expression(arg))
                    .collect::<Vec<_>>();
                let rendered = args
                    .iter()
                    .map(|arg| format!("{} {}", llvm_type(arg.ty), arg.repr))
                    .collect::<Vec<_>>()
                    .join(", ");
                if expression.ty == Type::Unit {
                    let _ = writeln!(self.out, "  call void @rusty.fn.{function}({rendered})");
                    Value::new(String::new(), Type::Unit)
                } else {
                    let result = self.temp();
                    let _ = writeln!(
                        self.out,
                        "  {result} = call {} @rusty.fn.{}({rendered})",
                        llvm_type(function_info.result),
                        function_info.id
                    );
                    Value::new(result, expression.ty)
                }
            }
            ExprKind::Print(args) => {
                self.print(args);
                Value::new(String::new(), Type::Unit)
            }
        }
    }

    fn binary(&mut self, op: BinaryOp, left: &Expr, right: &Expr, ty: Type) -> Value {
        let left = self.expression(left);
        let right = self.expression(right);
        let result = self.temp();
        let instruction = match op {
            BinaryOp::Add => format!("add i64 {}, {}", left.repr, right.repr),
            BinaryOp::Sub => format!("sub i64 {}, {}", left.repr, right.repr),
            BinaryOp::Mul => format!("mul i64 {}, {}", left.repr, right.repr),
            BinaryOp::Div => format!("call i64 @rusty.div(i64 {}, i64 {})", left.repr, right.repr),
            BinaryOp::Rem => format!("call i64 @rusty.rem(i64 {}, i64 {})", left.repr, right.repr),
            BinaryOp::Eq | BinaryOp::Ne => {
                let predicate = if matches!(op, BinaryOp::Eq) {
                    "eq"
                } else {
                    "ne"
                };
                format!(
                    "icmp {predicate} {} {}, {}",
                    llvm_type(left.ty),
                    left.repr,
                    right.repr
                )
            }
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                let predicate = match op {
                    BinaryOp::Lt => "slt",
                    BinaryOp::Le => "sle",
                    BinaryOp::Gt => "sgt",
                    _ => "sge",
                };
                format!("icmp {predicate} i64 {}, {}", left.repr, right.repr)
            }
            BinaryOp::And | BinaryOp::Or => unreachable!("short circuit handled separately"),
        };
        let _ = writeln!(self.out, "  {result} = {instruction}");
        Value::new(result, ty)
    }

    fn short_circuit(&mut self, short_value: bool, left: &Expr, right: &Expr) -> Value {
        let left = self.expression(left);
        let rhs_label = self.new_label("logic.rhs");
        let short_label = self.new_label("logic.short");
        let merge_label = self.new_label("logic.end");
        if short_value {
            let _ = writeln!(
                self.out,
                "  br i1 {}, label %{}, label %{}",
                left.repr, short_label, rhs_label
            );
        } else {
            let _ = writeln!(
                self.out,
                "  br i1 {}, label %{}, label %{}",
                left.repr, rhs_label, short_label
            );
        }
        self.start_label(&rhs_label);
        let right = self.expression(right);
        let right_predecessor = self.current_label.clone();
        let _ = writeln!(self.out, "  br label %{merge_label}");
        self.start_label(&short_label);
        let _ = writeln!(self.out, "  br label %{merge_label}");
        self.start_label(&merge_label);
        let result = self.temp();
        let _ = writeln!(
            self.out,
            "  {result} = phi i1 [{short_value}, %{short_label}], [{}, %{right_predecessor}]",
            right.repr
        );
        Value::new(result, Type::Bool)
    }

    fn print(&mut self, args: &[Expr]) {
        // Evaluate every argument left-to-right before producing output so nested
        // calls cannot interleave their side effects with this print operation.
        let values = args
            .iter()
            .map(|argument| self.expression(argument))
            .collect::<Vec<_>>();
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                self.out
                    .push_str("  call i32 (ptr, ...) @printf(ptr @.fmt.str, ptr @.space)\n");
            }
            match value.ty {
                Type::Int => {
                    let _ = writeln!(
                        self.out,
                        "  call i32 (ptr, ...) @printf(ptr @.fmt.int, i64 {})",
                        value.repr
                    );
                }
                Type::String => {
                    let _ = writeln!(
                        self.out,
                        "  call i32 (ptr, ...) @printf(ptr @.fmt.str, ptr {})",
                        value.repr
                    );
                }
                Type::Bool => {
                    let selected = self.temp();
                    let _ = writeln!(
                        self.out,
                        "  {selected} = select i1 {}, ptr @.true, ptr @.false",
                        value.repr
                    );
                    let _ = writeln!(
                        self.out,
                        "  call i32 (ptr, ...) @printf(ptr @.fmt.str, ptr {selected})"
                    );
                }
                Type::Unit => unreachable!("checked print argument"),
            }
        }
        self.out
            .push_str("  call i32 (ptr, ...) @printf(ptr @.fmt.str, ptr @.newline)\n");
    }

    fn temp(&mut self) -> String {
        let value = format!("%t{}", self.temp);
        self.temp += 1;
        value
    }

    fn new_label(&mut self, prefix: &str) -> String {
        let value = format!("{prefix}.{}", self.label);
        self.label += 1;
        value
    }

    fn start_label(&mut self, label: &str) {
        let _ = writeln!(self.out, "{label}:");
        self.current_label = label.to_owned();
    }
}

struct Value {
    repr: String,
    ty: Type,
}

impl Value {
    fn new(repr: impl Into<String>, ty: Type) -> Self {
        Self {
            repr: repr.into(),
            ty,
        }
    }
}

fn llvm_type(ty: Type) -> &'static str {
    match ty {
        Type::Int => "i64",
        Type::Bool => "i1",
        Type::String => "ptr",
        Type::Unit => "void",
    }
}

fn llvm_bytes(bytes: &[u8]) -> String {
    let mut output = String::new();
    for byte in bytes {
        match byte {
            b' '..=b'~' if !matches!(byte, b'"' | b'\\') => output.push(char::from(*byte)),
            _ => {
                let _ = write!(output, "\\{byte:02X}");
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use crate::{analyze, llvm, rir::Program};

    #[test]
    fn emits_guarded_division_and_wrapping_add() {
        let hir = analyze(
            "test",
            "fn f(x: int) -> int { return (x + 1) / 2 }\nfn main() { print(f(3)) }",
        )
        .unwrap();
        let output = llvm::emit(&Program::from_hir(&hir));
        assert!(output.contains(" = add i64 "));
        assert!(output.contains("call i64 @rusty.div"));
        assert!(output.contains("@llvm.trap"));
    }
}
