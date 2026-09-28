//! Pest-based DPath parser converting text expressions into canonical AST nodes.
//!
//! Handles operator precedence, functions, type constructors, variables, paths, and literals.

use crate::expression::ast::{BinaryOpKind, Expr, FuncKind, SimpleType, UnaryOpKind};
use crate::expression::error::{ExpressionError, Result};
use crate::expression::path::{Path, PathStep, StepTest};
use crate::value::{DfdlValue, StringValue};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use pest::Parser;
use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "expression/grammar.pest"]
pub struct DPathParser;

/// Parses a DPath expression string into a canonical `Expr` AST.
pub fn parse_expression(input: &str) -> Result<Expr> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ExpressionError::ParseError("empty expression".into()));
    }
    let pairs = DPathParser::parse(Rule::main, trimmed)
        .map_err(|e| ExpressionError::ParseError(format!("Pest parse error: {e}")))?;
    
    let first = pairs
        .into_iter()
        .next()
        .ok_or_else(|| ExpressionError::ParseError("no parse pair produced".into()))?;

    build_expr(first)
}

fn build_expr(pair: pest::iterators::Pair<Rule>) -> Result<Expr> {
    match pair.as_rule() {
        Rule::main => {
            let expr_pair = pair
                .into_inner()
                .find(|p| p.as_rule() == Rule::expr)
                .ok_or_else(|| ExpressionError::ParseError("empty expr pair".into()))?;
            build_expr(expr_pair)
        }
        Rule::expr => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| ExpressionError::ParseError("empty expr pair".into()))?;
            build_expr(inner)
        }
        Rule::if_expr => {
            let mut inner = pair.into_inner();
            let cond = build_expr(
                inner
                    .next()
                    .ok_or_else(|| ExpressionError::ParseError("missing if condition".into()))?,
            )?;
            let then_branch = build_expr(
                inner
                    .next()
                    .ok_or_else(|| ExpressionError::ParseError("missing then branch".into()))?,
            )?;
            let else_branch = build_expr(
                inner
                    .next()
                    .ok_or_else(|| ExpressionError::ParseError("missing else branch".into()))?,
            )?;
            Ok(Expr::IfThenElse {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: Box::new(else_branch),
            })
        }
        Rule::or_expr => {
            let mut inner = pair.into_inner();
            let mut left = build_expr(
                inner
                    .next()
                    .ok_or_else(|| ExpressionError::ParseError("missing lhs for or".into()))?,
            )?;
            while let Some(_op) = inner.next() {
                let right = build_expr(
                    inner
                        .next()
                        .ok_or_else(|| ExpressionError::ParseError("missing rhs for or".into()))?,
                )?;
                left = Expr::BinaryOp {
                    op: BinaryOpKind::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }
        Rule::and_expr => {
            let mut inner = pair.into_inner();
            let mut left = build_expr(
                inner
                    .next()
                    .ok_or_else(|| ExpressionError::ParseError("missing lhs for and".into()))?,
            )?;
            while let Some(_op) = inner.next() {
                let right = build_expr(
                    inner
                        .next()
                        .ok_or_else(|| ExpressionError::ParseError("missing rhs for and".into()))?,
                )?;
                left = Expr::BinaryOp {
                    op: BinaryOpKind::And,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }
        Rule::comp_expr => {
            let mut inner = pair.into_inner();
            let mut left = build_expr(
                inner.next().ok_or_else(|| {
                    ExpressionError::ParseError("missing lhs for comparison".into())
                })?,
            )?;
            if let Some(op_pair) = inner.next() {
                let op = match op_pair.as_str() {
                    "=" | "eq" => BinaryOpKind::Eq,
                    "!=" | "ne" => BinaryOpKind::Ne,
                    "<" | "lt" => BinaryOpKind::Lt,
                    "<=" | "le" => BinaryOpKind::Le,
                    ">" | "gt" => BinaryOpKind::Gt,
                    ">=" | "ge" => BinaryOpKind::Ge,
                    other => {
                        return Err(ExpressionError::ParseError(format!(
                            "unknown comp op `{other}`"
                        )))
                    }
                };
                let right = build_expr(inner.next().ok_or_else(|| {
                    ExpressionError::ParseError("missing rhs for comparison".into())
                })?)?;
                left = Expr::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }
        Rule::add_expr => {
            let mut inner = pair.into_inner();
            let mut left = build_expr(
                inner
                    .next()
                    .ok_or_else(|| ExpressionError::ParseError("missing lhs for add".into()))?,
            )?;
            while let Some(op_pair) = inner.next() {
                let op = match op_pair.as_str() {
                    "+" => BinaryOpKind::Add,
                    "-" => BinaryOpKind::Sub,
                    other => {
                        return Err(ExpressionError::ParseError(format!(
                            "unknown add op `{other}`"
                        )))
                    }
                };
                let right = build_expr(
                    inner
                        .next()
                        .ok_or_else(|| ExpressionError::ParseError("missing rhs for add".into()))?,
                )?;
                left = Expr::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }
        Rule::mult_expr => {
            let mut inner = pair.into_inner();
            let mut left = build_expr(
                inner
                    .next()
                    .ok_or_else(|| ExpressionError::ParseError("missing lhs for mult".into()))?,
            )?;
            while let Some(op_pair) = inner.next() {
                let op = match op_pair.as_str() {
                    "*" => BinaryOpKind::Mul,
                    "div" => BinaryOpKind::Div,
                    "mod" => BinaryOpKind::Mod,
                    other => {
                        return Err(ExpressionError::ParseError(format!(
                            "unknown mult op `{other}`"
                        )))
                    }
                };
                let right = build_expr(inner.next().ok_or_else(|| {
                    ExpressionError::ParseError("missing rhs for mult".into())
                })?)?;
                left = Expr::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }
        Rule::unary_expr => {
            let mut inner = pair.into_inner().collect::<Vec<_>>();
            let primary_pair = inner
                .pop()
                .ok_or_else(|| ExpressionError::ParseError("missing primary expr".into()))?;
            let mut expr = build_expr(primary_pair)?;
            while let Some(op_pair) = inner.pop() {
                let op = match op_pair.as_str() {
                    "+" => UnaryOpKind::Plus,
                    "-" => UnaryOpKind::Minus,
                    other => {
                        return Err(ExpressionError::ParseError(format!(
                            "unknown unary op `{other}`"
                        )))
                    }
                };
                expr = Expr::UnaryOp {
                    op,
                    expr: Box::new(expr),
                };
            }
            Ok(expr)
        }
        Rule::primary_expr => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| ExpressionError::ParseError("empty primary expr".into()))?;
            build_expr(inner)
        }
        Rule::paren_expr => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| ExpressionError::ParseError("empty paren expr".into()))?;
            build_expr(inner)
        }
        Rule::func_call => {
            let mut inner = pair.into_inner();
            let qname_str = inner
                .next()
                .ok_or_else(|| ExpressionError::ParseError("missing function qname".into()))?
                .as_str();
            let mut args = Vec::new();
            for arg_pair in inner {
                args.push(build_expr(arg_pair)?);
            }
            
            // Check if function name matches simple type constructor (xs:int, etc.)
            if let Some(cast_type) = parse_simple_type(qname_str) {
                if args.len() != 1 {
                    return Err(ExpressionError::InvalidArgumentCount {
                        expected: 1,
                        found: args.len(),
                    });
                }
                let arg = args.pop().ok_or_else(|| {
                    ExpressionError::ParseError("missing argument for cast".into())
                })?;
                return Ok(Expr::Cast {
                    target_type: cast_type,
                    expr: Box::new(arg),
                });
            }

            let func = match qname_str {
                "fn:count" => FuncKind::FnCount,
                "fn:concat" => FuncKind::FnConcat,
                "fn:substring" => FuncKind::FnSubstring,
                "fn:exists" => FuncKind::FnExists,
                "fn:empty" => FuncKind::FnEmpty,
                "fn:not" => FuncKind::FnNot,
                "fn:nillable" => FuncKind::FnNillable,
                "dfdl:occursIndex" => FuncKind::DfdlOccursIndex,
                "dfdl:valueLength" => FuncKind::DfdlValueLength,
                "dfdl:contentLength" => FuncKind::DfdlContentLength,
                other => FuncKind::Other(other.to_string()),
            };

            Ok(Expr::FunctionCall { func, args })
        }
        Rule::var_ref => {
            let qname_str = pair
                .into_inner()
                .next()
                .ok_or_else(|| ExpressionError::ParseError("missing variable qname".into()))?
                .as_str();
            let (prefix, local) = split_qname(qname_str);
            Ok(Expr::Variable { prefix, local })
        }
        Rule::path_expr => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| ExpressionError::ParseError("empty path expr".into()))?;
            build_path_expr(inner)
        }
        Rule::literal => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| ExpressionError::ParseError("empty literal".into()))?;
            match inner.as_rule() {
                Rule::string_literal => {
                    let str_inner = inner
                        .into_inner()
                        .next()
                        .ok_or_else(|| ExpressionError::ParseError("empty string literal".into()))?;
                    let raw = str_inner.as_str();
                    let content = &raw[1..raw.len() - 1];
                    Ok(Expr::Literal(DfdlValue::String(StringValue::new(
                        content.to_string(),
                    ))))
                }
                Rule::number_literal => {
                    let text = inner.as_str();
                    if text.contains('.') {
                        let val: f64 = text.parse().map_err(|_| {
                            ExpressionError::ParseError(format!("invalid float literal `{text}`"))
                        })?;
                        Ok(Expr::Literal(DfdlValue::Double(val)))
                    } else {
                        if let Ok(v32) = text.parse::<i32>() {
                            Ok(Expr::Literal(DfdlValue::Int(v32)))
                        } else if let Ok(val) = text.parse::<i64>() {
                            Ok(Expr::Literal(DfdlValue::Long(val)))
                        } else {
                            Ok(Expr::Literal(DfdlValue::Integer(text.to_string())))
                        }
                    }
                }
                Rule::bool_literal => {
                    let val = inner.as_str() == "true";
                    Ok(Expr::Literal(DfdlValue::Boolean(val)))
                }
                _ => Err(ExpressionError::ParseError(format!(
                    "unexpected literal rule `{:?}`",
                    inner.as_rule()
                ))),
            }
        }
        _ => Err(ExpressionError::ParseError(format!(
            "unexpected rule `{:?}`",
            pair.as_rule()
        ))),
    }
}

