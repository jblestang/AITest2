use crate::error::{ParseError, Result};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

/// Resolve XSD `schemaLocation` paths against bundled resources and external bases.
#[derive(Debug, Clone)]
pub struct SchemaResolver {
    bundled: BTreeMap<String, &'static str>,
    base_dirs: Vec<String>,
    /// Canonical include keys already merged (prevents infinite `xs:include` recursion).
    included: BTreeSet<String>,
    /// Shared across cloned resolvers so nested includes do not reuse `__inline_*` type names.
    inline_type_counter: Rc<Cell<usize>>,
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
        bundled.insert(
            "InvalidAlignSchema.dfdl.xsd".into(),
            include_str!("../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/InvalidAlignSchema.dfdl.xsd"),
        );
        bundled.insert(
            "/IBMdefined/GeneralPurposeFormat.xsd".into(),
            include_str!(
                "../../../third_party/daffodil/daffodil-core/src/main/resources/IBMdefined/GeneralPurposeFormat.xsd"
            ),
        );
        bundled.insert(
            "GeneralPurposeFormat.xsd".into(),
            include_str!(
                "../../../third_party/daffodil/daffodil-core/src/main/resources/IBMdefined/GeneralPurposeFormat.xsd"
            ),
        );
        Self {
            bundled,
            base_dirs: Vec::new(),
            included: BTreeSet::new(),
            inline_type_counter: Rc::new(Cell::new(0)),
        }
    }

    pub fn next_inline_type_name(&self, kind: &str) -> String {
        let n = self.inline_type_counter.get();
        self.inline_type_counter.set(n + 1);
        alloc::format!("__inline_{kind}_{n}")
    }

    /// Stable key for an include location (matches [`Self::resolve`] lookup order).
    pub fn include_dedup_key(&self, location: &str) -> String {
        let loc = location.trim();
        if self.bundled.contains_key(loc) {
            return loc.to_string();
        }
        let normalized = loc.trim_start_matches('/');
        if self.bundled.contains_key(normalized) {
            return normalized.to_string();
        }
        let file_name = loc.rsplit('/').next().unwrap_or(loc);
        if self.bundled.contains_key(file_name) {
            return file_name.to_string();
        }
        for base in &self.base_dirs {
            let candidate = alloc::format!("{base}/{loc}");
            if self.bundled.contains_key(&candidate) {
                return candidate;
            }
        }
        #[cfg(feature = "std")]
        {
            use std::path::Path;
            for base in &self.base_dirs {
                let candidates = [Path::new(base).join(loc), Path::new(base).join(file_name)];
                for path in &candidates {
                    if path.is_file() {
                        if let Ok(canon) = path.canonicalize() {
                            return canon.to_string_lossy().into_owned();
                        }
                        return path.to_string_lossy().into_owned();
                    }
                }
            }
        }
        if let Some(base) = self.base_dirs.first() {
            return alloc::format!("{base}/{loc}");
        }
        loc.to_string()
    }

    /// Register an include; returns `false` if this location was already included.
    pub fn register_include(&mut self, location: &str) -> bool {
        let key = self.include_dedup_key(location);
        self.included.insert(key)
    }

    pub fn with_base_dir(mut self, dir: impl Into<String>) -> Self {
        self.base_dirs.push(dir.into());
        self
    }

    pub fn resolve(&self, location: &str) -> Result<String> {
        self.resolve_with_include_dir(location)
            .map(|(content, _)| content)
    }

    /// Resolve schema content and the directory containing the file (for nested relative includes).
    pub fn resolve_with_include_dir(&self, location: &str) -> Result<(String, Option<String>)> {
        let loc = location.trim();
        if let Some(content) = self.bundled.get(loc) {
            return Ok(((*content).to_string(), None));
        }
        let normalized = loc.trim_start_matches('/');
        if let Some(content) = self.bundled.get(normalized) {
            return Ok(((*content).to_string(), None));
        }
        let file_name = loc.rsplit('/').next().unwrap_or(loc);
        if let Some(content) = self.bundled.get(file_name) {
            return Ok(((*content).to_string(), None));
        }
        for base in &self.base_dirs {
            let candidate = alloc::format!("{base}/{loc}");
            if let Some(content) = self.bundled.get(&candidate) {
                return Ok(((*content).to_string(), None));
            }
        }
        #[cfg(feature = "std")]
        {
            use std::path::{Path, PathBuf};
            let mut search_bases: Vec<PathBuf> = self
                .base_dirs
                .iter()
                .map(|b| PathBuf::from(b))
                .collect();
            search_bases.push(PathBuf::from(daffodil_test_resources_root()));

            for base in search_bases.iter().rev() {
                let candidates = [
                    base.join(loc),
                    base.join(normalized),
                    base.join(file_name),
                ];
                for path in &candidates {
                    if let Ok(content) = read_schema_text_file(path) {
                        let parent = path
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned());
                        return Ok((content, parent));
                    }
                }
            }
        }
        Err(ParseError::InvalidXml {
            message: alloc::format!(
                "Schema Definition Error: Resource not found at Include Location `{loc}`"
            ),
        }
        .into())
    }
}

