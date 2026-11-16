//! In-process interpreter for canonical RIR.
//!
//! The interpreter is the fast equivalence oracle for rewrite candidates. It
//! implements the v0 rules: wrapping arithmetic, toward-zero division, traps
//! on a zero divisor and on `INT64_MIN / -1`, short-circuit logic, and
//! half-open ranges. It is not a substitute for the native differential check.

use crate::hir::{self, BinaryOp, Expr, ExprKind, Program, Stmt, Type, UnaryOp};

pub const DEFAULT_STEP_BUDGET: u64 = 100_000_000;
const MAX_CALL_DEPTH: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    String(String),
    Unit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Completion {
    Returned(Option<i64>),
    Trap,
    BudgetExceeded,
    DepthExceeded,
}

impl Completion {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Returned(_) => "returned",
            Self::Trap => "trap",
            Self::BudgetExceeded => "budget-exceeded",
            Self::DepthExceeded => "depth-exceeded",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    pub completion: Completion,
    pub steps: u64,
}

impl Outcome {
    pub fn equivalent(&self, other: &Self) -> bool {
        self.stdout == other.stdout && self.completion == other.completion
    }
}

pub fn execute(program: &Program, budget: u64) -> Outcome {
    let Some(main) = program
        .functions
        .iter()
        .find(|function| function.name == "main")
    else {
        return Outcome {
            stdout: Vec::new(),
            completion: Completion::Trap,
            steps: 0,
        };
    };
    let mut machine = Machine {
        program,
        budget,
        steps: 0,
        stdout: Vec::new(),
        depth: 0,
    };
    let completion = match machine.call(main.id, Vec::new()) {
        Ok(value) => match value {
            Some(Value::Int(result)) => Completion::Returned(Some(result)),
            None | Some(Value::Unit) => Completion::Returned(None),
            Some(_) => Completion::Trap,
        },
        Err(Stop::Trap) => Completion::Trap,
        Err(Stop::Budget) => Completion::BudgetExceeded,
        Err(Stop::Depth) => Completion::DepthExceeded,
    };
    Outcome {
        stdout: machine.stdout,
        completion,
        steps: machine.steps,
    }
}

enum Stop {
    Trap,
    Budget,
    Depth,
}

enum Flow {
    Next,
    Return(Option<Value>),
}

struct Machine<'a> {
    program: &'a Program,
    budget: u64,
    steps: u64,
    stdout: Vec<u8>,
    depth: usize,
}

