//! Centralized DPath expression evaluator.
//!
//! Evaluates AST nodes against an `EvalContext`, implementing standard arithmetic,
//! comparisons, boolean logic, explicit type casts, standard function calls, and if-then-else.

use crate::expression::ast::{BinaryOpKind, Expr, FuncKind, SimpleType, UnaryOpKind};
use crate::expression::context::EvalContext;
use crate::expression::error::{ExpressionError, Result};
use crate::value::{DfdlValue, StringValue};
use alloc::format;
use alloc::string::{String, ToString};

/// Main evaluation entry point for any DPath AST node.
pub fn eval(expr: &Expr, ctx: &dyn EvalContext) -> Result<DfdlValue> {
    match expr {
        Expr::Literal(val) => Ok(val.clone()),
        Expr::Path(path) => ctx.resolve_path(path),
        Expr::Variable { prefix, local } => ctx.get_variable(prefix.as_deref(), local),
        Expr::UnaryOp { op, expr } => {
            let val = eval(expr, ctx)?;
            eval_unary_op(*op, val)
        }
        Expr::BinaryOp { op, left, right } => {
            let left_val = eval(left, ctx)?;
            let right_val = eval(right, ctx)?;
            eval_binary_op(*op, left_val, right_val)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let cond_val = eval(cond, ctx)?;
            if to_boolean(&cond_val)? {
                eval(then_branch, ctx)
            } else {
                eval(else_branch, ctx)
            }
        }
        Expr::Cast { target_type, expr } => {
            let val = eval(expr, ctx)?;
            cast_value(val, *target_type)
        }
        Expr::FunctionCall { func, args } => eval_function(func, args, ctx),
    }
}

fn eval_unary_op(op: UnaryOpKind, val: DfdlValue) -> Result<DfdlValue> {
    match op {
        UnaryOpKind::Plus => Ok(val),
        UnaryOpKind::Minus => match val {
            DfdlValue::Int(n) => Ok(DfdlValue::Int(-n)),
            DfdlValue::Long(n) => Ok(DfdlValue::Long(-n)),
            DfdlValue::Short(n) => Ok(DfdlValue::Short(-n)),
            DfdlValue::Byte(n) => Ok(DfdlValue::Byte(-n)),
            DfdlValue::Double(f) => Ok(DfdlValue::Double(-f)),
            DfdlValue::Float(f) => Ok(DfdlValue::Float(-f)),
            _ => Err(ExpressionError::TypeMismatch(format!(
                "cannot negate non-numeric value `{val:?}`"
            ))),
        },
        UnaryOpKind::Not => Ok(DfdlValue::Boolean(!to_boolean(&val)?)),
    }
}

fn eval_binary_op(op: BinaryOpKind, left: DfdlValue, right: DfdlValue) -> Result<DfdlValue> {
    match op {
        BinaryOpKind::Add => arithmetic_op(left, right, |a, b| a + b, |a, b| a + b),
        BinaryOpKind::Sub => arithmetic_op(left, right, |a, b| a - b, |a, b| a - b),
        BinaryOpKind::Mul => arithmetic_op(left, right, |a, b| a * b, |a, b| a * b),
        BinaryOpKind::Div => arithmetic_op(
            left,
            right,
            |a, b| if b == 0 { 0 } else { a / b },
            |a, b| a / b,
        ),
        BinaryOpKind::Mod => arithmetic_op(
            left,
            right,
            |a, b| if b == 0 { 0 } else { a % b },
            |a, b| a % b,
        ),
        BinaryOpKind::Eq => Ok(DfdlValue::Boolean(compare_values(&left, &right)? == 0)),
        BinaryOpKind::Ne => Ok(DfdlValue::Boolean(compare_values(&left, &right)? != 0)),
        BinaryOpKind::Lt => Ok(DfdlValue::Boolean(compare_values(&left, &right)? < 0)),
        BinaryOpKind::Le => Ok(DfdlValue::Boolean(compare_values(&left, &right)? <= 0)),
        BinaryOpKind::Gt => Ok(DfdlValue::Boolean(compare_values(&left, &right)? > 0)),
        BinaryOpKind::Ge => Ok(DfdlValue::Boolean(compare_values(&left, &right)? >= 0)),
        BinaryOpKind::And => {
            let b1 = to_boolean(&left)?;
            let b2 = to_boolean(&right)?;
            Ok(DfdlValue::Boolean(b1 && b2))
        }
        BinaryOpKind::Or => {
            let b1 = to_boolean(&left)?;
            let b2 = to_boolean(&right)?;
            Ok(DfdlValue::Boolean(b1 || b2))
        }
    }
}

