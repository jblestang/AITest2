//! Abstract Syntax Tree (AST) node definitions for DPath expressions.
//!
//! Models literals, paths, variables, function calls, casts, unary and binary operations,
//! and conditional expressions in a single unified hierarchy.

use crate::expression::path::Path;
use crate::value::DfdlValue;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Unary operator types supported in DPath expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOpKind {
    /// Arithmetic plus (`+x`).
    Plus,
    /// Arithmetic minus (`-x`).
    Minus,
    /// Logical negation (`not x` or `fn:not(x)`).
    Not,
}

/// Binary operator types supported in DPath expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOpKind {
    /// Addition (`+`).
    Add,
    /// Subtraction (`-`).
    Sub,
    /// Multiplication (`*`).
    Mul,
    /// Division (`div`).
    Div,
    /// Modulo / remainder (`mod`).
    Mod,
    /// Equality (`=` or `eq`).
    Eq,
    /// Inequality (`!=` or `ne`).
    Ne,
    /// Less than (`<` or `lt`).
    Lt,
    /// Less than or equal (`<=` or `le`).
    Le,
    /// Greater than (`>` or `gt`).
    Gt,
    /// Greater than or equal (`>=` or `ge`).
    Ge,
    /// Logical AND (`and`).
    And,
    /// Logical OR (`or`).
    Or,
}

/// Known built-in DPath function identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuncKind {
    /// `fn:count($arg)`
    FnCount,
    /// `fn:concat(...)`
    FnConcat,
    /// `fn:substring(...)`
    FnSubstring,
    /// `fn:exists(...)`
    FnExists,
    /// `fn:empty(...)`
    FnEmpty,
    /// `fn:not(...)`
    FnNot,
    /// `fn:nillable(...)`
    FnNillable,
    /// `dfdl:occursIndex()`
    DfdlOccursIndex,
    /// `dfdl:valueLength(...)`
    DfdlValueLength,
    /// `dfdl:contentLength(...)`
    DfdlContentLength,
    /// Generic or user-defined function.
    Other(String),
}

/// XSD Simple Types used in type constructors and explicit casts (e.g. `xs:int(...)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleType {
    String,
    Int,
    Integer,
    Long,
    Short,
    Byte,
    UnsignedInt,
    UnsignedLong,
    UnsignedShort,
    UnsignedByte,
    Double,
    Float,
    Boolean,
    Decimal,
}

/// Main Abstract Syntax Tree node for DPath expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Constant literal value (string, integer, float, boolean).
    Literal(DfdlValue),
    /// Path reference to infoset element or attribute.
    Path(Path),
    /// Variable reference (`$prefix:local` or `$local`).
    Variable {
        prefix: Option<String>,
        local: String,
    },
    /// Built-in or custom function call.
    FunctionCall {
        func: FuncKind,
        args: Vec<Expr>,
    },
    /// Explicit type cast or type constructor (`xs:int(...)`).
    Cast {
        target_type: SimpleType,
        expr: Box<Expr>,
    },
    /// Unary operation (`-x`, `+x`).
    UnaryOp {
        op: UnaryOpKind,
        expr: Box<Expr>,
    },
    /// Binary operation (`a + b`, `x eq y`, `cond and cond2`).
    BinaryOp {
        op: BinaryOpKind,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Conditional if-then-else expression (`if (c) then a else b`).
    IfThenElse {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
}

impl Expr {
    /// Helper builder for literal integer values.
    pub fn int_literal(val: i32) -> Self {
        Expr::Literal(DfdlValue::Int(val))
    }

    /// Helper builder for literal string values.
    pub fn string_literal(val: impl Into<String>) -> Self {
        Expr::Literal(DfdlValue::String(crate::value::StringValue::new(val.into())))
    }

    /// Helper builder for literal boolean values.
    pub fn bool_literal(val: bool) -> Self {
        Expr::Literal(DfdlValue::Boolean(val))
    }
}
