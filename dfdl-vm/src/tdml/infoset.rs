use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Normalized infoset node for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfosetNode {
    pub name: String,
    pub text: Option<String>,
    pub children: BTreeMap<String, Vec<InfosetNode>>,
}

/// Compare decoded value against expected TDML infoset XML (best-effort).
pub fn compare_infoset(actual: &DfdlValue, expected_xml: &str) -> Result<(), String> {
    let expected = parse_expected_infoset(expected_xml)?;
    let actual_nodes = value_to_infoset(actual);
    compare_nodes(&expected, &actual_nodes)
}

fn parse_expected_infoset(xml: &str) -> Result<Vec<InfosetNode>, String> {
    let inner = extract_dfdl_infoset(xml);
    let mut nodes = Vec::new();
    let mut pos = 0;
    while pos < inner.len() {
        skip_ws(&inner, &mut pos);
        if pos >= inner.len() || inner.as_bytes()[pos] != b'<' {
            break;
        }
        if inner[pos..].starts_with("</") {
            break;
        }
        pos += 1;
        let tag_end = inner[pos..]
            .find(|c: char| c == ' ' || c == '>' || c == '/')
            .ok_or_else(|| "invalid infoset".to_string())?;
        let raw_name = &inner[pos..pos + tag_end];
        let name = local_name(raw_name);
        pos += tag_end;
        while pos < inner.len() && inner.as_bytes()[pos] != b'>' {
            pos += 1;
        }
        if pos < inner.len() {
            pos += 1;
        }
        if inner.as_bytes().get(pos.saturating_sub(2)) == Some(&b'/') {
            nodes.push(InfosetNode {
                name: name.to_string(),
                text: None,
                children: BTreeMap::new(),
            });
            continue;
        }
        let content_end = find_matching_end(&inner, pos, raw_name).ok_or_else(|| {
            alloc::format!("unclosed element `{name}`")
        })?;
        let content = &inner[pos..content_end];
        let (text, children) = parse_mixed_content(content)?;
        nodes.push(InfosetNode {
            name: name.to_string(),
            text,
            children,
        });
        pos = content_end;
        if let Some(close_end) = inner[pos..].find('>') {
            pos += close_end + 1;
        }
    }
    Ok(nodes)
}

fn extract_dfdl_infoset(xml: &str) -> &str {
    let xml = xml.trim();
    if let Some(start) = xml.find("dfdlInfoset") {
        if let Some(tag_start) = xml[..start].rfind('<') {
            if let Some(gt) = xml[tag_start..].find('>') {
                let content_start = tag_start + gt + 1;
                if let Some(end) = find_matching_end(xml, content_start, &xml[tag_start + 1..start + "dfdlInfoset".len()]) {
                    return xml[content_start..end].trim();
                }
            }
        }
    }
    xml
}

fn parse_mixed_content(content: &str) -> Result<(Option<String>, BTreeMap<String, Vec<InfosetNode>>), String> {
    let mut pos = 0;
    skip_ws(content, &mut pos);
    if pos >= content.len() || content.as_bytes()[pos] != b'<' {
        let text = content.trim();
        return Ok((
            if text.is_empty() { None } else { Some(text.to_string()) },
            BTreeMap::<String, Vec<InfosetNode>>::new(),
        ));
    }
    let children = parse_expected_infoset(content)?;
    let mut map: BTreeMap<String, Vec<InfosetNode>> = BTreeMap::new();
    for child in children {
        map.entry(child.name.clone()).or_default().push(child);
    }
    Ok((None, map))
}

