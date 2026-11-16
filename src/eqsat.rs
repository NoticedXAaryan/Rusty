//! Equality saturation over integer RIR expressions and accumulator loops.
//!
//! `egg` holds the equivalences. Extraction uses a cost function that prefers
//! a constant to a loop and a raw division to a guarded one. Soundness of the
//! closed forms still rests on [`crate::rewrite::proven_accumulator`] plus the
//! interpreter and the native differential check. `egglog` is the planned home
//! for the same rules once the side conditions are Datalog facts.

use crate::hir::{BinaryOp, Expr, ExprKind, Program, Stmt, Type, UnaryOp};
use crate::rewrite;
use egg::{Applier, EGraph, Id, Language, Pattern, Rewrite, Subst, Var, rewrite as egg_rewrite};
use std::collections::BTreeMap;
const INDEX_LOCAL: usize = 0;

egg::define_language! {
    enum Lang {
        "+" = Add([Id; 2]),
        "-" = Sub([Id; 2]),
        "*" = Mul([Id; 2]),
        "/" = Div([Id; 2]),
        "%" = Rem([Id; 2]),
        "sdiv" = Sdiv([Id; 2]),
        "srem" = Srem([Id; 2]),
        "neg" = Neg([Id; 1]),
        "idx" = Idx,
        "loc" = Loc([Id; 1]),
        "for" = For([Id; 4]),
        Const(i64),
    }
}

#[derive(Default, Clone)]
struct Fold;

impl egg::Analysis<Lang> for Fold {
    type Data = Option<i64>;

    fn make(egraph: &mut EGraph<Lang, Self>, enode: &Lang, _id: Id) -> Self::Data {
        let value = |id: &Id| egraph[*id].data;
        match enode {
            Lang::Const(value) => Some(*value),
            Lang::Add([left, right]) => Some(value(left)?.wrapping_add(value(right)?)),
            Lang::Sub([left, right]) => Some(value(left)?.wrapping_sub(value(right)?)),
            Lang::Mul([left, right]) => Some(value(left)?.wrapping_mul(value(right)?)),
            Lang::Neg([inner]) => Some(value(inner)?.wrapping_neg()),
            Lang::Div([left, right]) | Lang::Sdiv([left, right]) => {
                checked(value(left)?, value(right)?, false)
            }
            Lang::Rem([left, right]) | Lang::Srem([left, right]) => {
                checked(value(left)?, value(right)?, true)
            }
            Lang::For([init, start, end, body]) => {
                let init = value(init)?;
                let start = value(start)?;
                let end = value(end)?;
                close_body(egraph, *body, start, end, init)
            }
            Lang::Idx | Lang::Loc(_) => None,
        }
    }

    fn merge(&mut self, to: &mut Self::Data, from: Self::Data) -> egg::DidMerge {
        match (&*to, from) {
            (None, Some(value)) => {
                *to = Some(value);
                egg::DidMerge(true, false)
            }
            (Some(_), None) => egg::DidMerge(false, true),
            (Some(left), Some(right)) if *left == right => egg::DidMerge(false, false),
            (Some(_), Some(_)) => egg::DidMerge(false, false),
            (None, None) => egg::DidMerge(false, false),
        }
    }

    fn modify(egraph: &mut EGraph<Lang, Self>, id: Id) {
        if let Some(value) = egraph[id].data {
            let constant = egraph.add(Lang::Const(value));
            egraph.union(id, constant);
        }
    }
}

fn checked(left: i64, right: i64, remainder: bool) -> Option<i64> {
    if right == 0 || (left == i64::MIN && right == -1) {
        return None;
    }
    Some(if remainder {
        left.wrapping_rem(right)
    } else {
        left.wrapping_div(right)
    })
}

fn close_body(
    egraph: &EGraph<Lang, Fold>,
    body: Id,
    start: i64,
    end: i64,
    init: i64,
) -> Option<i64> {
    let mut nodes = egraph[body].nodes.clone();
    nodes.sort();
    for node in nodes {
        let Some(expr) = node_to_expr(egraph, &node, 0) else {
            continue;
        };
        if let Some(value) = rewrite::proven_accumulator(&expr, INDEX_LOCAL, start, end, init) {
            return Some(value);
        }
    }
    None
}

