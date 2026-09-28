//! Expression error definitions for DPath expression parsing and evaluation.
//!
//! Provides typed errors for syntax errors, unresolved paths, invalid casts,
//! type mismatches, unknown functions, and missing variables.

use alloc::string::String;
use core::fmt;

/// Represents all possible errors during DPath parsing or evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionError {
    /// Syntax error during expression parsing.
    ParseError(String),
    /// Unresolved path or element step.
    PathNotFound(String),
    /// Index out of bounds on array or list.
    IndexOutOfBounds { index: i64, len: usize },
    /// Unknown function called in expression.
    UnknownFunction(String),
    /// Invalid number of arguments supplied to a function call.
    InvalidArgumentCount { expected: usize, found: usize },
    /// Variable reference was not found in evaluation context.
    UndefinedVariable(String),
    /// Type mismatch during binary or unary operation evaluation.
    TypeMismatch(String),
    /// Invalid cast from source value to target simple type.
    InvalidCast(String),
    /// General evaluation failure with detailed description.
    EvalError(String),
}

impl fmt::Display for ExpressionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExpressionError::ParseError(msg) => write!(f, "Parse Error: {msg}"),
            ExpressionError::PathNotFound(msg) => write!(f, "Path Not Found: {msg}"),
            ExpressionError::IndexOutOfBounds { index, len } => {
                write!(f, "Index Out of Bounds: index {index}, length {len}")
            }
            ExpressionError::UnknownFunction(name) => write!(f, "Unknown Function: {name}"),
            ExpressionError::InvalidArgumentCount { expected, found } => {
                write!(f, "Invalid Argument Count: expected {expected}, found {found}")
            }
            ExpressionError::UndefinedVariable(var) => write!(f, "Undefined Variable: ${var}"),
            ExpressionError::TypeMismatch(msg) => write!(f, "Type Mismatch: {msg}"),
            ExpressionError::InvalidCast(msg) => write!(f, "Invalid Cast: {msg}"),
            ExpressionError::EvalError(msg) => write!(f, "Evaluation Error: {msg}"),
        }
    }
}

pub type Result<T> = core::result::Result<T, ExpressionError>;