fn build_path_expr(pair: pest::iterators::Pair<Rule>) -> Result<Expr> {
    match pair.as_rule() {
        Rule::root_path => {
            let mut steps = Vec::new();
            for inner in pair.into_inner() {
                if inner.as_rule() == Rule::rel_path {
                    steps.extend(parse_rel_path_steps(inner)?);
                }
            }
            Ok(Expr::Path(Path::root(steps)))
        }
        Rule::rel_path => {
            let steps = parse_rel_path_steps(pair)?;
            
            // Check if relative path starts with '..' step(s)
            let mut up = 0usize;
            let mut remaining_steps = Vec::new();
            for step in steps {
                if step.test == (StepTest::Name { prefix: None, local: String::from("..") }) && step.predicate.is_none() {
                    up += 1;
                } else {
                    remaining_steps.push(step);
                }
            }

            if up > 0 {
                Ok(Expr::Path(Path::parent(up, remaining_steps)))
            } else {
                Ok(Expr::Path(Path::relative(remaining_steps)))
            }
        }
        _ => Err(ExpressionError::ParseError(format!(
            "unexpected path rule `{:?}`",
            pair.as_rule()
        ))),
    }
}

fn parse_rel_path_steps(pair: pest::iterators::Pair<Rule>) -> Result<Vec<PathStep>> {
    let mut steps = Vec::new();
    for step_pair in pair.into_inner() {
        if step_pair.as_rule() == Rule::path_step {
            steps.push(parse_path_step(step_pair)?);
        }
    }
    Ok(steps)
}