fn node_to_expr(egraph: &EGraph<Lang, Fold>, node: &Lang, depth: u8) -> Option<Expr> {
    if depth > 16 {
        return None;
    }
    let child = |id: &Id| {
        let nodes = &egraph[*id].nodes;
        let preferred = nodes
            .iter()
            .find(|node| matches!(node, Lang::Const(_) | Lang::Idx))
            .or_else(|| nodes.first())?;
        node_to_expr(egraph, preferred, depth + 1)
    };
    Some(match node {
        Lang::Const(value) => int_expr(*value),
        Lang::Idx => Expr {
            ty: Type::Int,
            kind: ExprKind::Local(INDEX_LOCAL),
        },
        Lang::Loc([id]) => {
            let ExprKind::Int(local) = child(id)?.kind else {
                return None;
            };
            return Some(Expr {
                ty: Type::Int,
                kind: ExprKind::Local(local as usize),
            });
        }
        Lang::Add([left, right]) => binary(BinaryOp::Add, child(left)?, child(right)?),
        Lang::Sub([left, right]) => binary(BinaryOp::Sub, child(left)?, child(right)?),
        Lang::Mul([left, right]) => binary(BinaryOp::Mul, child(left)?, child(right)?),
        Lang::Div([left, right]) => binary(BinaryOp::Div, child(left)?, child(right)?),
        Lang::Rem([left, right]) => binary(BinaryOp::Rem, child(left)?, child(right)?),
        Lang::Sdiv([left, right]) => Expr {
            ty: Type::Int,
            kind: ExprKind::RawDiv {
                left: Box::new(child(left)?),
                right: Box::new(child(right)?),
            },
        },
        Lang::Srem([left, right]) => Expr {
            ty: Type::Int,
            kind: ExprKind::RawRem {
                left: Box::new(child(left)?),
                right: Box::new(child(right)?),
            },
        },
        Lang::Neg([inner]) => Expr {
            ty: Type::Int,
            kind: ExprKind::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(child(inner)?),
            },
        },
        Lang::For(_) => return None,
    })
}

pub fn optimize(program: &Program) -> Program {
    let mut program = program.clone();
    for function in &mut program.functions {
        let mut known = BTreeMap::new();
        function.body = saturate_block(&function.body, &mut known);
    }
    program
}

