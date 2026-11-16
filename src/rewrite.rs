//! Verified rewrite search over canonical RIR.
//!
//! Pipelines are a coreset, not a trained net. A fixed linear cost model ranks
//! them. The interpreter rejects candidates it can execute, and
//! [`native_equivalent`] is the driver's gate before a candidate binary is
//! kept. See `docs/research/rir-optimization.md`.

use crate::hir::{self, AssignOp, BinaryOp, Expr, ExprKind, Program, Stmt, Type, UnaryOp};
use crate::interp::{self, Completion, DEFAULT_STEP_BUDGET, Outcome};
use crate::llvm;
use crate::project::{self, Optimization};
use crate::rir;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const PREFIX_ITERS: i64 = 32;
const SIMULATE_LIMIT: i64 = 4_096;
const MAX_PERIOD: i128 = 8_192;
const MAX_RESIDUAL_BYTES: usize = 1_000_000;

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub step_budget: u64,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            step_budget: DEFAULT_STEP_BUDGET,
        }
    }
}

impl SearchOptions {
    pub fn new(step_budget: u64) -> Result<Self, String> {
        if step_budget == 0 {
            return Err("rewrite step budget must be positive".to_owned());
        }
        Ok(Self { step_budget })
    }
}

#[derive(Clone, Debug)]
pub struct CandidateReport {
    pub pipeline: &'static str,
    pub predicted_cost: f64,
    pub estimated_iterations: u128,
    pub status: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct SearchOutcome {
    pub program: Program,
    pub pipeline: &'static str,
    pub predicted_cost: f64,
    pub identity_cost: f64,
    pub estimated_iterations: u128,
    pub interpreter_agrees: bool,
    pub candidates: Vec<CandidateReport>,
}

#[derive(Clone, Copy)]
struct Pipeline {
    name: &'static str,
    rank: u8,
}

const PIPELINES: [Pipeline; 4] = [
    Pipeline {
        name: "identity",
        rank: 0,
    },
    Pipeline {
        name: "fold",
        rank: 1,
    },
    Pipeline {
        name: "affine",
        rank: 2,
    },
    Pipeline {
        name: "eval",
        rank: 3,
    },
];

pub fn search(program: &Program, options: &SearchOptions) -> SearchOutcome {
    let reference = interp::execute(program, options.step_budget);
    let mut best: Option<Scored> = None;
    let mut candidates = Vec::with_capacity(PIPELINES.len());
    for pipeline in PIPELINES {
        let Some(optimized) = apply(program, pipeline.name, options.step_budget) else {
            candidates.push(CandidateReport {
                pipeline: pipeline.name,
                predicted_cost: f64::INFINITY,
                estimated_iterations: u128::MAX,
                status: "skipped",
                detail: "pipeline did not apply".to_owned(),
            });
            continue;
        };
        let profile = profile(&optimized);
        let agrees = pipeline.name == "identity"
            || interpreter_agrees(&reference, &optimized, options.step_budget);
        let detail = if agrees {
            "interpreter agrees with the reference".to_owned()
        } else if matches!(
            reference.completion,
            Completion::BudgetExceeded | Completion::DepthExceeded
        ) {
            "reference exceeded the interpreter budget; native check required".to_owned()
        } else {
            "interpreter mismatch".to_owned()
        };
        let status = if agrees { "verified" } else { "rejected" };
        candidates.push(CandidateReport {
            pipeline: pipeline.name,
            predicted_cost: profile.cost,
            estimated_iterations: profile.iterations,
            status,
            detail,
        });
        if !agrees {
            continue;
        }
        let scored = Scored {
            program: optimized,
            pipeline,
            profile,
        };
        let replace = best.as_ref().is_none_or(|current| scored.beats(current));
        if replace {
            best = Some(scored);
        }
    }
    let chosen = best.expect("identity pipeline is always applicable and equivalent");
    let identity_cost = candidates
        .iter()
        .find(|candidate| candidate.pipeline == "identity")
        .map(|candidate| candidate.predicted_cost)
        .unwrap_or(chosen.profile.cost);
    for candidate in &mut candidates {
        if candidate.pipeline == chosen.pipeline.name && candidate.status == "verified" {
            candidate.status = "selected";
        } else if candidate.status == "verified" {
            candidate.status = "dominated";
        }
    }
    SearchOutcome {
        program: chosen.program,
        pipeline: chosen.pipeline.name,
        predicted_cost: chosen.profile.cost,
        identity_cost,
        estimated_iterations: chosen.profile.iterations,
        interpreter_agrees: true,
        candidates,
    }
}

struct Scored {
    program: Program,
    pipeline: Pipeline,
    profile: Profile,
}

impl Scored {
    fn beats(&self, other: &Scored) -> bool {
        (
            self.profile.iterations,
            self.profile.cost,
            self.pipeline.rank,
        ) < (
            other.profile.iterations,
            other.profile.cost,
            other.pipeline.rank,
        )
    }
}

struct Profile {
    iterations: u128,
    cost: f64,
}

fn profile(program: &Program) -> Profile {
    let features = features(program);
    let iterations = estimate_iterations(program).unwrap_or(u128::MAX);
    Profile {
        iterations,
        cost: predict(&features, iterations),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Features {
    pub loops: u64,
    pub calls: u64,
    pub arithmetic: u64,
    pub prints: u64,
}

impl Features {
    pub fn from_program(program: &Program) -> Self {
        features(program)
    }
}

/// Reviewed linear prior. A JEPA cost head is meant to replace this function,
/// not the verifier around it. Iteration count is the primary search key;
/// these weights only separate programs with the same trip count.
pub fn predict(features: &Features, iterations: u128) -> f64 {
    let iterations = iterations.min(1_000_000_000_000) as f64;
    1_000.0
        + iterations * (1.0 + 0.05 * features.arithmetic as f64 + 0.25 * features.calls as f64)
        + features.prints as f64 * 25.0
}

fn interpreter_agrees(reference: &Outcome, candidate: &Program, budget: u64) -> bool {
    if matches!(
        reference.completion,
        Completion::BudgetExceeded | Completion::DepthExceeded
    ) {
        return false;
    }
    let outcome = interp::execute(candidate, budget);
    outcome.equivalent(reference)
}

fn apply(program: &Program, pipeline: &str, budget: u64) -> Option<Program> {
    match pipeline {
        "identity" => Some(program.clone()),
        "fold" => Some(fold_inline(program)),
        "affine" => {
            let folded = fold_inline(program);
            Some(fold_program(&affine_sums(&folded)))
        }
        "eval" => {
            let folded = fold_inline(program);
            let estimate = estimate_iterations(&folded)?;
            if estimate > u128::from(budget) {
                return None;
            }
            partial_eval(&folded, budget)
        }
        _ => None,
    }
}

fn fold_inline(program: &Program) -> Program {
    fold_program(&inline_pure(&fold_program(program)))
}

pub fn native_equivalent(reference: &Program, candidate: &Program) -> Result<bool, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("rusty-diff-{stamp}-{}", std::process::id()));
    let result = native_equivalent_in(&directory, reference, candidate);
    let _ = fs::remove_dir_all(&directory);
    result
}

fn native_equivalent_in(
    directory: &std::path::Path,
    reference: &Program,
    candidate: &Program,
) -> Result<bool, String> {
    let reference_bin = compile_program(reference, &directory.join("reference"), "reference")?;
    let candidate_bin = compile_program(candidate, &directory.join("candidate"), "candidate")?;
    let reference_out = run_native(&reference_bin)?;
    let candidate_out = run_native(&candidate_bin)?;
    Ok(reference_out.status.code() == candidate_out.status.code()
        && reference_out.stdout == candidate_out.stdout
        && reference_out.stderr == candidate_out.stderr)
}

fn compile_program(
    program: &Program,
    directory: &std::path::Path,
    name: &str,
) -> Result<PathBuf, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let llvm_path = directory.join(format!("{name}.ll"));
    let executable = directory.join(name);
    let ir = rir::Program::from_hir(program);
    fs::write(&llvm_path, llvm::emit(&ir))
        .map_err(|error| format!("cannot write {}: {error}", llvm_path.display()))?;
    project::compile_llvm(&llvm_path, &executable, Optimization::O0, None)?;
    Ok(executable)
}

