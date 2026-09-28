//! DPath / DFDL Expression Subsystem.
//!
//! Provides Pest-based PEG parsing, canonical AST representation, canonical path navigation,
//! context abstractions, and a unified expression evaluator shared across decode and encode pipelines.

pub mod ast;
pub mod context;
pub mod error;
pub mod eval;
pub mod parser;
pub mod path;

pub use ast::{BinaryOpKind, Expr, FuncKind, SimpleType, UnaryOpKind};
pub use context::EvalContext;
pub use error::{ExpressionError, Result};
pub use eval::{eval, to_boolean};
pub use parser::parse_expression;
pub use path::{Path, PathOrigin, PathStep, StepTest};