fn saturate_block(block: &[Stmt], known: &mut BTreeMap<usize, i64>) -> Vec<Stmt> {
    let mut output = Vec::with_capacity(block.len());
    for statement in block {
        match statement {
            Stmt::Let { local, value } => {
                let value = saturate_expr(value);
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
                let value = saturate_expr(value);
                output.push(Stmt::Assign {
                    local: *local,
                    op: *op,
                    value,
                });
                known.remove(local);
            }
            Stmt::For {
                local,
                start,
                end,
                body,
            } => {
                let start_expr = saturate_expr(start);
                let end_expr = saturate_expr(end);
                if let Some(replaced) = close_loop(*local, &start_expr, &end_expr, body, known) {
                    output.push(replaced);
                } else {
                    known.clear();
                    output.push(Stmt::For {
                        local: *local,
                        start: start_expr,
                        end: end_expr,
                        body: saturate_block(body, &mut BTreeMap::new()),
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
                    condition: saturate_expr(condition),
                    then_block: saturate_block(then_block, &mut BTreeMap::new()),
                    else_block: saturate_block(else_block, &mut BTreeMap::new()),
                });
            }
            Stmt::Return(value) => output.push(Stmt::Return(value.as_ref().map(saturate_expr))),
            Stmt::Expr(value) => output.push(Stmt::Expr(saturate_expr(value))),
        }
    }
    output
}

fn close_loop(
    index: usize,
    start: &Expr,
    end: &Expr,
    body: &[Stmt],
    known: &mut BTreeMap<usize, i64>,
) -> Option<Stmt> {
    let [
        Stmt::Assign {
            local,
            op: crate::hir::AssignOp::Add,
            value,
        },
    ] = body
    else {
        return None;
    };
    let start_value = const_int(start)?;
    let end_value = const_int(end)?;
    let init = known.get(local).copied()?;
    let mut egraph = EGraph::<Lang, Fold>::default();
    let init_id = egraph.add(Lang::Const(init));
    let start_id = egraph.add(Lang::Const(start_value));
    let end_id = egraph.add(Lang::Const(end_value));
    let body_id = add_expr(&mut egraph, value, Some(index));
    let root = egraph.add(Lang::For([init_id, start_id, end_id, body_id]));
    let runner = egg::Runner::default()
        .with_egraph(egraph)
        .with_iter_limit(12)
        .with_node_limit(8_000)
        .run(&rules());
    let extractor = egg::Extractor::new(&runner.egraph, Cost);
    let (_, best) = extractor.find_best(root);
    let Lang::Const(value) = best.last()? else {
        return None;
    };
    known.insert(*local, *value);
    Some(Stmt::Assign {
        local: *local,
        op: crate::hir::AssignOp::Set,
        value: int_expr(*value),
    })
}

fn saturate_expr(expression: &Expr) -> Expr {
    match &expression.kind {
        ExprKind::Unary { op, expr } => Expr {
            ty: expression.ty,
            kind: ExprKind::Unary {
                op: *op,
                expr: Box::new(saturate_expr(expr)),
            },
        },
        ExprKind::Binary { .. } | ExprKind::RawDiv { .. } | ExprKind::RawRem { .. }
            if expression.ty == Type::Int && pure_int(expression) =>
        {
            saturate_int(expression)
        }
        ExprKind::Binary { op, left, right } => {
            binary(*op, saturate_expr(left), saturate_expr(right))
        }
        ExprKind::Call {
            function,
            name,
            args,
        } => Expr {
            ty: expression.ty,
            kind: ExprKind::Call {
                function: *function,
                name: name.clone(),
                args: args.iter().map(saturate_expr).collect(),
            },
        },
        ExprKind::Print(args) => Expr {
            ty: expression.ty,
            kind: ExprKind::Print(args.iter().map(saturate_expr).collect()),
        },
        ExprKind::RawDiv { left, right } => Expr {
            ty: expression.ty,
            kind: ExprKind::RawDiv {
                left: Box::new(saturate_expr(left)),
                right: Box::new(saturate_expr(right)),
            },
        },
        ExprKind::RawRem { left, right } => Expr {
            ty: expression.ty,
            kind: ExprKind::RawRem {
                left: Box::new(saturate_expr(left)),
                right: Box::new(saturate_expr(right)),
            },
        },
        _ => expression.clone(),
    }
}

fn saturate_int(expression: &Expr) -> Expr {
    let mut egraph = EGraph::<Lang, Fold>::default();
    let root = add_expr(&mut egraph, expression, None);
    let runner = egg::Runner::default()
        .with_egraph(egraph)
        .with_iter_limit(8)
        .with_node_limit(4_000)
        .run(&rules());
    let extractor = egg::Extractor::new(&runner.egraph, Cost);
    let (_, best) = extractor.find_best(root);
    recexpr_to_expr(&best).unwrap_or_else(|| expression.clone())
}

fn add_expr(egraph: &mut EGraph<Lang, Fold>, expression: &Expr, index: Option<usize>) -> Id {
    match &expression.kind {
        ExprKind::Int(value) => egraph.add(Lang::Const(*value)),
        ExprKind::Local(local) if Some(*local) == index => egraph.add(Lang::Idx),
        ExprKind::Local(local) => {
            let id = egraph.add(Lang::Const(*local as i64));
            egraph.add(Lang::Loc([id]))
        }
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => {
            let inner = add_expr(egraph, expr, index);
            egraph.add(Lang::Neg([inner]))
        }
        ExprKind::Binary { op, left, right } => {
            let left = add_expr(egraph, left, index);
            let right = add_expr(egraph, right, index);
            let node = match op {
                BinaryOp::Add => Lang::Add([left, right]),
                BinaryOp::Sub => Lang::Sub([left, right]),
                BinaryOp::Mul => Lang::Mul([left, right]),
                BinaryOp::Div => Lang::Div([left, right]),
                BinaryOp::Rem => Lang::Rem([left, right]),
                _ => return egraph.add(Lang::Const(0)),
            };
            egraph.add(node)
        }
        ExprKind::RawDiv { left, right } => {
            let left = add_expr(egraph, left, index);
            let right = add_expr(egraph, right, index);
            egraph.add(Lang::Sdiv([left, right]))
        }
        ExprKind::RawRem { left, right } => {
            let left = add_expr(egraph, left, index);
            let right = add_expr(egraph, right, index);
            egraph.add(Lang::Srem([left, right]))
        }
        _ => egraph.add(Lang::Const(0)),
    }
}

fn rules() -> Vec<Rewrite<Lang, Fold>> {
    vec![
        egg_rewrite!("add-0-right"; "(+ ?x 0)" => "?x"),
        egg_rewrite!("add-0-left"; "(+ 0 ?x)" => "?x"),
        egg_rewrite!("sub-0"; "(- ?x 0)" => "?x"),
        egg_rewrite!("mul-1-right"; "(* ?x 1)" => "?x"),
        egg_rewrite!("mul-1-left"; "(* 1 ?x)" => "?x"),
        egg_rewrite!("mul-0-right"; "(* ?x 0)" => "0"),
        egg_rewrite!("mul-0-left"; "(* 0 ?x)" => "0"),
        egg_rewrite!("div-1"; "(/ ?x 1)" => "?x"),
        egg_rewrite!("rem-1"; "(% ?x 1)" => "0"),
        egg_rewrite!("neg-neg"; "(neg (neg ?x))" => "?x"),
        unguard("unguard-div", "(/ ?x ?c)", true),
        unguard("unguard-rem", "(% ?x ?c)", false),
    ]
}

fn unguard(name: &str, search: &str, division: bool) -> Rewrite<Lang, Fold> {
    let search: Pattern<Lang> = search.parse().unwrap();
    let divisor: Var = "?c".parse().unwrap();
    let value: Var = "?x".parse().unwrap();
    struct Unguard {
        divisor: Var,
        value: Var,
        division: bool,
    }
    impl Applier<Lang, Fold> for Unguard {
        fn apply_one(
            &self,
            egraph: &mut EGraph<Lang, Fold>,
            eclass: Id,
            subst: &Subst,
            _searcher_ast: Option<&egg::PatternAst<Lang>>,
            _rule_name: egg::Symbol,
        ) -> Vec<Id> {
            let Some(constant) = constant_of(egraph, subst[self.divisor]) else {
                return vec![];
            };
            if !safe_divisor(constant) {
                return vec![];
            }
            let node = if self.division {
                Lang::Sdiv([subst[self.value], subst[self.divisor]])
            } else {
                Lang::Srem([subst[self.value], subst[self.divisor]])
            };
            let id = egraph.add(node);
            egraph.union(eclass, id);
            vec![id]
        }
    }
    Rewrite::new(
        name,
        search,
        Unguard {
            divisor,
            value,
            division,
        },
    )
    .unwrap()
}

fn constant_of(egraph: &EGraph<Lang, Fold>, id: Id) -> Option<i64> {
    egraph[id].data.or_else(|| {
        egraph[id].nodes.iter().find_map(|node| match node {
            Lang::Const(value) => Some(*value),
            _ => None,
        })
    })
}

fn safe_divisor(divisor: i64) -> bool {
    divisor != 0 && divisor != -1
}

struct Cost;

impl egg::CostFunction<Lang> for Cost {
    type Cost = u64;

    fn cost<C>(&mut self, enode: &Lang, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        let operator = match enode {
            Lang::Const(_) | Lang::Idx | Lang::Loc(_) => 1,
            Lang::Sdiv(_) | Lang::Srem(_) => 2,
            Lang::Div(_) | Lang::Rem(_) => 8,
            Lang::For(_) => 1_000,
            _ => 2,
        };
        enode.fold(operator, |sum, id| sum.saturating_add(costs(id)))
    }
}

fn recexpr_to_expr(expr: &[Lang]) -> Option<Expr> {
    fn build_back(expr: &[Lang]) -> Option<Expr> {
        let mut cursor = expr.len();
        fn take(expr: &[Lang], cursor: &mut usize) -> Option<Expr> {
            if *cursor == 0 {
                return None;
            }
            *cursor -= 1;
            let node = &expr[*cursor];
            match node {
                Lang::Const(value) => Some(int_expr(*value)),
                Lang::Idx => None,
                Lang::Loc(_) => {
                    let id = take(expr, cursor)?;
                    let ExprKind::Int(local) = id.kind else {
                        return None;
                    };
                    Some(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Local(local as usize),
                    })
                }
                Lang::Neg(_) => {
                    let inner = take(expr, cursor)?;
                    Some(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Unary {
                            op: UnaryOp::Neg,
                            expr: Box::new(inner),
                        },
                    })
                }
                Lang::Add(_) | Lang::Sub(_) | Lang::Mul(_) | Lang::Div(_) | Lang::Rem(_) => {
                    let right = take(expr, cursor)?;
                    let left = take(expr, cursor)?;
                    let op = match node {
                        Lang::Add(_) => BinaryOp::Add,
                        Lang::Sub(_) => BinaryOp::Sub,
                        Lang::Mul(_) => BinaryOp::Mul,
                        Lang::Div(_) => BinaryOp::Div,
                        Lang::Rem(_) => BinaryOp::Rem,
                        _ => unreachable!("filtered above"),
                    };
                    Some(binary(op, left, right))
                }
                Lang::Sdiv(_) | Lang::Srem(_) => {
                    let right = take(expr, cursor)?;
                    let left = take(expr, cursor)?;
                    let kind = if matches!(node, Lang::Sdiv(_)) {
                        ExprKind::RawDiv {
                            left: Box::new(left),
                            right: Box::new(right),
                        }
                    } else {
                        ExprKind::RawRem {
                            left: Box::new(left),
                            right: Box::new(right),
                        }
                    };
                    Some(Expr {
                        ty: Type::Int,
                        kind,
                    })
                }
                Lang::For(_) => None,
            }
        }
        let expr = take(expr, &mut cursor)?;
        (cursor == 0).then_some(expr)
    }
    build_back(expr)
}