fn run_native(executable: &std::path::Path) -> Result<std::process::Output, String> {
    Command::new(executable)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", executable.display()))
}

fn features(program: &Program) -> Features {
    let mut features = Features {
        loops: 0,
        calls: 0,
        arithmetic: 0,
        prints: 0,
    };
    for function in &program.functions {
        count_block(&function.body, &mut features);
    }
    features
}

fn count_block(block: &[Stmt], features: &mut Features) {
    for statement in block {
        match statement {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } | Stmt::Expr(value) => {
                count_expr(value, features);
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    count_expr(value, features);
                }
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
            } => {
                count_expr(condition, features);
                count_block(then_block, features);
                count_block(else_block, features);
            }
            Stmt::For {
                start, end, body, ..
            } => {
                features.loops += 1;
                count_expr(start, features);
                count_expr(end, features);
                count_block(body, features);
            }
        }
    }
}

fn count_expr(expression: &Expr, features: &mut Features) {
    match &expression.kind {
        ExprKind::Unary { expr, .. } => count_expr(expr, features),
        ExprKind::Binary { op, left, right } => {
            if matches!(
                op,
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
            ) {
                features.arithmetic += 1;
            }
            count_expr(left, features);
            count_expr(right, features);
        }
        ExprKind::Call { args, .. } => {
            features.calls += 1;
            for arg in args {
                count_expr(arg, features);
            }
        }
        ExprKind::Print(args) => {
            features.prints += 1;
            for arg in args {
                count_expr(arg, features);
            }
        }
        _ => {}
    }
}

fn estimate_iterations(program: &Program) -> Option<u128> {
    let mut memo = vec![None; program.functions.len()];
    let mut done = vec![false; program.functions.len()];
    let mut stack = vec![false; program.functions.len()];
    let main = program
        .functions
        .iter()
        .find(|function| function.name == "main")?;
    function_cost(program, main.id, &mut memo, &mut done, &mut stack)
}

fn function_cost(
    program: &Program,
    id: usize,
    memo: &mut [Option<u128>],
    done: &mut [bool],
    stack: &mut [bool],
) -> Option<u128> {
    if stack[id] {
        return None;
    }
    if done[id] {
        return memo[id];
    }
    stack[id] = true;
    let cost = block_cost(program, &program.functions[id].body, memo, done, stack);
    stack[id] = false;
    done[id] = true;
    memo[id] = cost;
    cost
}

fn block_cost(
    program: &Program,
    block: &[Stmt],
    memo: &mut [Option<u128>],
    done: &mut [bool],
    stack: &mut [bool],
) -> Option<u128> {
    let mut total = 0u128;
    for statement in block {
        total = total.saturating_add(stmt_cost(program, statement, memo, done, stack)?);
    }
    Some(total)
}