fn find_matching_end(xml: &str, start: usize, raw_name: &str) -> Option<usize> {
    let local = local_name(raw_name);
    let mut pos = start;
    let mut depth = 1;
    while pos < xml.len() {
        if xml.as_bytes()[pos] != b'<' {
            pos += 1;
            continue;
        }
        if xml[pos..].starts_with("</") {
            let tag_start = pos + 2;
            let tag_end = xml[tag_start..]
                .find(|c: char| c == ' ' || c == '>')
                .map(|i| tag_start + i)
                .unwrap_or(xml.len());
            let close_name = local_name(&xml[tag_start..tag_end]);
            if close_name == local {
                depth -= 1;
                if depth == 0 {
                    return Some(pos);
                }
            }
            pos = tag_end;
            continue;
        }
        if xml[pos..].starts_with("<") && !xml[pos..].starts_with("<?") && !xml[pos..].starts_with("<!") {
            let tag_start = pos + 1;
            let tag_end = xml[tag_start..]
                .find(|c: char| c == ' ' || c == '>' || c == '/')
                .map(|i| tag_start + i)
                .unwrap_or(xml.len());
            let open_name = local_name(&xml[tag_start..tag_end]);
            if open_name == local && !xml[tag_start..].starts_with("/") {
                let rest = &xml[tag_end..];
                if !rest.trim_start().starts_with("/>") {
                    depth += 1;
                }
            }
        }
        pos += 1;
    }
    None
}

fn value_to_infoset(value: &DfdlValue) -> Vec<InfosetNode> {
    match value {
        DfdlValue::Sequence(map) => map
            .iter()
            .map(|(name, v)| value_to_node(name, v))
            .collect(),
        other => vec![value_to_node("root", other)],
    }
}

fn value_to_node(name: &str, value: &DfdlValue) -> InfosetNode {
    match value {
        DfdlValue::Sequence(map) => InfosetNode {
            name: name.to_string(),
            text: None,
            children: map
                .iter()
                .map(|(k, v)| (k.clone(), vec![value_to_node(k, v)]))
                .collect(),
        },
        DfdlValue::Array(items) => InfosetNode {
            name: name.to_string(),
            text: None,
            children: BTreeMap::from([(name.to_string(), items.iter().map(|v| value_to_node(name, v)).collect())]),
        },
        DfdlValue::Choice { discriminator, value } => value_to_node(discriminator, value),
        scalar => InfosetNode {
            name: name.to_string(),
            text: Some(scalar_to_string(scalar)),
            children: BTreeMap::new(),
        },
    }
}

fn scalar_to_string(value: &DfdlValue) -> String {
    match value {
        DfdlValue::Boolean(v) => v.to_string(),
        DfdlValue::Int(v) => v.to_string(),
        DfdlValue::Long(v) => v.to_string(),
        DfdlValue::Short(v) => v.to_string(),
        DfdlValue::Byte(v) => v.to_string(),
        DfdlValue::UnsignedInt(v) => v.to_string(),
        DfdlValue::UnsignedShort(v) => v.to_string(),
        DfdlValue::UnsignedByte(v) => v.to_string(),
        DfdlValue::Float(v) => v.to_string(),
        DfdlValue::Double(v) => v.to_string(),
        DfdlValue::String(v) => v.clone(),
        DfdlValue::HexBinary(v) => hex_encode(v),
        DfdlValue::Null => String::new(),
        DfdlValue::Array(_) | DfdlValue::Sequence(_) | DfdlValue::Choice { .. } => String::new(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn compare_nodes(expected: &[InfosetNode], actual: &[InfosetNode]) -> Result<(), String> {
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

fn compare_node(expected: &InfosetNode, actual: &InfosetNode) -> Result<(), String> {
    if local_name(&expected.name) != local_name(&actual.name) {
        return Err(alloc::format!(
            "element name mismatch: expected `{}`, got `{}`",
            expected.name, actual.name
        ));
    }
    if let Some(exp_text) = &expected.text {
        let act_text = actual.text.as_deref().unwrap_or("");
        if exp_text.trim() != act_text.trim() {
            return Err(alloc::format!(
                "text mismatch for `{}`: expected `{exp_text}`, got `{act_text}`",
                expected.name
            ));
        }
    }
    for (name, exp_children) in &expected.children {
        let key = local_name(name);
        let act_children = actual
            .children
            .iter()
            .find(|(k, _)| local_name(k) == key)
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
    Ok(())
}

fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn skip_ws(s: &str, pos: &mut usize) {
    while *pos < s.len() {
        if s.as_bytes()[*pos].is_ascii_whitespace() {
            *pos += 1;
        } else {
            break;
        }
    }
}