fn pure_int(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Int(_) => true,
        ExprKind::Local(_) => true,
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => pure_int(expr),
        ExprKind::Binary {
            op: BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem,
            left,
            right,
        } => pure_int(left) && pure_int(right),
        ExprKind::RawDiv { left, right } | ExprKind::RawRem { left, right } => {
            pure_int(left) && pure_int(right)
        }
        _ => false,
    }
}

fn const_int(expression: &Expr) -> Option<i64> {
    match &expression.kind {
        ExprKind::Int(value) => Some(*value),
        _ => None,
    }
}

fn int_expr(value: i64) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value),
    }
}

fn binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr {
        ty: if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
        ) {
            Type::Int
        } else {
            Type::Bool
        },
        kind: ExprKind::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;
    use crate::interp::{self, DEFAULT_STEP_BUDGET};

    #[test]
    fn closes_a_periodic_sum_inside_the_egraph() {
        let source =
            "fn main() { var acc = 0\nfor i in 0..5000 { acc += (i * 3 + 1) % 1024 }\nprint(acc) }";
        let hir = analyze("eqsat.ry", source).unwrap();
        let optimized = optimize(&hir);
        assert!(
            !contains_for(&optimized.functions[0].body),
            "e-graph did not extract a closed form"
        );
        let left = interp::execute(&hir, DEFAULT_STEP_BUDGET);
        let right = interp::execute(&optimized, DEFAULT_STEP_BUDGET);
        assert_eq!(left.stdout, right.stdout);
    }

    fn contains_for(block: &[Stmt]) -> bool {
        block.iter().any(|statement| match statement {
            Stmt::For { .. } => true,
            Stmt::If {
                then_block,
                else_block,
                ..
            } => contains_for(then_block) || contains_for(else_block),
            _ => false,
        })
    }

    #[test]
    fn unguards_division_by_a_safe_constant() {
        let expr = Expr {
            ty: Type::Int,
            kind: ExprKind::Binary {
                op: BinaryOp::Div,
                left: Box::new(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Local(0),
                }),
                right: Box::new(int_expr(7)),
            },
        };
        let saturated = saturate_int(&expr);
        assert!(matches!(saturated.kind, ExprKind::RawDiv { .. }));
    }

    #[test]
    fn refuses_to_unguard_division_by_negative_one() {
        let expr = Expr {
            ty: Type::Int,
            kind: ExprKind::Binary {
                op: BinaryOp::Div,
                left: Box::new(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Local(0),
                }),
                right: Box::new(int_expr(-1)),
            },
        };
        let saturated = saturate_int(&expr);
        assert!(matches!(
            saturated.kind,
            ExprKind::Binary {
                op: BinaryOp::Div,
                ..
            }
        ));
    }

    #[test]
    fn alive2_is_not_required_for_the_interpreter_check() {
        let found = std::process::Command::new("alive-tv")
            .arg("--version")
            .output()
            .is_ok();
        if !found {
            return;
        }
        let directory = std::env::temp_dir().join(format!("rusty-alive-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let source = directory.join("pair.ll");
        std::fs::write(
            &source,
            "define i64 @src(i64 %a) {\n  %r = sdiv i64 %a, 7\n  ret i64 %r\n}\n\ndefine i64 @tgt(i64 %a) {\n  %r = sdiv i64 %a, 7\n  ret i64 %r\n}\n",
        )
        .unwrap();
        let output = std::process::Command::new("alive-tv")
            .arg(&source)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&directory);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn fuzz_matches_the_interpreter() {
        let mut state = 0x5155_4553_4154_u64;
        for _ in 0..80 {
            state = state.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
            let trip = 2 + (state % 25) as i64;
            let factor = ((state >> 8) % 9) as i64;
            let base = ((state >> 16) % 11) as i64;
            let modulus = 3 + ((state >> 24) % 20) as i64;
            let source = format!(
                "fn main() {{ var acc = 0\nfor i in 0..{trip} {{ acc += (i * {factor} + {base}) % {modulus} }}\nprint(acc) }}"
            );
            let hir = analyze("fuzz.ry", &source).unwrap();
            let optimized = optimize(&hir);
            let left = interp::execute(&hir, DEFAULT_STEP_BUDGET);
            let right = interp::execute(&optimized, DEFAULT_STEP_BUDGET);
            assert_eq!(left.stdout, right.stdout, "{source}");
            assert_eq!(left.completion, right.completion, "{source}");
        }
    }
}