impl<'a> Machine<'a> {
    fn tick(&mut self) -> Result<(), Stop> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > self.budget {
            Err(Stop::Budget)
        } else {
            Ok(())
        }
    }

    fn call(&mut self, function: usize, arguments: Vec<Value>) -> Result<Option<Value>, Stop> {
        if self.depth >= MAX_CALL_DEPTH {
            return Err(Stop::Depth);
        }
        self.depth += 1;
        let result = self.call_inner(function, arguments);
        self.depth -= 1;
        result
    }

    fn call_inner(
        &mut self,
        function: usize,
        arguments: Vec<Value>,
    ) -> Result<Option<Value>, Stop> {
        let signature = &self.program.functions[function];
        let mut slots = vec![None; signature.locals.len()];
        for (index, argument) in arguments.into_iter().enumerate() {
            slots[index] = Some(argument);
        }
        match self.block(&signature.body, &mut slots)? {
            Flow::Return(value) => Ok(value),
            Flow::Next => Ok(None),
        }
    }

    fn block(&mut self, block: &[Stmt], slots: &mut [Option<Value>]) -> Result<Flow, Stop> {
        for statement in block {
            match self.statement(statement, slots)? {
                Flow::Next => {}
                returned @ Flow::Return(_) => return Ok(returned),
            }
        }
        Ok(Flow::Next)
    }

    fn statement(&mut self, statement: &Stmt, slots: &mut [Option<Value>]) -> Result<Flow, Stop> {
        self.tick()?;
        match statement {
            Stmt::Let { local, value } => {
                slots[*local] = Some(self.expression(value, slots)?);
                Ok(Flow::Next)
            }
            Stmt::Assign { local, op, value } => {
                let right = self.expression(value, slots)?;
                let Value::Int(right) = right else {
                    return Err(Stop::Trap);
                };
                let updated = match op {
                    hir::AssignOp::Set => right,
                    hir::AssignOp::Add => self.int_slot(slots, *local)?.wrapping_add(right),
                    hir::AssignOp::Sub => self.int_slot(slots, *local)?.wrapping_sub(right),
                };
                slots[*local] = Some(Value::Int(updated));
                Ok(Flow::Next)
            }
            Stmt::Return(value) => {
                let value = value
                    .as_ref()
                    .map(|value| self.expression(value, slots))
                    .transpose()?;
                Ok(Flow::Return(value))
            }
            Stmt::Expr(value) => {
                self.expression(value, slots)?;
                Ok(Flow::Next)
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
            } => {
                let Value::Bool(condition) = self.expression(condition, slots)? else {
                    return Err(Stop::Trap);
                };
                if condition {
                    self.block(then_block, slots)
                } else {
                    self.block(else_block, slots)
                }
            }
            Stmt::For {
                local,
                start,
                end,
                body,
            } => {
                let Value::Int(mut index) = self.expression(start, slots)? else {
                    return Err(Stop::Trap);
                };
                let Value::Int(end) = self.expression(end, slots)? else {
                    return Err(Stop::Trap);
                };
                while index < end {
                    self.tick()?;
                    slots[*local] = Some(Value::Int(index));
                    if let Flow::Return(value) = self.block(body, slots)? {
                        return Ok(Flow::Return(value));
                    }
                    index = index.wrapping_add(1);
                }
                Ok(Flow::Next)
            }
        }
    }

    fn expression(&mut self, expression: &Expr, slots: &[Option<Value>]) -> Result<Value, Stop> {
        self.tick()?;
        match &expression.kind {
            ExprKind::Int(value) => Ok(Value::Int(*value)),
            ExprKind::Bool(value) => Ok(Value::Bool(*value)),
            ExprKind::String(value) => Ok(Value::String(value.clone())),
            ExprKind::Local(local) => slots[*local].clone().ok_or(Stop::Trap),
            ExprKind::Unary { op, expr } => {
                let value = self.expression(expr, slots)?;
                match (op, value) {
                    (UnaryOp::Neg, Value::Int(value)) => Ok(Value::Int(value.wrapping_neg())),
                    (UnaryOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                    _ => Err(Stop::Trap),
                }
            }
            ExprKind::Binary { op, left, right } => self.binary(*op, left, right, slots),
            ExprKind::Call { function, args, .. } => {
                let mut values = Vec::with_capacity(args.len());
                for argument in args {
                    values.push(self.expression(argument, slots)?);
                }
                let returned = self.call(*function, values)?;
                if self.program.functions[*function].result == Type::Unit {
                    Ok(Value::Unit)
                } else {
                    returned.ok_or(Stop::Trap)
                }
            }
            ExprKind::Print(args) => {
                let mut values = Vec::with_capacity(args.len());
                for argument in args {
                    values.push(self.expression(argument, slots)?);
                }
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        self.stdout.push(b' ');
                    }
                    match value {
                        Value::Int(value) => {
                            self.stdout.extend(value.to_string().into_bytes());
                        }
                        Value::Bool(true) => self.stdout.extend(b"true"),
                        Value::Bool(false) => self.stdout.extend(b"false"),
                        Value::String(value) => self.stdout.extend(value.as_bytes()),
                        Value::Unit => return Err(Stop::Trap),
                    }
                }
                self.stdout.push(b'\n');
                Ok(Value::Int(0))
            }
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        slots: &[Option<Value>],
    ) -> Result<Value, Stop> {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            let Value::Bool(left) = self.expression(left, slots)? else {
                return Err(Stop::Trap);
            };
            let short = op == BinaryOp::Or;
            if left == short {
                return Ok(Value::Bool(short));
            }
            return self.expression(right, slots);
        }
        let left = self.expression(left, slots)?;
        let right = self.expression(right, slots)?;
        match op {
            BinaryOp::Add => int2(left, right, i64::wrapping_add),
            BinaryOp::Sub => int2(left, right, i64::wrapping_sub),
            BinaryOp::Mul => int2(left, right, i64::wrapping_mul),
            BinaryOp::Div => div(left, right, false),
            BinaryOp::Rem => div(left, right, true),
            BinaryOp::Eq => Ok(Value::Bool(left == right)),
            BinaryOp::Ne => Ok(Value::Bool(left != right)),
            BinaryOp::Lt => cmp(left, right, |a, b| a < b),
            BinaryOp::Le => cmp(left, right, |a, b| a <= b),
            BinaryOp::Gt => cmp(left, right, |a, b| a > b),
            BinaryOp::Ge => cmp(left, right, |a, b| a >= b),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuit handled above"),
        }
    }

    fn int_slot(&self, slots: &[Option<Value>], local: usize) -> Result<i64, Stop> {
        match slots[local] {
            Some(Value::Int(value)) => Ok(value),
            _ => Err(Stop::Trap),
        }
    }
}