fn stmt_cost(
    program: &Program,
    statement: &Stmt,
    memo: &mut [Option<u128>],
    done: &mut [bool],
    stack: &mut [bool],
) -> Option<u128> {
    match statement {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } | Stmt::Expr(value) => {
            expr_cost(program, value, memo, done, stack)
        }
        Stmt::Return(value) => match value {
            Some(value) => expr_cost(program, value, memo, done, stack),
            None => Some(0),
        },
        Stmt::If {
            condition,
            then_block,
            else_block,
        } => {
            let condition = expr_cost(program, condition, memo, done, stack)?;
            let then_cost = block_cost(program, then_block, memo, done, stack)?;
            let else_cost = block_cost(program, else_block, memo, done, stack)?;
            Some(condition.saturating_add(then_cost.max(else_cost)))
        }
        Stmt::For {
            start, end, body, ..
        } => {
            let trip = match (const_int(start), const_int(end)) {
                (Some(start), Some(end)) if end > start => u128::from(end.abs_diff(start)),
                (Some(start), Some(end)) if end <= start => 0,
                _ => return None,
            };
            let body = block_cost(program, body, memo, done, stack)?.saturating_add(1);
            Some(trip.saturating_mul(body))
        }
    }
}

fn expr_cost(
    program: &Program,
    expression: &Expr,
    memo: &mut [Option<u128>],
    done: &mut [bool],
    stack: &mut [bool],
) -> Option<u128> {
    match &expression.kind {
        ExprKind::Unary { expr, .. } => expr_cost(program, expr, memo, done, stack),
        ExprKind::Binary { left, right, .. } => {
            let left = expr_cost(program, left, memo, done, stack)?;
            let right = expr_cost(program, right, memo, done, stack)?;
            Some(left.saturating_add(right))
        }
        ExprKind::Call { function, args, .. } => {
            let mut total = function_cost(program, *function, memo, done, stack)?;
            for arg in args {
                total = total.saturating_add(expr_cost(program, arg, memo, done, stack)?);
            }
            Some(total.saturating_add(1))
        }
        ExprKind::Print(args) => {
            let mut total = 1u128;
            for arg in args {
                total = total.saturating_add(expr_cost(program, arg, memo, done, stack)?);
            }
            Some(total)
        }
        _ => Some(0),
    }
}

fn fold_program(program: &Program) -> Program {
    let mut program = program.clone();
    for function in &mut program.functions {
        function.body = fold_block(&function.body);
    }
    program
}

fn fold_block(block: &[Stmt]) -> Vec<Stmt> {
    block.iter().map(fold_stmt).collect()
}

fn fold_stmt(statement: &Stmt) -> Stmt {
    match statement {
        Stmt::Let { local, value } => Stmt::Let {
            local: *local,
            value: fold_expr(value),
        },
        Stmt::Assign { local, op, value } => Stmt::Assign {
            local: *local,
            op: *op,
            value: fold_expr(value),
        },
        Stmt::Return(value) => Stmt::Return(value.as_ref().map(fold_expr)),
        Stmt::Expr(value) => Stmt::Expr(fold_expr(value)),
        Stmt::If {
            condition,
            then_block,
            else_block,
        } => Stmt::If {
            condition: fold_expr(condition),
            then_block: fold_block(then_block),
            else_block: fold_block(else_block),
        },
        Stmt::For {
            local,
            start,
            end,
            body,
        } => Stmt::For {
            local: *local,
            start: fold_expr(start),
            end: fold_expr(end),
            body: fold_block(body),
        },
    }
}

fn fold_expr(expression: &Expr) -> Expr {
    let folded = match &expression.kind {
        ExprKind::Unary { op, expr } => Expr {
            ty: expression.ty,
            kind: ExprKind::Unary {
                op: *op,
                expr: Box::new(fold_expr(expr)),
            },
        },
        ExprKind::Binary { op, left, right } => Expr {
            ty: expression.ty,
            kind: ExprKind::Binary {
                op: *op,
                left: Box::new(fold_expr(left)),
                right: Box::new(fold_expr(right)),
            },
        },
        ExprKind::Call {
            function,
            name,
            args,
        } => Expr {
            ty: expression.ty,
            kind: ExprKind::Call {
                function: *function,
                name: name.clone(),
                args: args.iter().map(fold_expr).collect(),
            },
        },
        ExprKind::Print(args) => Expr {
            ty: expression.ty,
            kind: ExprKind::Print(args.iter().map(fold_expr).collect()),
        },
        _ => expression.clone(),
    };
    if let Some(value) = eval_const(&folded) {
        return value_expr(value, folded.ty);
    }
    simplify(folded)
}