impl Default for SchemaResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "std")]
fn daffodil_test_resources_root() -> String {
    alloc::format!(
        "{}/../third_party/daffodil/daffodil-test/src/test/resources",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[cfg(feature = "std")]
fn read_daffodil_test_resource(loc: &str) -> Option<String> {
    use std::path::Path;
    let root = daffodil_test_resources_root();
    let normalized = loc.trim_start_matches('/');
    let path = Path::new(&root).join(normalized);
    read_schema_text_file(&path).ok()
}

/// Read an on-disk XSD/DFDL schema as UTF-8 text, decoding UTF-16 when declared or implied by BOM.
#[cfg(feature = "std")]
pub fn read_schema_text_file(path: &std::path::Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    decode_schema_bytes(&bytes)
        .map(normalize_decoded_schema_xml_decl)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(feature = "std")]
fn normalize_decoded_schema_xml_decl(mut text: String) -> String {
    let Some(end) = text.find("?>") else {
        return text;
    };
    let (decl, rest) = text.split_at(end);
    let mut normalized = decl.to_string();
    for enc in [
        "UTF-16BE",
        "UTF-16LE",
        "UTF-16",
        "utf-16be",
        "utf-16le",
        "utf-16",
    ] {
        normalized = normalized.replace(
            &alloc::format!("encoding=\"{enc}\""),
            "encoding=\"UTF-8\"",
        );
        normalized = normalized.replace(
            &alloc::format!("encoding='{enc}'"),
            "encoding=\"UTF-8\"",
        );
    }
    text = alloc::format!("{normalized}{rest}");
    text
}

fn decode_schema_bytes(bytes: &[u8]) -> core::result::Result<String, String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(bytes[3..].to_vec())
            .map_err(|e| alloc::format!("invalid UTF-8 after BOM: {e}"));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_utf16_be(&bytes[2..]);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return decode_utf16_le(&bytes[2..]);
    }
    if bytes.len() >= 2 && bytes[0] == 0 && bytes[1] == b'<' {
        return decode_utf16_be(bytes);
    }
    if bytes.len() >= 2 && bytes[0] == b'<' && bytes[1] == 0 {
        return decode_utf16_le(bytes);
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let head_upper = head.to_ascii_uppercase();
    if head_upper.contains("ENCODING=\"UTF-16BE\"") || head_upper.contains("ENCODING='UTF-16BE'") {
        return decode_utf16_be(bytes);
    }
    if head_upper.contains("ENCODING=\"UTF-16LE\"") || head_upper.contains("ENCODING='UTF-16LE'") {
        return decode_utf16_le(bytes);
    }
    String::from_utf8(bytes.to_vec()).map_err(|e| alloc::format!("invalid UTF-8 schema: {e}"))
}

#[cfg(feature = "std")]
fn decode_utf16_be(bytes: &[u8]) -> core::result::Result<String, String> {
    if bytes.len() % 2 != 0 {
        return Err("UTF-16BE schema has odd byte length".into());
    }
    let mut units = alloc::vec::Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        units.push(u16::from_be_bytes([chunk[0], chunk[1]]));
    }
    String::from_utf16(&units).map_err(|e| alloc::format!("invalid UTF-16BE schema: {e}"))
}

#[cfg(feature = "std")]
fn decode_utf16_le(bytes: &[u8]) -> core::result::Result<String, String> {
    if bytes.len() % 2 != 0 {
        return Err("UTF-16LE schema has odd byte length".into());
    }
    let mut units = alloc::vec::Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    String::from_utf16(&units).map_err(|e| alloc::format!("invalid UTF-16LE schema: {e}"))
}
