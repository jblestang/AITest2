//! Canonical path and step data structures for DPath expression evaluation.
//!
//! Centralizes path representation so decoder and encoder share identical step navigation,
//! relative parent resolution, attribute lookups, and predicate evaluation.

use crate::expression::ast::Expr;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Defines the origin of a path (relative, root, or parent steps).
#[derive(Debug, Clone, PartialEq)]
pub enum PathOrigin {
    /// Relative to current infoset context node (`.`).
    Relative,
    /// Absolute path starting from infoset root (`/`).
    Root,
    /// Path climbing `up` levels up to parent or ancestor frames (`..`).
    Parent(usize),
}

/// Represents a single step test in a path expression.
#[derive(Debug, Clone, PartialEq)]
pub enum StepTest {
    /// Match a specific QName element node (`prefix:local` or `local`).
    Name {
        prefix: Option<String>,
        local: String,
    },
    /// Match any element node (`*`).
    Wildcard,
    /// Attribute step (`@attr`).
    Attribute(String),
    /// Current node self step (`.`).
    SelfNode,
}

impl fmt::Display for StepTest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepTest::Name { prefix: Some(p), local } => write!(f, "{p}:{local}"),
            StepTest::Name { prefix: None, local } => write!(f, "{local}"),
            StepTest::Wildcard => write!(f, "*"),
            StepTest::Attribute(attr) => write!(f, "@{attr}"),
            StepTest::SelfNode => write!(f, "."),
        }
    }
}

/// Represents a single step in a path, including optional predicate filters.
#[derive(Debug, Clone, PartialEq)]
pub struct PathStep {
    /// The step test target (name, wildcard, attribute, self).
    pub test: StepTest,
    /// Optional predicate filter (e.g. `[1]`, `[dfdl:occursIndex()]`, or `[x = 5]`).
    pub predicate: Option<Box<Expr>>,
}

impl PathStep {
    /// Creates a simple step with no predicate.
    pub fn simple(test: StepTest) -> Self {
        Self {
            test,
            predicate: None,
        }
    }

    /// Creates a step with a predicate filter.
    pub fn with_predicate(test: StepTest, predicate: Expr) -> Self {
        Self {
            test,
            predicate: Some(Box::new(predicate)),
        }
    }
}

impl fmt::Display for PathStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.test)?;
        if let Some(pred) = &self.predicate {
            write!(f, "[{pred:?}]")?;
        }
        Ok(())
    }
}

/// Canonical path representation for all DPath path expressions.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Origin baseline (relative, parent climb, or root).
    pub origin: PathOrigin,
    /// Ordered sequence of path steps.
    pub steps: Vec<PathStep>,
}

impl Path {
    /// Creates a new relative path with given steps.
    pub fn relative(steps: Vec<PathStep>) -> Self {
        Self {
            origin: PathOrigin::Relative,
            steps,
        }
    }

    /// Creates a new root-anchored path.
    pub fn root(steps: Vec<PathStep>) -> Self {
        Self {
            origin: PathOrigin::Root,
            steps,
        }
    }

    /// Creates a path climbing `up` parent levels.
    pub fn parent(up: usize, steps: Vec<PathStep>) -> Self {
        Self {
            origin: PathOrigin::Parent(up),
            steps,
        }
    }

    /// Checks if the path refers directly to self (`.`).
    pub fn is_self(&self) -> bool {
        self.origin == PathOrigin::Relative && (self.steps.is_empty() || (self.steps.len() == 1 && self.steps[0].test == StepTest::SelfNode))
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.origin {
            PathOrigin::Relative => {}
            PathOrigin::Root => write!(f, "/")?,
            PathOrigin::Parent(n) => {
                for i in 0..*n {
                    if i > 0 {
                        write!(f, "/")?;
                    }
                    write!(f, "..")?;
                }
                if !self.steps.is_empty() {
                    write!(f, "/")?;
                }
            }
        }
        for (i, step) in self.steps.iter().enumerate() {
            if i > 0 || (self.origin == PathOrigin::Relative && i > 0) {
                write!(f, "/")?;
            }
            write!(f, "{step}")?;
        }
        Ok(())
    }
}
