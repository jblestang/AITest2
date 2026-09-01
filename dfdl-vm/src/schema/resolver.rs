use crate::error::{ParseError, Result};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Resolve XSD `schemaLocation` paths against bundled resources and external bases.
#[derive(Debug, Clone)]
pub struct SchemaResolver {
    bundled: BTreeMap<String, &'static str>,
    base_dirs: Vec<String>,
}

impl SchemaResolver {
    pub fn new() -> Self {
        let mut bundled = BTreeMap::new();
        bundled.insert(
            "/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd".into(),
            include_str!("../../resources/dfdl/DFDLGeneralFormat.dfdl.xsd"),
        );
        bundled.insert(
            "DFDLGeneralFormat.dfdl.xsd".into(),
            include_str!("../../resources/dfdl/DFDLGeneralFormat.dfdl.xsd"),
        );
        bundled.insert(
            "DFDLGeneralFormatBase.dfdl.xsd".into(),
            include_str!("../../resources/dfdl/DFDLGeneralFormatBase.dfdl.xsd"),
        );
        bundled.insert(
            "/org/apache/daffodil/xsd/DFDLGeneralFormatBase.dfdl.xsd".into(),
            include_str!("../../resources/dfdl/DFDLGeneralFormatBase.dfdl.xsd"),
        );
        bundled.insert(
            "DFDLGeneralFormatPortable.dfdl.xsd".into(),
            include_str!("../../resources/dfdl/DFDLGeneralFormatPortable.dfdl.xsd"),
        );
        bundled.insert(
            "/org/apache/daffodil/xsd/DFDLGeneralFormatPortable.dfdl.xsd".into(),
            include_str!("../../resources/dfdl/DFDLGeneralFormatPortable.dfdl.xsd"),
        );
        bundled.insert(
            "AB.dfdl.xsd".into(),
            include_str!("../../resources/dfdl/AB.dfdl.xsd"),
        );
        bundled.insert(
            "/org/apache/daffodil/section12/lengthKind/AB.dfdl.xsd".into(),
            include_str!("../../resources/dfdl/AB.dfdl.xsd"),
        );
        Self {
            bundled,
            base_dirs: Vec::new(),
        }
    }

    pub fn with_base_dir(mut self, dir: impl Into<String>) -> Self {
        self.base_dirs.push(dir.into());
        self
    }

    pub fn resolve(&self, location: &str) -> Result<String> {
        let loc = location.trim();
        if let Some(content) = self.bundled.get(loc) {
            return Ok((*content).to_string());
        }
        let normalized = loc.trim_start_matches('/');
        if let Some(content) = self.bundled.get(normalized) {
            return Ok((*content).to_string());
        }
        let file_name = loc.rsplit('/').next().unwrap_or(loc);
        if let Some(content) = self.bundled.get(file_name) {
            return Ok((*content).to_string());
        }
        for base in &self.base_dirs {
            let candidate = alloc::format!("{base}/{loc}");
            if let Some(content) = self.bundled.get(&candidate) {
                return Ok((*content).to_string());
            }
        }
        Err(ParseError::InvalidXml {
            message: alloc::format!("cannot resolve schemaLocation `{loc}`"),
        }
        .into())
    }
}

impl Default for SchemaResolver {
    fn default() -> Self {
        Self::new()
    }
}