fn parse_path_step(pair: pest::iterators::Pair<Rule>) -> Result<PathStep> {
    let mut test = StepTest::SelfNode;
    let mut predicate = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::axis => {
                let axis_str = inner.as_str();
                if axis_str == "@" || axis_str == "attribute::" {
                    // Attribute axis follows next
                }
            }
            Rule::step_test => {
                let s = inner.as_str();
                test = if s == ".." {
                    StepTest::Name {
                        prefix: None,
                        local: "..".to_string(),
                    }
                } else if s == "." {
                    StepTest::SelfNode
                } else if s == "*" {
                    StepTest::Wildcard
                } else {
                    let (prefix, local) = split_qname(s);
                    StepTest::Name { prefix, local }
                };
            }
            Rule::predicate => {
                let pred_inner = inner
                    .into_inner()
                    .next()
                    .ok_or_else(|| ExpressionError::ParseError("empty predicate".into()))?;
                predicate = Some(Box::new(build_expr(pred_inner)?));
            }
            _ => {}
        }
    }

    Ok(PathStep { test, predicate })
}

fn split_qname(qname: &str) -> (Option<String>, String) {
    if let Some((p, l)) = qname.split_once(':') {
        (Some(p.to_string()), l.to_string())
    } else {
        (None, qname.to_string())
    }
}

fn parse_simple_type(name: &str) -> Option<SimpleType> {
    let local = if let Some((_, l)) = name.split_once(':') {
        l
    } else {
        name
    };
    match local {
        "string" => Some(SimpleType::String),
        "int" => Some(SimpleType::Int),
        "integer" => Some(SimpleType::Integer),
        "long" => Some(SimpleType::Long),
        "short" => Some(SimpleType::Short),
        "byte" => Some(SimpleType::Byte),
        "unsignedInt" => Some(SimpleType::UnsignedInt),
        "unsignedLong" => Some(SimpleType::UnsignedLong),
        "unsignedShort" => Some(SimpleType::UnsignedShort),
        "unsignedByte" => Some(SimpleType::UnsignedByte),
        "double" => Some(SimpleType::Double),
        "float" => Some(SimpleType::Float),
        "boolean" => Some(SimpleType::Boolean),
        "decimal" => Some(SimpleType::Decimal),
        _ => None,
    }
}
