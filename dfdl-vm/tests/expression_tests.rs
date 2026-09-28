//! Comprehensive unit tests for the DPath expression subsystem.

use dfdl_vm::expression::ast::*;
use dfdl_vm::expression::context::EvalContext;
use dfdl_vm::expression::error::{ExpressionError, Result};
use dfdl_vm::expression::eval::eval;
use dfdl_vm::expression::parser::parse_expression;
use dfdl_vm::expression::path::*;
use dfdl_vm::value::{DfdlValue, StringValue};
use std::collections::BTreeMap;

struct DummyEvalContext {
    vars: BTreeMap<String, DfdlValue>,
}

impl DummyEvalContext {
    fn new() -> Self {
        Self {
            vars: BTreeMap::new(),
        }
    }
}

impl EvalContext for DummyEvalContext {
    fn resolve_path(&self, path: &Path) -> Result<DfdlValue> {
        if path.is_self() {
            return Ok(DfdlValue::Int(42));
        }
        if let Some(first) = path.steps.first() {
            if let StepTest::Name { local, .. } = &first.test {
                if local == "item" {
                    return Ok(DfdlValue::Array(vec![
                        DfdlValue::Int(10),
                        DfdlValue::Int(20),
                        DfdlValue::Int(30),
                    ]));
                }
            }
        }
        Err(ExpressionError::PathNotFound(path.to_string()))
    }

    fn get_variable(&self, _prefix: Option<&str>, local: &str) -> Result<DfdlValue> {
        self.vars
            .get(local)
            .cloned()
            .ok_or_else(|| ExpressionError::UndefinedVariable(local.to_string()))
    }

    fn occurs_index(&self) -> Result<usize> {
        Ok(3)
    }

    fn value_length(&self, _path: Option<&Path>) -> Result<usize> {
        Ok(8)
    }

    fn content_length(&self, _path: Option<&Path>) -> Result<usize> {
        Ok(10)
    }
}

#[test]
fn test_parse_literals() {
    let expr = parse_expression("123").unwrap();
    assert_eq!(expr, Expr::Literal(DfdlValue::Int(123)));

    let expr = parse_expression("'hello'").unwrap();
    assert_eq!(
        expr,
        Expr::Literal(DfdlValue::String(StringValue::new("hello".to_string())))
    );

    let expr = parse_expression("true").unwrap();
    assert_eq!(expr, Expr::Literal(DfdlValue::Boolean(true)));
}

#[test]
fn test_eval_arithmetic() {
    let ctx = DummyEvalContext::new();
    let expr = parse_expression("10 + 20 * 2").unwrap();
    let val = eval(&expr, &ctx).unwrap();
    assert_eq!(val, DfdlValue::Int(50));
}

#[test]
fn test_eval_comparisons() {
    let ctx = DummyEvalContext::new();
    let expr = parse_expression("5 lt 10").unwrap();
    let val = eval(&expr, &ctx).unwrap();
    assert_eq!(val, DfdlValue::Boolean(true));

    let expr = parse_expression("'a' eq 'a'").unwrap();
    let val = eval(&expr, &ctx).unwrap();
    assert_eq!(val, DfdlValue::Boolean(true));
}

#[test]
fn test_eval_if_then_else() {
    let ctx = DummyEvalContext::new();
    let expr = parse_expression("if (10 > 5) then 'yes' else 'no'").unwrap();
    let val = eval(&expr, &ctx).unwrap();
    assert_eq!(
        val,
        DfdlValue::String(StringValue::new("yes".to_string()))
    );
}

#[test]
fn test_eval_functions() {
    let ctx = DummyEvalContext::new();
    let expr = parse_expression("dfdl:occursIndex()").unwrap();
    let val = eval(&expr, &ctx).unwrap();
    assert_eq!(val, DfdlValue::Int(3));

    let expr = parse_expression("fn:concat('foo', 'bar')").unwrap();
    let val = eval(&expr, &ctx).unwrap();
    assert_eq!(
        val,
        DfdlValue::String(StringValue::new("foobar".to_string()))
    );
}

#[test]
fn test_eval_fn_count() {
    let ctx = DummyEvalContext::new();
    let expr = parse_expression("fn:count(item)").unwrap();
    let val = eval(&expr, &ctx).unwrap();
    assert_eq!(val, DfdlValue::Int(3));
}

#[test]
fn test_eval_cast() {
    let ctx = DummyEvalContext::new();
    let expr = parse_expression("xs:int('100')").unwrap();
    let val = eval(&expr, &ctx).unwrap();
    assert_eq!(val, DfdlValue::Int(100));
}

#[test]
fn test_parse_paths() {
    let expr = parse_expression("../foo/bar").unwrap();
    if let Expr::Path(p) = expr {
        assert_eq!(p.origin, PathOrigin::Parent(1));
        assert_eq!(p.steps.len(), 2);
    } else {
        panic!("expected path expression");
    }
}