fn arithmetic_op<FI, FF>(
    left: DfdlValue,
    right: DfdlValue,
    int_op: FI,
    float_op: FF,
) -> Result<DfdlValue>
where
    FI: Fn(i64, i64) -> i64,
    FF: Fn(f64, f64) -> f64,
{
    match (left, right) {
        (DfdlValue::Int(a), DfdlValue::Int(b)) => Ok(DfdlValue::Int(int_op(a as i64, b as i64) as i32)),
        (DfdlValue::Long(a), DfdlValue::Long(b)) => Ok(DfdlValue::Long(int_op(a, b))),
        (DfdlValue::Double(a), DfdlValue::Double(b)) => Ok(DfdlValue::Double(float_op(a, b))),
        (DfdlValue::Float(a), DfdlValue::Float(b)) => Ok(DfdlValue::Float(float_op(a as f64, b as f64) as f32)),
        (DfdlValue::Int(a), DfdlValue::Double(b)) => Ok(DfdlValue::Double(float_op(a as f64, b))),
        (DfdlValue::Double(a), DfdlValue::Int(b)) => Ok(DfdlValue::Double(float_op(a, b as f64))),
        (l, r) => Err(ExpressionError::TypeMismatch(format!(
            "invalid operands for arithmetic operation: `{l:?}` and `{r:?}`"
        ))),
    }
}

fn compare_values(left: &DfdlValue, right: &DfdlValue) -> Result<i32> {
    match (left, right) {
        (DfdlValue::Int(a), DfdlValue::Int(b)) => Ok(a.cmp(b) as i32),
        (DfdlValue::Long(a), DfdlValue::Long(b)) => Ok(a.cmp(b) as i32),
        (DfdlValue::UnsignedInt(a), DfdlValue::UnsignedInt(b)) => Ok(a.cmp(b) as i32),
        (DfdlValue::Double(a), DfdlValue::Double(b)) => {
            if a < b {
                Ok(-1)
            } else if a > b {
                Ok(1)
            } else {
                Ok(0)
            }
        }
        (DfdlValue::Boolean(a), DfdlValue::Boolean(b)) => Ok(a.cmp(b) as i32),
        (DfdlValue::String(a), DfdlValue::String(b)) => Ok(a.text.cmp(&b.text) as i32),
        (DfdlValue::String(a), DfdlValue::Int(b)) => {
            if let Ok(n) = a.text.parse::<i32>() {
                Ok(n.cmp(b) as i32)
            } else {
                Ok(a.text.cmp(&b.to_string()) as i32)
            }
        }
        (DfdlValue::Int(a), DfdlValue::String(b)) => {
            if let Ok(n) = b.text.parse::<i32>() {
                Ok(a.cmp(&n) as i32)
            } else {
                Ok(a.to_string().cmp(&b.text) as i32)
            }
        }
        _ => Err(ExpressionError::TypeMismatch(format!(
            "cannot compare `{left:?}` and `{right:?}`"
        ))),
    }
}

pub fn to_boolean(val: &DfdlValue) -> Result<bool> {
    match val {
        DfdlValue::Boolean(b) => Ok(*b),
        DfdlValue::Int(n) => Ok(*n != 0),
        DfdlValue::Long(n) => Ok(*n != 0),
        DfdlValue::UnsignedInt(n) => Ok(*n != 0),
        DfdlValue::String(s) => match s.text.trim() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(ExpressionError::TypeMismatch(format!(
                "cannot convert string `{}` to boolean",
                s.text
            ))),
        },
        _ => Err(ExpressionError::TypeMismatch(format!(
            "cannot convert value to boolean: `{val:?}`"
        ))),
    }
}

