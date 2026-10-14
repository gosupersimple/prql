//! Separates conditions from values for dialects without a boolean data type (MS SQL).
//!
//! In T-SQL, a condition such as `a > 1` is only valid where a condition is expected (WHERE, ON,
//! HAVING, CASE WHEN), and a value only where a value is expected. Boolean values are bits.
//! - A condition used as a value becomes `CASE WHEN a > 1 THEN 1 WHEN NOT a > 1 THEN 0 END`,
//!   which keeps NULL for an unknown condition.
//! - A value used as a condition (a bit column, a CASE expression) is compared to 1.
//!
//! S-strings are opaque: they are kept as they are in both contexts, while expressions
//! interpolated into them are treated as values.

use super::ast::SqlTransform;
use crate::ir::pl::Literal;
use crate::ir::rq::{self, new_binop, Compute, Expr, ExprKind, RqFold, Transform};
use crate::Result;
use prqlc_parser::generic::{InterpolateItem, SwitchCase};

const CONDITION_OPERATORS: &[&str] = &[
    "std.eq",
    "std.ne",
    "std.gt",
    "std.lt",
    "std.gte",
    "std.lte",
    "std.and",
    "std.or",
    "std.not",
    "std.array_in",
    "std.regex_search",
    "std.text.starts_with",
    "std.text.contains",
    "std.text.ends_with",
];

const LOGICAL_OPERATORS: &[&str] = &["std.and", "std.or", "std.not"];

/// Separates conditions from values in the whole query, before columns are anchored.
pub(in crate::sql) struct SeparateConditions;

impl RqFold for SeparateConditions {
    fn fold_transform(&mut self, transform: Transform) -> Result<Transform> {
        Ok(match rq::fold_transform(self, transform)? {
            Transform::Compute(compute) => Transform::Compute(Compute {
                expr: value(compute.expr),
                ..compute
            }),
            Transform::Filter(expr) => Transform::Filter(condition(expr)),
            Transform::Join { side, with, filter } => Transform::Join {
                side,
                with,
                filter: condition(filter),
            },
            transform => transform,
        })
    }
}

/// Separates conditions in filters that preprocessing adds to a pipeline.
pub(in crate::sql) fn separate_pipeline_conditions(
    pipeline: Vec<SqlTransform>,
) -> Vec<SqlTransform> {
    pipeline
        .into_iter()
        .map(|transform| match transform {
            SqlTransform::Super(Transform::Filter(expr)) => {
                SqlTransform::Super(Transform::Filter(condition(expr)))
            }
            SqlTransform::Filter(expr) => SqlTransform::Filter(condition(expr)),
            SqlTransform::Join { side, with, filter } => SqlTransform::Join {
                side,
                with,
                filter: condition(filter),
            },
            transform => transform,
        })
        .collect()
}

/// Rewrites an expression that is used as a value.
fn value(expr: Expr) -> Expr {
    let span = expr.span;
    let kind = match expr.kind {
        ExprKind::Literal(Literal::Boolean(b)) => return bit(b),
        ExprKind::Operator { ref name, .. } if CONDITION_OPERATORS.contains(&name.as_str()) => {
            return condition_to_bit(condition(expr));
        }
        ExprKind::Operator { name, args } => ExprKind::Operator {
            name,
            args: args.into_iter().map(value).collect(),
        },
        ExprKind::Case(cases) => ExprKind::Case(switch_cases(cases)),
        ExprKind::SString(items) => ExprKind::SString(interpolations(items)),
        ExprKind::Array(items) => ExprKind::Array(items.into_iter().map(value).collect()),
        kind => kind,
    };
    Expr { kind, span }
}

/// Rewrites an expression that is used as a condition.
fn condition(expr: Expr) -> Expr {
    let span = expr.span;
    match expr.kind {
        ExprKind::Literal(Literal::Boolean(b)) => {
            new_binop(int(1), "std.eq", int(if b { 1 } else { 0 }))
        }
        ExprKind::Operator { name, args } if LOGICAL_OPERATORS.contains(&name.as_str()) => Expr {
            kind: ExprKind::Operator {
                name,
                args: args.into_iter().map(condition).collect(),
            },
            span,
        },
        ExprKind::Operator { name, args } if CONDITION_OPERATORS.contains(&name.as_str()) => Expr {
            kind: ExprKind::Operator {
                name,
                args: args.into_iter().map(value).collect(),
            },
            span,
        },
        ExprKind::SString(items) => Expr {
            kind: ExprKind::SString(interpolations(items)),
            span,
        },
        kind => new_binop(value(Expr { kind, span }), "std.eq", int(1)),
    }
}

fn switch_cases(cases: Vec<SwitchCase<Expr>>) -> Vec<SwitchCase<Expr>> {
    let count = cases.len();
    cases
        .into_iter()
        .enumerate()
        .map(|(index, case)| {
            // A trailing `true` condition is the ELSE branch, which SQL generation detects by its literal.
            let is_default = index + 1 == count
                && matches!(
                    case.condition.kind,
                    ExprKind::Literal(Literal::Boolean(true))
                );
            SwitchCase {
                condition: if is_default {
                    case.condition
                } else {
                    condition(case.condition)
                },
                value: value(case.value),
            }
        })
        .collect()
}

fn interpolations(items: Vec<InterpolateItem<Expr>>) -> Vec<InterpolateItem<Expr>> {
    items
        .into_iter()
        .map(|item| match item {
            InterpolateItem::Expr { expr, format } => InterpolateItem::Expr {
                expr: Box::new(value(*expr)),
                format,
            },
            item => item,
        })
        .collect()
}

fn condition_to_bit(condition: Expr) -> Expr {
    let otherwise = if can_be_null(&condition) {
        new_unary_not(condition.clone())
    } else {
        Expr {
            kind: ExprKind::Literal(Literal::Boolean(true)),
            span: None,
        }
    };

    Expr {
        kind: ExprKind::Case(vec![
            SwitchCase {
                condition,
                value: bit(true),
            },
            SwitchCase {
                condition: otherwise,
                value: bit(false),
            },
        ]),
        span: None,
    }
}

/// Whether a condition can evaluate to unknown. IS NULL checks cannot.
fn can_be_null(condition: &Expr) -> bool {
    match &condition.kind {
        ExprKind::Operator { name, args } if name == "std.eq" || name == "std.ne" => !args
            .iter()
            .any(|arg| matches!(arg.kind, ExprKind::Literal(Literal::Null))),
        _ => true,
    }
}

fn new_unary_not(expr: Expr) -> Expr {
    Expr {
        kind: ExprKind::Operator {
            name: "std.not".to_string(),
            args: vec![expr],
        },
        span: None,
    }
}

fn bit(b: bool) -> Expr {
    Expr {
        kind: ExprKind::SString(vec![InterpolateItem::String(format!(
            "CAST({} AS BIT)",
            if b { 1 } else { 0 }
        ))]),
        span: None,
    }
}

fn int(i: i64) -> Expr {
    Expr {
        kind: ExprKind::Literal(Literal::Integer(i)),
        span: None,
    }
}
