//! Abstraction for expression evaluation context.
//!
//! Exposes semantic capabilities such as current value, path resolution,
//! variable lookups, occurrence index, value length, and content length.

use crate::expression::error::Result;
use crate::expression::path::Path;
use crate::value::DfdlValue;

/// Trait implemented by decoder and encoder evaluation contexts.
pub trait EvalContext {
    /// Resolves a path expression to a `DfdlValue` in the current infoset scope.
    fn resolve_path(&self, path: &Path) -> Result<DfdlValue>;

    /// Fetches a variable value by optional namespace prefix and local name.
    fn get_variable(&self, prefix: Option<&str>, local: &str) -> Result<DfdlValue>;

    /// Returns the current 1-based array occurrence index (`dfdl:occursIndex()`).
    fn occurs_index(&self) -> Result<usize>;

    /// Evaluates the value length in bytes of the current or targeted node.
    fn value_length(&self, path: Option<&Path>) -> Result<usize>;

    /// Evaluates the content length in bytes of the current or targeted node.
    fn content_length(&self, path: Option<&Path>) -> Result<usize>;
}