fn int2(left: Value, right: Value, op: impl Fn(i64, i64) -> i64) -> Result<Value, Stop> {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => Ok(Value::Int(op(left, right))),
        _ => Err(Stop::Trap),
    }
}

fn cmp(left: Value, right: Value, op: impl Fn(i64, i64) -> bool) -> Result<Value, Stop> {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => Ok(Value::Bool(op(left, right))),
        _ => Err(Stop::Trap),
    }
}

fn div(left: Value, right: Value, remainder: bool) -> Result<Value, Stop> {
    let (Value::Int(left), Value::Int(right)) = (left, right) else {
        return Err(Stop::Trap);
    };
    if right == 0 || (left == i64::MIN && right == -1) {
        return Err(Stop::Trap);
    }
    let value = if remainder {
        left.wrapping_rem(right)
    } else {
        left.wrapping_div(right)
    };
    Ok(Value::Int(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;

    fn run(source: &str) -> Outcome {
        let hir = analyze("interp.ry", source).unwrap();
        execute(&hir, DEFAULT_STEP_BUDGET)
    }

    #[test]
    fn matches_v0_runtime_observations() {
        let cases = [
            (
                "fn main() { print(\"hello\", 7, true, false) }",
                "hello 7 true false\n",
            ),
            ("fn main() { print(1 + 2 * 3, (1 + 2) * 3) }", "7 9\n"),
            ("fn main() { print(-7 / 3, -7 % 3) }", "-2 -1\n"),
            (
                "fn main() { print(9223372036854775807 + 1) }",
                "-9223372036854775808\n",
            ),
            (
                "fn main() { var sum = 0\nfor i in 1..5 { sum += i }\nprint(sum) }",
                "10\n",
            ),
            (
                "fn main() { for i in 5..1 { print(i) }\nprint(\"done\") }",
                "done\n",
            ),
            (
                "fn main() { let x = 1\nif true { let x = 2\nprint(x) }\nprint(x) }",
                "2\n1\n",
            ),
            (
                "fn twice(x: int) -> int { return x * 2 }\nfn main() { print(twice(6)) }",
                "12\n",
            ),
            (
                "fn side() -> bool { print(\"evaluated\")\nreturn true }\nfn main() { print(false && side())\nprint(true || side()) }",
                "false\ntrue\n",
            ),
            (
                "fn side() -> int { print(\"side\")\nreturn 2 }\nfn main() { print(1, side()) }",
                "side\n1 2\n",
            ),
        ];
        for (source, expected) in cases {
            let outcome = run(source);
            assert_eq!(outcome.completion, Completion::Returned(None), "{source}");
            assert_eq!(String::from_utf8(outcome.stdout).unwrap(), expected);
        }
    }

    #[test]
    fn traps_on_the_guarded_divisions() {
        assert_eq!(
            run("fn main() { print(1 / 0) }").completion,
            Completion::Trap
        );
        assert_eq!(
            run("fn main() { print((-9223372036854775807 - 1) / -1) }").completion,
            Completion::Trap
        );
    }

    #[test]
    fn integer_main_returns_the_full_value() {
        let outcome = run("fn main() -> int { return 7 }");
        assert_eq!(outcome.completion, Completion::Returned(Some(7)));
    }
}
