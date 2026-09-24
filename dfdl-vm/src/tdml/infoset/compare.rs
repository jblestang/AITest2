//! TDML infoset comparison utilities.

use super::InfosetNode;
use crate::value::DfdlValue;
use alloc::string::{String, ToString};

pub fn compare_nodes(expected: &[InfosetNode], actual: &[InfosetNode]) -> Result<(), String> {
    if expected.len() != actual.len() {
        return Err(alloc::format!(
            "root child count mismatch: expected {}, got {}",
            expected.len(),
            actual.len()
        ));
    }
    for (e, a) in expected.iter().zip(actual.iter()) {
        compare_node(e, a)?;
    }
    Ok(())
}

pub fn compare_node(expected: &InfosetNode, actual: &InfosetNode) -> Result<(), String> {
    if crate::xml_util::local_name_str(&expected.name)
        != crate::xml_util::local_name_str(&actual.name)
    {
        return Err(alloc::format!(
            "element name mismatch: expected `{}`, got `{}`",
            expected.name,
            actual.name
        ));
    }
    if let Some(exp_text) = &expected.text {
        if exp_text.contains("/blobs/") && exp_text.ends_with(".bin") {
            let blob = actual.blob_bytes.as_deref().ok_or_else(|| {
                alloc::format!(
                    "expected blob data for `{}`, got text `{:?}`",
                    expected.name,
                    actual.text
                )
            })?;
            compare_blob_reference(exp_text, blob)?;
        } else {
            let act_text = actual.text.as_deref().unwrap_or("");
            let exp_trim = exp_text.trim();
            let act_trim = act_text.trim();
            if exp_trim != act_trim
                && !float_infoset_texts_equal(exp_trim, act_trim)
                && !calendar_infoset_texts_equal(exp_trim, act_trim)
            {
                return Err(alloc::format!(
                    "text mismatch for `{}`: expected `{exp_text}`, got `{act_text}`",
                    expected.name
                ));
            }
        }
    }
    for (name, exp_children) in &expected.children {
        let key = crate::xml_util::local_name_str(name);
        let act_children = actual
            .children
            .iter()
            .find(|(k, _)| crate::xml_util::local_name_str(k) == key)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[]);
        if exp_children.len() != act_children.len() {
            return Err(alloc::format!(
                "child count mismatch for `{key}`: expected {}, got {}",
                exp_children.len(),
                act_children.len()
            ));
        }
        for (e, a) in exp_children.iter().zip(act_children.iter()) {
            compare_node(e, a)?;
        }
    }
    for (name, act_children) in &actual.children {
        let key = crate::xml_util::local_name_str(name);
        if !expected
            .children
            .keys()
            .any(|k| crate::xml_util::local_name_str(k) == key)
        {
            return Err(alloc::format!(
                "unexpected child `{key}` ({} occurrence(s))",
                act_children.len()
            ));
        }
    }
    Ok(())
}

pub fn calendar_infoset_texts_equal(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    if expected.contains('T') && !expected.contains('+') && !expected.contains('Z') {
        if let Some(rest) = actual.strip_prefix(expected) {
            if rest == "+00:00" || rest == "Z" {
                return true;
            }
        }
    }
    false
}

pub fn float_infoset_texts_equal(expected: &str, actual: &str) -> bool {
    let Ok(exp) = expected.trim().parse::<f32>() else {
        return false;
    };
    let Ok(act) = actual.trim().parse::<f32>() else {
        return false;
    };
    if exp.to_bits() == act.to_bits() {
        return true;
    }
    let diff = f32_abs(exp - act);
    let mut scale = f32_abs(exp);
    if f32_abs(act) > scale {
        scale = f32_abs(act);
    }
    if scale < 1.0 {
        scale = 1.0;
    }
    diff <= scale * 1e-5
}

fn f32_abs(v: f32) -> f32 {
    if v.is_sign_negative() {
        -v
    } else {
        v
    }
}

pub fn format_float_for_infoset(v: f32) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    let av = f32_abs(v);
    if av >= 1_000_000.0 || (av > 0.0 && av < 0.0001) {
        let mut s = alloc::format!("{v:e}");
        if let Some(idx) = s.find('e') {
            s.replace_range(idx..idx + 1, "E");
        }
        return s;
    }
    let whole = (v as i64) as f32;
    if f32_abs(v - whole) < f32::EPSILON {
        alloc::format!("{v:.1}")
    } else {
        v.to_string()
    }
}

pub fn scalar_to_string(value: &DfdlValue) -> String {
    match value {
        DfdlValue::Boolean(v) => v.to_string(),
        DfdlValue::Int(v) => v.to_string(),
        DfdlValue::Integer(v) => v.clone(),
        DfdlValue::Long(v) => v.to_string(),
        DfdlValue::UnsignedLong(v) => v.to_string(),
        DfdlValue::Short(v) => v.to_string(),
        DfdlValue::Byte(v) => v.to_string(),
        DfdlValue::UnsignedInt(v) => v.to_string(),
        DfdlValue::UnsignedShort(v) => v.to_string(),
        DfdlValue::UnsignedByte(v) => v.to_string(),
        DfdlValue::Float(v) => format_float_for_infoset(*v),
        DfdlValue::Double(v) => format_float_for_infoset(*v as f32),
        DfdlValue::Decimal(v) => v.clone(),
        DfdlValue::DateTime(v) => v.clone(),
        DfdlValue::String(v) => v.text.clone(),
        DfdlValue::HexBinary(v) => hex_encode(v),
        DfdlValue::Blob(_) => String::new(),
        DfdlValue::Null => String::new(),
        DfdlValue::Array(_) | DfdlValue::Sequence(_) | DfdlValue::Choice { .. } => String::new(),
    }
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

pub fn resolve_blob_uri_to_bytes(uri: &str) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
    let uri = uri.trim();
    if uri.contains('\'') {
        return Err("Illegal character".into());
    }
    if uri.contains("://") && !uri.starts_with("file:") {
        return Err(alloc::format!("Blob URI must be a file: {uri}"));
    }
    #[cfg(feature = "std")]
    {
        let path = if let Some(rest) = uri.strip_prefix("file:") {
            std::path::PathBuf::from(rest)
        } else {
            std::path::PathBuf::from(tdml_blob_reference_path(uri))
        };
        std::fs::read(&path).map_err(|_| alloc::format!("Unable to open blob for reading: {uri}"))
    }
    #[cfg(not(feature = "std"))]
    {
        let _ = uri;
        Err("blob unparse requires the `std` feature".into())
    }
}

fn tdml_blob_reference_path(uri: &str) -> alloc::string::String {
    alloc::format!(
        "{}/../third_party/daffodil/daffodil-test/src/test/resources/{}",
        env!("CARGO_MANIFEST_DIR"),
        uri.trim_start_matches('/')
    )
}

fn compare_blob_reference(expected_uri: &str, actual: &[u8]) -> Result<(), String> {
    #[cfg(feature = "std")]
    {
        let path = tdml_blob_reference_path(expected_uri);
        let reference = std::fs::read(&path)
            .map_err(|e| alloc::format!("blob reference read `{path}`: {e}"))?;
        if reference != actual {
            return Err(alloc::format!(
                "blob bytes mismatch for `{expected_uri}`: expected {} byte(s), got {} byte(s)",
                reference.len(),
                actual.len()
            ));
        }
        Ok(())
    }
    #[cfg(not(feature = "std"))]
    {
        let _ = (expected_uri, actual);
        Err("blob infoset compare requires the `std` feature".into())
    }
}
