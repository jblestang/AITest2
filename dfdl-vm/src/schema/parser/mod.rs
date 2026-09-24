pub(crate) mod decl;
pub(crate) mod document;
pub(crate) mod element;
pub(crate) mod expr;
pub(crate) mod props;
pub mod qname;

pub(crate) use document::*;
pub(crate) use props::*;
pub use qname::*;

use super::ast::*;
use super::resolver::SchemaResolver;
use crate::error::Result;

/// Options controlling XSD parsing and include resolution.
#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    pub base_dir: Option<String>,
    /// File name for error messages (e.g. TDML external `model="foo.dfdl.xsd"`).
    pub schema_label: Option<String>,
}

/// Parse an XSD document with DFDL annotations into a [`SchemaDocument`].
pub fn parse_schema(input: &str) -> Result<SchemaDocument> {
    parse_schema_with_options(input, &ParseOptions::default())
}

/// Parse with include resolution via bundled/general format schemas.
pub fn parse_schema_with_options(input: &str, options: &ParseOptions) -> Result<SchemaDocument> {
    let mut resolver = SchemaResolver::new();
    if let Some(base) = &options.base_dir {
        resolver = resolver.with_base_dir(base.clone());
    }
    parse_schema_with_resolver_and_label(input, resolver, options.schema_label.as_deref())
}

/// Parse using a custom [`SchemaResolver`] for `xs:include` / `xs:import`.
pub fn parse_schema_with_resolver(input: &str, resolver: SchemaResolver) -> Result<SchemaDocument> {
    parse_schema_with_resolver_and_label(input, resolver, None)
}

#[cfg(test)]
mod tests;
