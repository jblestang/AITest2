//! Text/binary boolean representation helpers.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Split `textBooleanTrueRep` / `textBooleanFalseRep` into alternative literals (DFDL list syntax).
pub fn tokenize_text_boolean_rep_list(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    let bytes = trimmed.as_bytes();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'{' {
            let start = i;
            i += 1;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'{' {
                    depth += 1;
                } else if bytes[i] == b'}' {
                    depth -= 1;
                }
                i += 1;
            }
            out.push(trimmed[start..i].to_string());
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        out.push(trimmed[start..i].to_string());
    }
    out
}

fn local_name_from_qname(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

fn sibling_ref_name(inner: &str) -> Option<String> {
    let trimmed = inner.trim();
    let path = trimmed.strip_prefix("../")?;
    Some(local_name_from_qname(path.trim()).to_string())
}

fn sibling_text_value(
    siblings: Option<&BTreeMap<String, String>>,
    name: &str,
) -> Result<String, String> {
    siblings
        .and_then(|m| m.get(name))
        .cloned()
        .ok_or_else(|| alloc::format!("Schema Definition Error: {name} does not exist"))
}

fn sibling_content_byte_length(
    content_bytes: Option<&BTreeMap<String, usize>>,
    name: &str,
) -> Result<String, String> {
    content_bytes
        .and_then(|m| m.get(name))
        .map(|n| n.to_string())
        .ok_or_else(|| alloc::format!("Schema Definition Error: {name} does not exist"))
}

/// Sibling context for resolving `{ ../ex:foo }` and `dfdl:valueLength(../ex:foo, 'bytes')`.
pub struct BooleanSiblingEnv<'a> {
    pub text: &'a BTreeMap<String, String>,
    pub content_bytes: &'a BTreeMap<String, usize>,
}

/// Evaluate a single `{ ... }` or plain token to the comparison string at runtime.
pub fn resolve_text_boolean_rep_token(
    token: &str,
    siblings: Option<&BTreeMap<String, String>>,
    sibling_content_bytes: Option<&BTreeMap<String, usize>>,
) -> Result<String, String> {
    let trimmed = token.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if inner.starts_with('\'') && inner.ends_with('\'') && inner.len() >= 2 {
            return Ok(inner[1..inner.len() - 1].to_string());
        }
        if let Some(name) = sibling_ref_name(inner) {
            return sibling_text_value(siblings, &name);
        }
        if inner.starts_with("xs:string(") && inner.ends_with(')') {
            let arg = inner["xs:string(".len()..inner.len() - 1].trim();
            if arg.starts_with('\'') && arg.ends_with('\'') && arg.len() >= 2 {
                return Ok(arg[1..arg.len() - 1].to_string());
            }
            if arg.starts_with("dfdl:valueLength(") && arg.ends_with(')') {
                let inner_arg = arg["dfdl:valueLength(".len()..arg.len() - 1].trim();
                let (sib_part, _) = inner_arg
                    .split_once(',')
                    .ok_or_else(|| alloc::format!("unsupported xs:string argument `{arg}`"))?;
                let name = sibling_ref_name(sib_part.trim())
                    .ok_or_else(|| alloc::format!("unsupported xs:string argument `{arg}`"))?;
                if let Some(units) = inner_arg.split(',').nth(1).map(|u| u.trim().trim_matches('\'')) {
                    if units.eq_ignore_ascii_case("bytes") {
                        return sibling_content_byte_length(sibling_content_bytes, &name);
                    }
                }
                let text = sibling_text_value(siblings, &name)?;
                return Ok(text.len().to_string());
            }
            if let Some(v) = eval_simple_arithmetic(arg) {
                return Ok(v.to_string());
            }
            return Err(alloc::format!("unsupported xs:string argument `{arg}`"));
        }
        return Err(alloc::format!("unsupported boolean rep expression `{inner}`"));
    }
    Ok(trimmed.to_string())
}

fn eval_simple_arithmetic(expr: &str) -> Option<i64> {
    let expr = expr.replace(' ', "");
    if let Some((a, b)) = expr.split_once('-') {
        let x = a.parse::<i64>().ok()?;
        let y = b.parse::<i64>().ok()?;
        return Some(x - y);
    }
    if let Some((a, b)) = expr.split_once('+') {
        let x = a.parse::<i64>().ok()?;
        let y = b.parse::<i64>().ok()?;
        return Some(x + y);
    }
    expr.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_whitespace_and_braces() {
        let t = tokenize_text_boolean_rep_list("yes Y 1");
        assert_eq!(t, alloc::vec!["yes", "Y", "1"]);
        let t2 = tokenize_text_boolean_rep_list("{'a b c'} { xs:string(5-3) }");
        assert_eq!(t2.len(), 2);
        assert_eq!(t2[0], "{'a b c'}");
    }

    #[test]
    fn resolve_xs_string_arithmetic() {
        assert_eq!(
            resolve_text_boolean_rep_token("{ xs:string(5-3) }", None, None).unwrap(),
            "2"
        );
        assert_eq!(
            resolve_text_boolean_rep_token("{'a b c'}", None, None).unwrap(),
            "a b c"
        );
    }
}
