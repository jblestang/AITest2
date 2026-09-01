use crate::ir::{IrNode, IrProgram, ValueKind};
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
    pub nil: bool,
    pub children: BTreeMap<String, Vec<InfosetNode>>,
}

/// Compare decoded value against expected TDML infoset XML (best-effort).
pub fn compare_infoset(actual: &DfdlValue, expected_xml: &str) -> Result<(), String> {
    let expected = parse_expected_infoset(expected_xml)?;
    let actual_nodes = value_to_infoset(actual);
    compare_nodes(&expected, &actual_nodes)
}

/// Build a root-wrapped [`DfdlValue`] from TDML infoset XML for unparser tests.
pub fn infoset_xml_to_root_value(
    infoset_xml: &str,
    root: &str,
    program: &IrProgram,
) -> Result<DfdlValue, String> {
    let nodes = parse_expected_infoset(infoset_xml)?;
    let node = nodes
        .iter()
        .find(|n| local_name_str(&n.name) == root)
        .ok_or_else(|| alloc::format!("infoset missing root element `{root}`"))?;
    let inner = infoset_node_to_ir_value(program, program.root, node)?;
    let mut map = BTreeMap::new();
    map.insert(root.to_string(), inner);
    Ok(DfdlValue::sequence(map))
}

fn infoset_node_to_ir_value(
    program: &IrProgram,
    node_id: u32,
    node: &InfosetNode,
) -> Result<DfdlValue, String> {
    match program.node(node_id).map_err(|e| e.to_string())? {
        IrNode::Element { kind, child, .. } => {
            if node.nil {
                return Ok(DfdlValue::Null);
            }
            if *kind != ValueKind::Complex {
                return parse_scalar_for_kind(node.text.as_deref().unwrap_or(""), *kind);
            }
            let Some(child_id) = child else {
                return Ok(DfdlValue::sequence(BTreeMap::new()));
            };
            infoset_particle_to_value(program, *child_id, node)
        }
        IrNode::Sequence { children, .. } => {
            infoset_sequence_children_to_value(program, children, node)
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                let branch_name = program
                    .strings
                    .get(branch.name)
                    .map_err(|e| e.to_string())?;
                if !find_infoset_children(node, branch_name).is_empty() {
                    return infoset_particle_to_value(program, branch.node, node);
                }
            }
            Err(alloc::format!(
                "infoset does not match any choice branch under `{}`",
                node.name
            ))
        }
    }
}

fn infoset_particle_to_value(
    program: &IrProgram,
    node_id: u32,
    node: &InfosetNode,
) -> Result<DfdlValue, String> {
    match program.node(node_id).map_err(|e| e.to_string())? {
        IrNode::Sequence { children, .. } => {
            infoset_sequence_children_to_value(program, children, node)
        }
        _ => infoset_node_to_ir_value(program, node_id, node),
    }
}

fn infoset_sequence_children_to_value(
    program: &IrProgram,
    children: &[u32],
    node: &InfosetNode,
) -> Result<DfdlValue, String> {
    let mut map = BTreeMap::new();
    for &child_id in children {
        match program.node(child_id).map_err(|e| e.to_string())? {
            IrNode::Element { name, .. } => {
                let elem_name = program.strings.get(*name).map_err(|e| e.to_string())?;
                let local = local_name_str(elem_name);
                let infoset_children = find_infoset_children(node, local);
                if infoset_children.is_empty() {
                    continue;
                }
                if infoset_children.len() == 1
                    && infoset_children[0].children.is_empty()
                    && !infoset_children[0].nil
                    && infoset_children[0]
                        .text
                        .as_ref()
                        .map(|t| t.is_empty())
                        .unwrap_or(true)
                {
                    continue;
                }
                let value = if infoset_children.len() == 1 {
                    infoset_node_to_ir_value(program, child_id, infoset_children[0])?
                } else {
                    DfdlValue::Array(
                        infoset_children
                            .iter()
                            .map(|c| infoset_node_to_ir_value(program, child_id, c))
                            .collect::<Result<_, _>>()?,
                    )
                };
                map.insert(elem_name.to_string(), value);
            }
            IrNode::Sequence { .. } | IrNode::Choice { .. } => {
                let nested = infoset_particle_to_value(program, child_id, node)?;
                if let DfdlValue::Sequence(nested) = nested {
                    map.extend(nested.fields);
                }
            }
        }
    }
    Ok(DfdlValue::sequence(map))
}

fn find_infoset_children<'a>(node: &'a InfosetNode, local_name: &str) -> Vec<&'a InfosetNode> {
    node.children
        .iter()
        .filter(|(k, _)| local_name_str(k) == local_name)
        .flat_map(|(_, v)| v.iter())
        .collect()
}