fn eval_const(expression: &Expr) -> Option<interp::Value> {
    match &expression.kind {
        ExprKind::Int(value) => Some(interp::Value::Int(*value)),
        ExprKind::Bool(value) => Some(interp::Value::Bool(*value)),
        ExprKind::String(value) => Some(interp::Value::String(value.clone())),
        ExprKind::Unary { op, expr } => match (op, eval_const(expr)?) {
            (UnaryOp::Neg, interp::Value::Int(value)) => {
                Some(interp::Value::Int(value.wrapping_neg()))
            }
            (UnaryOp::Not, interp::Value::Bool(value)) => Some(interp::Value::Bool(!value)),
            _ => None,
        },
        ExprKind::Binary { op, left, right } => {
            let left = eval_const(left)?;
            let right = eval_const(right)?;
            match (op, left, right) {
                (BinaryOp::Add, interp::Value::Int(a), interp::Value::Int(b)) => {
                    Some(interp::Value::Int(a.wrapping_add(b)))
                }
                (BinaryOp::Sub, interp::Value::Int(a), interp::Value::Int(b)) => {
                    Some(interp::Value::Int(a.wrapping_sub(b)))
                }
                (BinaryOp::Mul, interp::Value::Int(a), interp::Value::Int(b)) => {
                    Some(interp::Value::Int(a.wrapping_mul(b)))
                }
                (BinaryOp::Div, interp::Value::Int(a), interp::Value::Int(b)) => {
                    checked_div(a, b, false).map(interp::Value::Int)
                }
                (BinaryOp::Rem, interp::Value::Int(a), interp::Value::Int(b)) => {
                    checked_div(a, b, true).map(interp::Value::Int)
                }
                (BinaryOp::Eq, left, right) => Some(interp::Value::Bool(left == right)),
                (BinaryOp::Ne, left, right) => Some(interp::Value::Bool(left != right)),
                (BinaryOp::Lt, interp::Value::Int(a), interp::Value::Int(b)) => {
                    Some(interp::Value::Bool(a < b))
                }
                (BinaryOp::Le, interp::Value::Int(a), interp::Value::Int(b)) => {
                    Some(interp::Value::Bool(a <= b))
                }
                (BinaryOp::Gt, interp::Value::Int(a), interp::Value::Int(b)) => {
                    Some(interp::Value::Bool(a > b))
                }
                (BinaryOp::Ge, interp::Value::Int(a), interp::Value::Int(b)) => {
                    Some(interp::Value::Bool(a >= b))
                }
                (BinaryOp::And, interp::Value::Bool(a), interp::Value::Bool(b)) => {
                    Some(interp::Value::Bool(a && b))
                }
                (BinaryOp::Or, interp::Value::Bool(a), interp::Value::Bool(b)) => {
                    Some(interp::Value::Bool(a || b))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn checked_div(left: i64, right: i64, remainder: bool) -> Option<i64> {
    if right == 0 || (left == i64::MIN && right == -1) {
        return None;
    }
    Some(if remainder {
        left.wrapping_rem(right)
    } else {
        left.wrapping_div(right)
    })
}

fn simplify(expression: Expr) -> Expr {
    let Expr { ty, kind } = expression;
    let kind = match kind {
        ExprKind::Binary {
            op: BinaryOp::Add,
            left,
            right,
        } if is_int(&left, 0) || is_int(&right, 0) => {
            return if is_int(&right, 0) { *left } else { *right };
        }
        ExprKind::Binary {
            op: BinaryOp::Sub,
            left,
            right,
        } if is_int(&right, 0) => return *left,
        ExprKind::Binary {
            op: BinaryOp::Mul,
            left,
            right,
        } if is_int(&left, 1) || is_int(&right, 1) => {
            return if is_int(&right, 1) { *left } else { *right };
        }
        ExprKind::Binary {
            op: BinaryOp::Mul,
            left,
            right,
        } if (is_int(&left, 0) && is_pure(&right)) || (is_int(&right, 0) && is_pure(&left)) => {
            return int_expr(0);
        }
        ExprKind::Binary {
            op: BinaryOp::Div,
            left,
            right,
        } if is_int(&right, 1) => return *left,
        ExprKind::Binary {
            op: BinaryOp::Rem,
            left,
            right,
        } if is_int(&right, 1) && is_pure(&left) => {
            return int_expr(0);
        }
        ExprKind::Binary {
            op: BinaryOp::And,
            left,
            right,
        } if is_bool(&left, false) => {
            return bool_expr(false);
        }
        ExprKind::Binary {
            op: BinaryOp::Or,
            left,
            right,
        } if is_bool(&left, true) => {
            return bool_expr(true);
        }
        ExprKind::Binary {
            op: BinaryOp::And,
            left,
            right,
        } if is_bool(&left, true) => return *right,
        ExprKind::Binary {
            op: BinaryOp::Or,
            left,
            right,
        } if is_bool(&left, false) => return *right,
        ExprKind::Binary {
            op: BinaryOp::And,
            left,
            right,
        } if is_bool(&right, true) => return *left,
        ExprKind::Binary {
            op: BinaryOp::Or,
            left,
            right,
        } if is_bool(&right, false) => return *left,
        ExprKind::Binary {
            op: BinaryOp::And,
            left,
            right,
        } if is_bool(&right, false) && is_pure(&left) => {
            return bool_expr(false);
        }
        ExprKind::Binary {
            op: BinaryOp::Or,
            left,
            right,
        } if is_bool(&right, true) && is_pure(&left) => {
            return bool_expr(true);
        }
        ExprKind::Unary {
            op: UnaryOp::Not,
            expr,
        } if matches!(
            expr.kind,
            ExprKind::Unary {
                op: UnaryOp::Not,
                ..
            }
        ) =>
        {
            if let ExprKind::Unary { expr, .. } = expr.kind {
                return *expr;
            }
            ExprKind::Unary {
                op: UnaryOp::Not,
                expr,
            }
        }
        other => other,
    };
    Expr { ty, kind }
}

fn inline_pure(program: &Program) -> Program {
    let bodies = program
        .functions
        .iter()
        .map(inline_body)
        .collect::<Vec<_>>();
    let mut program = program.clone();
    for function in &mut program.functions {
        function.body = inline_block(&function.body, &bodies);
    }
    program
}

fn inline_body(function: &hir::Function) -> Option<Expr> {
    let [Stmt::Return(Some(expr))] = function.body.as_slice() else {
        return None;
    };
    if !is_pure(expr) || expr_locals(expr).any(|local| local >= function.params.len()) {
        return None;
    }
    Some(expr.clone())
}

fn inline_block(block: &[Stmt], bodies: &[Option<Expr>]) -> Vec<Stmt> {
    block
        .iter()
        .map(|statement| inline_stmt(statement, bodies))
        .collect()
}

fn inline_stmt(statement: &Stmt, bodies: &[Option<Expr>]) -> Stmt {
    match statement {
        Stmt::Let { local, value } => Stmt::Let {
            local: *local,
            value: inline_expr(value, bodies),
        },
        Stmt::Assign { local, op, value } => Stmt::Assign {
            local: *local,
            op: *op,
            value: inline_expr(value, bodies),
        },
        Stmt::Return(value) => Stmt::Return(value.as_ref().map(|value| inline_expr(value, bodies))),
        Stmt::Expr(value) => Stmt::Expr(inline_expr(value, bodies)),
        Stmt::If {
            condition,
            then_block,
            else_block,
        } => Stmt::If {
            condition: inline_expr(condition, bodies),
            then_block: inline_block(then_block, bodies),
            else_block: inline_block(else_block, bodies),
        },
        Stmt::For {
            local,
            start,
            end,
            body,
        } => Stmt::For {
            local: *local,
            start: inline_expr(start, bodies),
            end: inline_expr(end, bodies),
            body: inline_block(body, bodies),
        },
    }
}

fn inline_expr(expression: &Expr, bodies: &[Option<Expr>]) -> Expr {
    match &expression.kind {
        ExprKind::Call { function, args, .. }
            if args.iter().all(is_pure)
                && bodies.get(*function).and_then(Option::as_ref).is_some() =>
        {
            let body = bodies[*function].as_ref().expect("checked");
            let substituted = substitute(body, args);
            inline_expr(&substituted, bodies)
        }
        ExprKind::Unary { op, expr } => Expr {
            ty: expression.ty,
            kind: ExprKind::Unary {
                op: *op,
                expr: Box::new(inline_expr(expr, bodies)),
            },
        },
        ExprKind::Binary { op, left, right } => Expr {
            ty: expression.ty,
            kind: ExprKind::Binary {
                op: *op,
                left: Box::new(inline_expr(left, bodies)),
                right: Box::new(inline_expr(right, bodies)),
            },
        },
        ExprKind::Call {
            function,
            name,
            args,
        } => Expr {
            ty: expression.ty,
            kind: ExprKind::Call {
                function: *function,
                name: name.clone(),
                args: args.iter().map(|arg| inline_expr(arg, bodies)).collect(),
            },
        },
        ExprKind::Print(args) => Expr {
            ty: expression.ty,
            kind: ExprKind::Print(args.iter().map(|arg| inline_expr(arg, bodies)).collect()),
        },
        _ => expression.clone(),
    }
}

fn substitute(expression: &Expr, args: &[Expr]) -> Expr {
    match &expression.kind {
        ExprKind::Local(local) if *local < args.len() => args[*local].clone(),
        ExprKind::Unary { op, expr } => Expr {
            ty: expression.ty,
            kind: ExprKind::Unary {
                op: *op,
                expr: Box::new(substitute(expr, args)),
            },
        },
        ExprKind::Binary { op, left, right } => Expr {
            ty: expression.ty,
            kind: ExprKind::Binary {
                op: *op,
                left: Box::new(substitute(left, args)),
                right: Box::new(substitute(right, args)),
            },
        },
        _ => expression.clone(),
    }
}

fn affine_sums(program: &Program) -> Program {
    let mut program = program.clone();
    for function in &mut program.functions {
        let mut known = BTreeMap::new();
        function.body = reduce_block(&function.body, &mut known);
    }
    program
}

fn reduce_block(block: &[Stmt], known: &mut BTreeMap<usize, i64>) -> Vec<Stmt> {
    let mut output = Vec::with_capacity(block.len());
    for statement in block {
        match statement {
            Stmt::Let { local, value } => {
                let value = fold_expr(value);
                if let Some(constant) = const_int(&value) {
                    known.insert(*local, constant);
                } else {
                    known.remove(local);
                }
                output.push(Stmt::Let {
                    local: *local,
                    value,
                });
            }
            Stmt::Assign { local, op, value } => {
                let value = fold_expr(value);
                if let Some(constant) = const_int(&value)
                    && let Some(current) = known.get(local).copied()
                {
                    let updated = match op {
                        AssignOp::Set => constant,
                        AssignOp::Add => current.wrapping_add(constant),
                        AssignOp::Sub => current.wrapping_sub(constant),
                    };
                    known.insert(*local, updated);
                    output.push(Stmt::Assign {
                        local: *local,
                        op: AssignOp::Set,
                        value: int_expr(updated),
                    });
                } else {
                    known.remove(local);
                    output.push(Stmt::Assign {
                        local: *local,
                        op: *op,
                        value,
                    });
                }
            }
            Stmt::For {
                local,
                start,
                end,
                body,
            } => {
                let start = fold_expr(start);
                let end = fold_expr(end);
                if let Some(reduced) = reduce_for(*local, &start, &end, body, known) {
                    output.push(reduced);
                } else {
                    known.clear();
                    let mut inner = BTreeMap::new();
                    output.push(Stmt::For {
                        local: *local,
                        start,
                        end,
                        body: reduce_block(body, &mut inner),
                    });
                }
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
            } => {
                known.clear();
                output.push(Stmt::If {
                    condition: fold_expr(condition),
                    then_block: reduce_block(then_block, &mut BTreeMap::new()),
                    else_block: reduce_block(else_block, &mut BTreeMap::new()),
                });
            }
            Stmt::Return(value) => output.push(Stmt::Return(value.as_ref().map(fold_expr))),
            Stmt::Expr(value) => output.push(Stmt::Expr(fold_expr(value))),
        }
    }
    output
}

fn reduce_for(
    index: usize,
    start: &Expr,
    end: &Expr,
    body: &[Stmt],
    known: &mut BTreeMap<usize, i64>,
) -> Option<Stmt> {
    let start_value = const_int(start)?;
    let end_value = const_int(end)?;
    let (accumulator, kind) = match_accumulation(body, index)?;
    let init = known.get(&accumulator).copied()?;
    let sum = sum_of(&kind, start_value, end_value)?;
    let final_value = i128::from(init).checked_add(sum)?;
    let final_value = i64::try_from(final_value).ok()?;
    if !prefix_matches(&kind, start_value, end_value, init, final_value) {
        return None;
    }
    known.insert(accumulator, final_value);
    Some(Stmt::Assign {
        local: accumulator,
        op: AssignOp::Set,
        value: int_expr(final_value),
    })
}

fn prefix_matches(kind: &SumKind, start: i64, end: i64, init: i64, full: i64) -> bool {
    let trip = if end > start {
        end.saturating_sub(start)
    } else {
        0
    };
    if trip == 0 {
        return full == init;
    }
    let n = trip.min(PREFIX_ITERS);
    let mut acc = init;
    for offset in 0..n {
        let Some(index) = start.checked_add(offset) else {
            return false;
        };
        let Some(term) = term_at(kind, index) else {
            return false;
        };
        acc = acc.wrapping_add(term);
    }
    let Some(end_prefix) = start.checked_add(n) else {
        return false;
    };
    let Some(predicted_sum) = sum_of(kind, start, end_prefix) else {
        return false;
    };
    let Some(predicted) = i128::from(init).checked_add(predicted_sum) else {
        return false;
    };
    let Ok(predicted) = i64::try_from(predicted) else {
        return false;
    };
    acc == predicted && (trip > PREFIX_ITERS || full == predicted)
}

#[derive(Clone, Debug)]
enum SumKind {
    Linear {
        factor: i128,
        base: i128,
    },
    Rem {
        factor: i128,
        base: i128,
        modulus: i64,
    },
    Div {
        divisor: i64,
    },
}

fn match_accumulation(body: &[Stmt], index: usize) -> Option<(usize, SumKind)> {
    let [
        Stmt::Assign {
            local,
            op: AssignOp::Add,
            value,
        },
    ] = body
    else {
        return None;
    };
    let kind = sum_kind(value, index)?;
    Some((*local, kind))
}

fn sum_kind(expression: &Expr, index: usize) -> Option<SumKind> {
    match &expression.kind {
        ExprKind::Binary {
            op: BinaryOp::Rem,
            left,
            right,
        } => {
            let modulus = const_int(right)?;
            if modulus <= 0 {
                return None;
            }
            let linear = linear(left, index)?;
            Some(SumKind::Rem {
                factor: linear.0,
                base: linear.1,
                modulus,
            })
        }
        ExprKind::Binary {
            op: BinaryOp::Div,
            left,
            right,
        } => {
            let divisor = const_int(right)?;
            let (factor, base) = linear(left, index)?;
            if divisor <= 0 || factor != 1 || base != 0 {
                return None;
            }
            Some(SumKind::Div { divisor })
        }
        _ => {
            let (factor, base) = linear(expression, index)?;
            Some(SumKind::Linear { factor, base })
        }
    }
}

fn linear(expression: &Expr, index: usize) -> Option<(i128, i128)> {
    match &expression.kind {
        ExprKind::Int(value) => Some((0, i128::from(*value))),
        ExprKind::Local(local) if *local == index => Some((1, 0)),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => {
            let (factor, base) = linear(expr, index)?;
            Some((-factor, -base))
        }
        ExprKind::Binary {
            op: BinaryOp::Add,
            left,
            right,
        } => {
            let (lf, lb) = linear(left, index)?;
            let (rf, rb) = linear(right, index)?;
            Some((lf + rf, lb + rb))
        }
        ExprKind::Binary {
            op: BinaryOp::Sub,
            left,
            right,
        } => {
            let (lf, lb) = linear(left, index)?;
            let (rf, rb) = linear(right, index)?;
            Some((lf - rf, lb - rb))
        }
        ExprKind::Binary {
            op: BinaryOp::Mul,
            left,
            right,
        } => {
            let (lf, lb) = linear(left, index)?;
            let (rf, rb) = linear(right, index)?;
            if lf == 0 {
                Some((rf * lb, rb * lb))
            } else if rf == 0 {
                Some((lf * rb, lb * rb))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn sum_of(kind: &SumKind, start: i64, end: i64) -> Option<i128> {
    if end <= start {
        return Some(0);
    }
    let trip = i128::from(end) - i128::from(start);
    if trip <= i128::from(SIMULATE_LIMIT) {
        let mut acc = 0i128;
        for offset in 0..trip {
            let term = i128::from(term_at(kind, start + i64::try_from(offset).ok()?)?);
            acc = acc.checked_add(term)?;
        }
        return Some(acc);
    }
    match kind {
        SumKind::Linear { factor, base } => sum_linear(*factor, *base, start, end),
        SumKind::Rem {
            factor,
            base,
            modulus,
        } => sum_rem(*factor, *base, *modulus, start, end),
        SumKind::Div { divisor } => sum_div(start, end, *divisor),
    }
}

fn term_at(kind: &SumKind, index: i64) -> Option<i64> {
    match kind {
        SumKind::Linear { factor, base } => fit_i64(factor * i128::from(index) + base),
        SumKind::Rem {
            factor,
            base,
            modulus,
        } => {
            let value = factor * i128::from(index) + base;
            if value < 0 {
                return None;
            }
            let value = fit_i64(value)?;
            Some(value.wrapping_rem(*modulus))
        }
        SumKind::Div { divisor } => {
            if index < 0 {
                return None;
            }
            Some(index.wrapping_div(*divisor))
        }
    }
}

fn sum_linear(factor: i128, base: i128, start: i64, end: i64) -> Option<i128> {
    let first = factor * i128::from(start) + base;
    let last = factor * i128::from(end - 1) + base;
    if first < i64::MIN as i128
        || first > i64::MAX as i128
        || last < i64::MIN as i128
        || last > i64::MAX as i128
        || (first < 0 && last > 0)
        || (first > 0 && last < 0)
    {
        return None;
    }
    let n = i128::from(end) - i128::from(start);
    let sum_index = n * (i128::from(start) + i128::from(end) - 1) / 2;
    Some(factor * sum_index + base * n)
}

fn sum_rem(factor: i128, base: i128, modulus: i64, start: i64, end: i64) -> Option<i128> {
    let modulus_i = i128::from(modulus);
    let period = period(factor, modulus_i);
    if period > MAX_PERIOD {
        return None;
    }
    let mut cycle_sum = 0i128;
    let mut prefix = Vec::with_capacity(period as usize);
    for offset in 0..period {
        let index = start.checked_add(i64::try_from(offset).ok()?)?;
        let term = i128::from(term_at(
            &SumKind::Rem {
                factor,
                base,
                modulus,
            },
            index,
        )?);
        prefix.push(term);
        cycle_sum += term;
    }
    let n = i128::from(end) - i128::from(start);
    let full = n / period;
    let remainder = (n % period) as usize;
    Some(full * cycle_sum + prefix.iter().take(remainder).sum::<i128>())
}

fn sum_div(start: i64, end: i64, divisor: i64) -> Option<i128> {
    if start < 0 || divisor <= 0 {
        return None;
    }
    Some(sum_floor(end, divisor) - sum_floor(start, divisor))
}

fn sum_floor(n: i64, divisor: i64) -> i128 {
    if n <= 0 {
        return 0;
    }
    let n = i128::from(n);
    let divisor = i128::from(divisor);
    let q = (n - 1) / divisor;
    if q == 0 {
        return 0;
    }
    divisor * (q - 1) * q / 2 + q * (n - q * divisor)
}

fn period(factor: i128, modulus: i128) -> i128 {
    let factor = factor.rem_euclid(modulus);
    if factor == 0 {
        1
    } else {
        modulus / gcd(factor, modulus)
    }
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    a.abs()
}

fn fit_i64(value: i128) -> Option<i64> {
    i64::try_from(value).ok()
}

fn partial_eval(program: &Program, budget: u64) -> Option<Program> {
    let outcome = interp::execute(program, budget);
    let Completion::Returned(result) = outcome.completion else {
        return None;
    };
    if outcome.stdout.len() > MAX_RESIDUAL_BYTES {
        return None;
    }
    let text = String::from_utf8(outcome.stdout).ok()?;
    if !text.is_empty() && !text.ends_with('\n') {
        return None;
    }
    let mut rewritten = program.clone();
    let main = rewritten
        .functions
        .iter_mut()
        .find(|function| function.name == "main")?;
    if main.result == Type::Int && result.is_none() {
        return None;
    }
    if main.result == Type::Unit && result.is_some() {
        return None;
    }
    main.params.clear();
    main.locals.clear();
    let mut body = Vec::new();
    if !text.is_empty() {
        let printed = text.strip_suffix('\n').unwrap_or(&text);
        body.push(Stmt::Expr(Expr {
            ty: Type::Unit,
            kind: ExprKind::Print(vec![string_expr(printed)]),
        }));
    }
    if let Some(result) = result {
        body.push(Stmt::Return(Some(int_expr(result))));
    }
    main.body = body;
    Some(rewritten)
}

fn is_pure(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::String(_) | ExprKind::Local(_) => true,
        ExprKind::Unary { expr, .. } => is_pure(expr),
        ExprKind::Binary { left, right, .. } => is_pure(left) && is_pure(right),
        ExprKind::Call { .. } | ExprKind::Print(_) => false,
    }
}

fn expr_locals(expression: &Expr) -> impl Iterator<Item = usize> + '_ {
    let mut locals = Vec::new();
    collect_locals(expression, &mut locals);
    locals.into_iter()
}

fn collect_locals(expression: &Expr, locals: &mut Vec<usize>) {
    match &expression.kind {
        ExprKind::Local(local) => locals.push(*local),
        ExprKind::Unary { expr, .. } => collect_locals(expr, locals),
        ExprKind::Binary { left, right, .. } => {
            collect_locals(left, locals);
            collect_locals(right, locals);
        }
        ExprKind::Call { args, .. } | ExprKind::Print(args) => {
            for arg in args {
                collect_locals(arg, locals);
            }
        }
        _ => {}
    }
}

fn const_int(expression: &Expr) -> Option<i64> {
    match &expression.kind {
        ExprKind::Int(value) => Some(*value),
        _ => None,
    }
}

fn is_int(expression: &Expr, value: i64) -> bool {
    const_int(expression) == Some(value)
}

fn is_bool(expression: &Expr, value: bool) -> bool {
    matches!(&expression.kind, ExprKind::Bool(actual) if *actual == value)
}

fn value_expr(value: interp::Value, ty: Type) -> Expr {
    match value {
        interp::Value::Int(value) => int_expr(value),
        interp::Value::Bool(value) => bool_expr(value),
        interp::Value::String(value) => string_expr(&value),
        interp::Value::Unit => Expr {
            ty,
            kind: ExprKind::Int(0),
        },
    }
}

fn int_expr(value: i64) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value),
    }
}

fn bool_expr(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

fn string_expr(value: &str) -> Expr {
    Expr {
        ty: Type::String,
        kind: ExprKind::String(value.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;

    fn program(source: &str) -> Program {
        analyze("rewrite.ry", source).unwrap()
    }

    fn stdout_of(program: &Program) -> Vec<u8> {
        let outcome = interp::execute(program, DEFAULT_STEP_BUDGET);
        assert!(
            matches!(outcome.completion, Completion::Returned(_)),
            "{outcome:?}"
        );
        outcome.stdout
    }

    #[test]
    fn folding_preserves_arithmetic_and_short_circuit() {
        let source = "fn side() -> bool { print(\"x\")\nreturn true }\nfn main() { print(1 + 2 * 3)\nprint(false && side())\nprint(true || side()) }";
        let original = program(source);
        let folded = fold_program(&original);
        assert_eq!(stdout_of(&original), stdout_of(&folded));
        assert_eq!(stdout_of(&folded), b"7\nfalse\ntrue\n");
    }

    #[test]
    fn affine_summaries_match_the_interpreter() {
        let cases = [
            "fn main() { var acc = 0\nfor i in 0..100 { acc += i }\nprint(acc) }",
            "fn main() { var acc = 4\nfor i in 2..80 { acc += i * 3 + 1 }\nprint(acc) }",
            "fn main() { var acc = 0\nfor i in 0..5000 { acc += (i * 3 + 1) % 1024 }\nprint(acc) }",
            "fn main() { var acc = 0\nfor i in 0..300 { acc += i / 7 }\nprint(acc) }",
            "fn main() { var acc = 0\nfor i in 0..10000 { acc += i / 7 }\nprint(acc) }",
            "fn main() { var acc = 0\nfor i in 10..10 { acc += i }\nprint(acc) }",
        ];
        for source in cases {
            let original = program(source);
            let reduced = affine_sums(&fold_program(&original));
            assert_eq!(stdout_of(&original), stdout_of(&reduced), "{source}");
            let serialized = rir::Program::from_hir(&reduced).serialize();
            assert!(!serialized.contains("(for @"), "{source}\n{serialized}");
        }
    }

    #[test]
    fn step_cost_of_a_hot_loop() {
        let source =
            "fn main() { var acc = 0\nfor i in 0..1000 { acc += (i * 3 + 1) % 1024 }\nprint(acc) }";
        let outcome = interp::execute(&program(source), DEFAULT_STEP_BUDGET);
        assert_eq!(outcome.steps, 9_008);
    }

    #[test]
    fn large_closed_forms_match_a_long_interpreter_run() {
        let source = "fn main() { var acc = 0\nfor i in 0..20000 { acc += (i * 3 + 1) % 1024 }\nprint(acc) }";
        let original = program(source);
        let outcome = search(&original, &SearchOptions::default());
        assert_eq!(outcome.pipeline, "affine");
        assert_eq!(stdout_of(&original), stdout_of(&outcome.program));
        assert!(outcome.estimated_iterations < 8);
        assert!(
            !rir::Program::from_hir(&outcome.program)
                .serialize()
                .contains("(for @")
        );
    }

    #[test]
    fn partial_evaluation_keeps_print_order() {
        let source = "fn main() { var acc = 0\nfor i in 0..20 { if i % 2 == 0 { acc += i / 2 } else { acc += 1 } }\nprint(\"acc\", acc) }";
        let original = program(source);
        let outcome = search(
            &original,
            &SearchOptions {
                step_budget: 10_000,
            },
        );
        assert_eq!(outcome.pipeline, "eval");
        assert_eq!(stdout_of(&original), stdout_of(&outcome.program));
        assert_eq!(
            String::from_utf8(stdout_of(&outcome.program)).unwrap(),
            "acc 55\n"
        );
    }

    #[test]
    fn search_rejects_an_over_budget_partial_evaluation() {
        let source = "fn bump(x: int) -> int { if x % 2 == 0 { return x / 2 } else { return x * 3 + 1 } }\nfn main() { var acc = 0\nfor i in 0..5000 { acc += bump(i) }\nprint(acc) }";
        let outcome = search(&program(source), &SearchOptions { step_budget: 100 });
        assert_eq!(outcome.pipeline, "identity");
        assert!(
            outcome
                .candidates
                .iter()
                .any(|candidate| candidate.pipeline == "eval" && candidate.status == "skipped")
        );
    }

    #[test]
    fn native_binary_matches_after_rewriting() {
        let source =
            "fn main() { var acc = 0\nfor i in 0..1000 { acc += (i * 3 + 1) % 128 }\nprint(acc) }";
        let original = program(source);
        let outcome = search(&original, &SearchOptions::default());
        assert!(native_equivalent(&original, &outcome.program).unwrap());
    }
}
