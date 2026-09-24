use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Directory containing the `.tdml` file (for relative `documentPart type="file"` paths).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TdmlResourceContext {
    pub tdml_dir: Option<String>,
    /// Classpath-style path to the `.tdml` file (for error diagnostics).
    pub tdml_resource_path: Option<String>,
}

impl TdmlResourceContext {
    pub fn from_tdml_path(path: &str) -> Self {
        let path = path.replace('\\', "/");
        let tdml_dir = path.rsplit_once('/').map(|(dir, _)| dir.to_string());
        Self {
            tdml_dir,
            tdml_resource_path: tdml_resource_path_from_fs_path(&path),
        }
    }
}

pub fn tdml_resource_path_from_fs_path(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let marker = "org/apache/daffodil/";
    path.find(marker).map(|i| path[i..].to_string())
}

fn daffodil_test_resources_root() -> String {
    alloc::format!(
        "{}/../third_party/daffodil/daffodil-test/src/test/resources",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn candidate_paths(res_name: &str, ctx: &TdmlResourceContext) -> Vec<String> {
    let name = res_name.trim();
    let mut out = Vec::new();
    if let Some(dir) = &ctx.tdml_dir {
        out.push(alloc::format!("{dir}/{name}"));
    }
    let root = daffodil_test_resources_root();
    let normalized = name.trim_start_matches('/');
    out.push(alloc::format!("{root}/{normalized}"));
    if normalized.starts_with("org/apache/daffodil/") {
        out.push(alloc::format!("{root}/{normalized}"));
    }
    out
}

/// Load bytes for a TDML `documentPart` / infoset `type="file"` reference.
pub fn load_tdml_resource(res_name: &str, ctx: &TdmlResourceContext) -> Result<Vec<u8>, String> {
    #[cfg(feature = "std")]
    {
        use std::path::Path;
        for path in candidate_paths(res_name, ctx) {
            if Path::new(&path).is_file() {
                return std::fs::read(&path).map_err(|e| {
                    alloc::format!("TDMLRunner: data file '{res_name}' was not found ({e})")
                });
            }
        }
        Err(alloc::format!(
            "TDMLRunner: data file '{res_name}' was not found"
        ))
    }
    #[cfg(not(feature = "std"))]
    {
        let _ = (res_name, ctx);
        Err("TDML file resources require the `std` feature".into())
    }
}

/// Load text string for a TDML resource.
pub fn load_tdml_resource_string(
    res_name: &str,
    ctx: &TdmlResourceContext,
) -> Result<String, String> {
    let bytes = load_tdml_resource(res_name, ctx)?;
    String::from_utf8(bytes).map_err(|e| alloc::format!("invalid UTF-8 in resource file: {e}"))
}
