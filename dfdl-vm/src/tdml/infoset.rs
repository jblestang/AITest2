use crate::value::DfdlValue;
use crate::xml_util::{attrs_to_map, local_name_str, owned_local_name, XmlReader};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use xml_no_std::reader::XmlEvent;

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
    let inner = extract_dfdl_infoset_xml(xml);
    parse_infoset_elements(&inner)
}

fn extract_dfdl_infoset_xml(xml: &str) -> String {
    let xml = xml.trim();
    if xml.is_empty() {
        return String::new();
    }
    let wrapped = alloc::format!("<wrapper>{xml}</wrapper>");
    let mut reader = XmlReader::new(&wrapped);
    let _ = reader.expect_start("wrapper");
    reader.skip_insignificant_ws().ok();
    if reader.peek_start_local().ok().flatten().as_deref() == Some("dfdlInfoset") {
        let _ = reader.next_event();
        return reader.read_inner_xml().unwrap_or_default();
    }
    xml.to_string()
}

fn parse_infoset_elements(xml: &str) -> Result<Vec<InfosetNode>, String> {
    let wrapped = alloc::format!("<infosetRoot>{xml}</infosetRoot>");
    let mut reader = XmlReader::new(&wrapped);
    reader.expect_start("infosetRoot").map_err(|e| e.to_string())?;

    let mut nodes = Vec::new();
    loop {
        reader.skip_insignificant_ws().map_err(|e| e.to_string())?;
        if reader.peek_is_end("infosetRoot").map_err(|e| e.to_string())? {
            reader.expect_end("infosetRoot").map_err(|e| e.to_string())?;
            break;
        }
        match reader.peek_start_local().map_err(|e| e.to_string())? {
            Some(_) => nodes.push(parse_infoset_element(&mut reader)?),
            None => break,
        }
    }
    Ok(nodes)
}

fn parse_infoset_element(reader: &mut XmlReader<'_>) -> Result<InfosetNode, String> {
    let XmlEvent::StartElement { name, attributes, .. } = reader.next_event().map_err(|e| e.to_string())?
    else {
        return Err("expected infoset element".into());
    };
    let element_name = owned_local_name(&name).to_string();
    let _ = attrs_to_map(&attributes);

    if reader.peek_is_end(&element_name).map_err(|e| e.to_string())? {
        reader.expect_end(&element_name).map_err(|e| e.to_string())?;
        return Ok(InfosetNode {
            name: element_name,
            text: None,
            children: BTreeMap::new(),
        });
    }

    reader.skip_insignificant_ws().map_err(|e| e.to_string())?;
    if reader.peek_is_end(&element_name).map_err(|e| e.to_string())? {
        reader.expect_end(&element_name).map_err(|e| e.to_string())?;
        return Ok(InfosetNode {
            name: element_name,
            text: None,
            children: BTreeMap::new(),
        });
    }

    match reader.peek_start_local().map_err(|e| e.to_string())? {
        Some(_) => {
            let children = parse_infoset_elements(&reader.read_inner_xml().map_err(|e| e.to_string())?)?;
            let mut map: BTreeMap<String, Vec<InfosetNode>> = BTreeMap::new();
            for child in children {
                map.entry(child.name.clone()).or_default().push(child);
            }
            Ok(InfosetNode {
                name: element_name,
                text: None,
                children: map,
            })
        }
        None => {
            let text = reader.read_text_until_end(&element_name).map_err(|e| e.to_string())?;
            Ok(InfosetNode {
                name: element_name,
                text: Some(text.trim().to_string()),
                children: BTreeMap::new(),
            })
        }
    }
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
    if local_name_str(&expected.name) != local_name_str(&actual.name) {
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
        let key = local_name_str(name);
        let act_children = actual
            .children
            .iter()
            .find(|(k, _)| local_name_str(k) == key)
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
        let key = local_name_str(name);
        if !expected
            .children
            .keys()
            .any(|k| local_name_str(k) == key)
        {
            return Err(alloc::format!(
                "unexpected child `{key}` ({} occurrence(s))",
                act_children.len()
            ));
        }
    }
    Ok(())
}