fn parse_scalar_for_kind(text: &str, kind: ValueKind) -> Result<DfdlValue, String> {
    let trimmed = text.trim();
    match kind {
        ValueKind::String => Ok(DfdlValue::String(trimmed.to_string())),
        ValueKind::Boolean => trimmed
            .parse::<bool>()
            .or_else(|_| match trimmed {
                "TRUE" | "true" | "1" => Ok(true),
                "FALSE" | "false" | "0" => Ok(false),
                _ => Err(()),
            })
            .map(DfdlValue::Boolean)
            .map_err(|_| alloc::format!("invalid boolean `{trimmed}`")),
        ValueKind::Byte => trimmed
            .parse::<i8>()
            .map(DfdlValue::Byte)
            .map_err(|e| e.to_string()),
        ValueKind::Short => trimmed
            .parse::<i16>()
            .map(DfdlValue::Short)
            .map_err(|e| e.to_string()),
        ValueKind::Int => trimmed
            .parse::<i32>()
            .map(DfdlValue::Int)
            .map_err(|e| e.to_string()),
        ValueKind::Long => trimmed
            .parse::<i64>()
            .map(DfdlValue::Long)
            .map_err(|e| e.to_string()),
        ValueKind::UnsignedByte => trimmed
            .parse::<u8>()
            .map(DfdlValue::UnsignedByte)
            .map_err(|e| e.to_string()),
        ValueKind::UnsignedShort => trimmed
            .parse::<u16>()
            .map(DfdlValue::UnsignedShort)
            .map_err(|e| e.to_string()),
        ValueKind::UnsignedInt => trimmed
            .parse::<u32>()
            .map(DfdlValue::UnsignedInt)
            .map_err(|e| e.to_string()),
        ValueKind::Float => trimmed
            .parse::<f32>()
            .map(DfdlValue::Float)
            .map_err(|e| e.to_string()),
        ValueKind::Double => trimmed
            .parse::<f64>()
            .map(DfdlValue::Double)
            .map_err(|e| e.to_string()),
        ValueKind::Decimal => Ok(DfdlValue::Decimal(trimmed.to_string())),
        ValueKind::DateTime | ValueKind::Time => Ok(DfdlValue::DateTime(trimmed.to_string())),
        ValueKind::HexBinary => decode_hex(trimmed).map(DfdlValue::HexBinary),
        ValueKind::Complex => {
            if trimmed.is_empty() {
                Ok(DfdlValue::sequence(BTreeMap::new()))
            } else {
                Err("complex element requires child elements in infoset".into())
            }
        }
    }
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    let hex = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    if hex.len() % 2 != 0 {
        return Err(alloc::format!("invalid hex `{text}`"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Infer the document root element local name from expected TDML infoset XML.
pub fn infer_root_element_name(expected_xml: &str) -> Option<String> {
    let inner = extract_dfdl_infoset_xml(expected_xml);
    if inner.trim().is_empty() {
        return None;
    }
    let nodes = parse_infoset_elements(&inner).ok()?;
    nodes.first().map(|n| local_name_str(&n.name).to_string())
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
    let attrs = attrs_to_map(&attributes);
    let is_nil = attrs
        .get("xsi:nil")
        .or_else(|| attrs.get("{http://www.w3.org/2001/XMLSchema-instance}nil"))
        .is_some_and(|v| v == "true");

    if reader.peek_is_end(&element_name).map_err(|e| e.to_string())? {
        reader.expect_end(&element_name).map_err(|e| e.to_string())?;
        return Ok(InfosetNode {
            name: element_name,
            text: None,
            nil: is_nil,
            children: BTreeMap::new(),
        });
    }

    reader.skip_insignificant_ws().map_err(|e| e.to_string())?;
    if reader.peek_is_end(&element_name).map_err(|e| e.to_string())? {
        reader.expect_end(&element_name).map_err(|e| e.to_string())?;
        return Ok(InfosetNode {
            name: element_name,
            text: None,
            nil: is_nil,
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
                nil: is_nil,
                children: map,
            })
        }
        None => {
            let text = reader.read_text_until_end(&element_name).map_err(|e| e.to_string())?;
            Ok(InfosetNode {
                name: element_name,
                text: Some(text.trim().to_string()),
                nil: is_nil,
                children: BTreeMap::new(),
            })
        }
    }
}

fn value_to_infoset(value: &DfdlValue) -> Vec<InfosetNode> {
    match value {
        DfdlValue::Sequence(seq) => seq
            .fields
            .iter()
            .map(|(name, v)| value_to_node(name, v))
            .collect(),
        other => vec![value_to_node("root", other)],
    }
}

fn value_to_node(name: &str, value: &DfdlValue) -> InfosetNode {
    match value {
        DfdlValue::Sequence(seq) => InfosetNode {
            name: name.to_string(),
            text: None,
            nil: false,
            children: seq
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), field_values_to_infoset_nodes(k, v)))
                .collect(),
        },
        DfdlValue::Array(items) => InfosetNode {
            name: name.to_string(),
            text: None,
            nil: false,
            children: BTreeMap::from([(name.to_string(), items.iter().map(|v| value_to_node(name, v)).collect())]),
        },
        DfdlValue::Choice { discriminator, value } => value_to_node(discriminator, value),
        DfdlValue::Null => InfosetNode {
            name: name.to_string(),
            text: None,
            nil: true,
            children: BTreeMap::new(),
        },
        scalar => InfosetNode {
            name: name.to_string(),
            text: Some(scalar_to_string(scalar)),
            nil: false,
            children: BTreeMap::new(),
        },
    }
}

fn field_values_to_infoset_nodes(name: &str, value: &DfdlValue) -> Vec<InfosetNode> {
    match value {
        DfdlValue::Array(items) => items.iter().map(|v| value_to_node(name, v)).collect(),
        other => vec![value_to_node(name, other)],
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
        DfdlValue::Decimal(v) => v.clone(),
        DfdlValue::DateTime(v) => v.clone(),
        DfdlValue::String(v) => v.clone(),
        DfdlValue::HexBinary(v) => hex_encode(v),
        DfdlValue::Null => String::new(),
        DfdlValue::Array(_) | DfdlValue::Sequence(_) | DfdlValue::Choice { .. } => String::new(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
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