fn cast_value(val: DfdlValue, target: SimpleType) -> Result<DfdlValue> {
    match target {
        SimpleType::Int => match val {
            DfdlValue::Int(n) => Ok(DfdlValue::Int(n)),
            DfdlValue::Long(n) => Ok(DfdlValue::Int(n as i32)),
            DfdlValue::Double(f) => Ok(DfdlValue::Int(f as i32)),
            DfdlValue::String(s) => {
                let n = s.text.trim().parse::<i32>().map_err(|_| {
                    ExpressionError::InvalidCast(format!("cannot convert '{}' to xs:Int", s.text))
                })?;
                Ok(DfdlValue::Int(n))
            }
            DfdlValue::Boolean(b) => Ok(DfdlValue::Int(if b { 1 } else { 0 })),
            _ => Err(ExpressionError::InvalidCast(format!(
                "cannot cast `{val:?}` to xs:Int"
            ))),
        },
        SimpleType::Double => match val {
            DfdlValue::Double(f) => Ok(DfdlValue::Double(f)),
            DfdlValue::Int(n) => Ok(DfdlValue::Double(n as f64)),
            DfdlValue::Long(n) => Ok(DfdlValue::Double(n as f64)),
            DfdlValue::String(s) => {
                let f = s.text.trim().parse::<f64>().map_err(|_| {
                    ExpressionError::InvalidCast(format!("cannot convert '{}' to xs:Double", s.text))
                })?;
                Ok(DfdlValue::Double(f))
            }
            _ => Err(ExpressionError::InvalidCast(format!(
                "cannot cast `{val:?}` to xs:Double"
            ))),
        },
        SimpleType::String => match val {
            DfdlValue::String(s) => Ok(DfdlValue::String(s)),
            DfdlValue::Int(n) => Ok(DfdlValue::String(StringValue::new(n.to_string()))),
            DfdlValue::Long(n) => Ok(DfdlValue::String(StringValue::new(n.to_string()))),
            DfdlValue::Double(f) => Ok(DfdlValue::String(StringValue::new(f.to_string()))),
            DfdlValue::Boolean(b) => Ok(DfdlValue::String(StringValue::new(b.to_string()))),
            _ => Ok(DfdlValue::String(StringValue::new(format!("{val:?}")))),
        },
        SimpleType::Boolean => {
            let b = to_boolean(&val)?;
            Ok(DfdlValue::Boolean(b))
        }
        _ => Ok(val),
    }
}

fn eval_function(func: &FuncKind, args: &[Expr], ctx: &dyn EvalContext) -> Result<DfdlValue> {
    match func {
        FuncKind::DfdlOccursIndex => {
            if !args.is_empty() {
                return Err(ExpressionError::InvalidArgumentCount {
                    expected: 0,
                    found: args.len(),
                });
            }
            let idx = ctx.occurs_index()?;
            Ok(DfdlValue::Int(idx as i32))
        }
        FuncKind::DfdlValueLength => {
            let path = if let Some(arg) = args.first() {
                if let Expr::Path(p) = arg {
                    Some(p)
                } else {
                    None
                }
            } else {
                None
            };
            let len = ctx.value_length(path)?;
            Ok(DfdlValue::Int(len as i32))
        }
        FuncKind::DfdlContentLength => {
            let path = if let Some(arg) = args.first() {
                if let Expr::Path(p) = arg {
                    Some(p)
                } else {
                    None
                }
            } else {
                None
            };
            let len = ctx.content_length(path)?;
            Ok(DfdlValue::Int(len as i32))
        }
        FuncKind::FnCount => {
            if args.len() != 1 {
                return Err(ExpressionError::InvalidArgumentCount {
                    expected: 1,
                    found: args.len(),
                });
            }
            let arg_val = eval(&args[0], ctx)?;
            match arg_val {
                DfdlValue::Array(items) => Ok(DfdlValue::Int(items.len() as i32)),
                DfdlValue::Sequence(_) => Ok(DfdlValue::Int(1)),
                _ => Ok(DfdlValue::Int(1)),
            }
        }
        FuncKind::FnConcat => {
            let mut out = String::new();
            for arg in args {
                let val = eval(arg, ctx)?;
                match cast_value(val, SimpleType::String)? {
                    DfdlValue::String(s) => out.push_str(&s.text),
                    _ => {}
                }
            }
            Ok(DfdlValue::String(StringValue::new(out)))
        }
        FuncKind::FnNot => {
            if args.len() != 1 {
                return Err(ExpressionError::InvalidArgumentCount {
                    expected: 1,
                    found: args.len(),
                });
            }
            let val = eval(&args[0], ctx)?;
            Ok(DfdlValue::Boolean(!to_boolean(&val)?))
        }
        FuncKind::Other(name) => Err(ExpressionError::UnknownFunction(name.clone())),
        _ => Err(ExpressionError::EvalError(format!(
            "unsupported function `{func:?}`"
        ))),
    }
}
