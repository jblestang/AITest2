use super::resources::{load_tdml_resource, TdmlResourceContext};
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
    pub namespace: Option<String>,
    pub text: Option<String>,
    pub nil: bool,
    pub children: BTreeMap<String, Vec<InfosetNode>>,
    /// Raw blob bytes when `dfdlx:objectKind="bytes"`.
    pub blob_bytes: Option<Vec<u8>>,
}

/// Compare decoded value against expected TDML infoset XML (best-effort).
pub fn compare_infoset(actual: &DfdlValue, expected_xml: &str) -> Result<(), String> {
    compare_infoset_with_context(actual, expected_xml, &TdmlResourceContext::default())
}

/// Compare decoded value against expected TDML infoset, resolving `type="file"` references.
pub fn compare_infoset_with_context(
    actual: &DfdlValue,
    expected_xml: &str,
    ctx: &TdmlResourceContext,
) -> Result<(), String> {
    let expected = parse_expected_infoset_with_context(expected_xml, ctx)?;
    let actual_nodes = value_to_infoset(actual);
    compare_nodes(&expected, &actual_nodes)
}

/// Resolve expected infoset XML, including `dfdlInfoset type="file"` (validates DOCTYPE policy).
pub fn resolve_expected_infoset_xml(
    expected_xml: &str,
    ctx: &TdmlResourceContext,
) -> Result<String, String> {
    resolve_infoset_inner_xml(expected_xml, ctx)
}

/// Build a root-wrapped [`DfdlValue`] from TDML infoset XML for unparser tests.
pub fn infoset_xml_to_root_value(
    infoset_xml: &str,
    root: &str,
    program: &IrProgram,
) -> Result<DfdlValue, String> {
    infoset_xml_to_root_value_with_context(
        infoset_xml,
        root,
        program,
        &TdmlResourceContext::default(),
    )
}

pub fn infoset_xml_to_root_value_with_context(
    infoset_xml: &str,
    root: &str,
    program: &IrProgram,
    ctx: &TdmlResourceContext,
) -> Result<DfdlValue, String> {
    infoset_xml_to_root_value_with_target_ns(infoset_xml, root, program, ctx, None)
}

pub fn infoset_xml_to_root_value_with_target_ns(
    infoset_xml: &str,
    root: &str,
    program: &IrProgram,
    ctx: &TdmlResourceContext,
    target_namespace: Option<&str>,
) -> Result<DfdlValue, String> {
    let nodes = parse_expected_infoset_with_target_ns(infoset_xml, ctx, target_namespace)?;
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
            match program.node(*child_id).map_err(|e| e.to_string())? {
                IrNode::Choice { branches, .. } => {
                    choice_matched_branch_value(program, branches, node)
                }
                _ => infoset_particle_to_value(program, *child_id, node),
            }
        }
        IrNode::Sequence { children, .. } => {
            infoset_sequence_children_to_value(program, children, node)
        }
        IrNode::Choice { branches, .. } => {
            choice_matched_branch_value(program, branches, node)
        }
    }
}

fn insert_choice_branch_value(
    program: &IrProgram,
    branch_node: u32,
    branch_name: &str,
    value: DfdlValue,
    map: &mut BTreeMap<String, DfdlValue>,
) -> Result<(), String> {
    match value {
        DfdlValue::Sequence(nested) => {
            let hoist = matches!(
                program.node(branch_node),
                Ok(IrNode::Sequence { .. })
            );
            if hoist {
                map.extend(nested.fields);
            } else {
                map.insert(branch_name.to_string(), DfdlValue::Sequence(nested));
            }
        }
        other => {
            map.insert(branch_name.to_string(), other);
        }
    }
    Ok(())
}

fn choice_matched_branch_value(
    program: &IrProgram,
    branches: &[crate::ir::ChoiceBranch],
    node: &InfosetNode,
) -> Result<DfdlValue, String> {
    for branch in branches {
        let branch_name = program
            .strings
            .get(branch.name)
            .map_err(|e| e.to_string())?;
        let local = local_name_str(branch_name);
        if find_infoset_children(node, local).is_empty() {
            continue;
        }
        let branch_nodes = find_infoset_children(node, local);
        let value = if branch_nodes.len() == 1 {
            infoset_node_to_ir_value(program, branch.node, branch_nodes[0])?
        } else {
            infoset_particle_to_value(program, branch.node, node)?
        };
        let mut map = BTreeMap::new();
        insert_choice_branch_value(program, branch.node, branch_name, value, &mut map)?;
        return Ok(DfdlValue::sequence(map));
    }
    for branch in branches {
        if matches!(
            program.node(branch.node).ok(),
            Some(IrNode::Sequence { children, .. }) if children.is_empty()
        ) {
            return Ok(DfdlValue::sequence(BTreeMap::new()));
        }
    }
    Err(alloc::format!(
        "infoset does not match any choice branch under `{}`",
        node.name
    ))
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
                let empty_present = infoset_children.len() == 1
                    && infoset_children[0].children.is_empty()
                    && !infoset_children[0].nil
                    && infoset_children[0]
                        .text
                        .as_ref()
                        .map(|t| t.is_empty())
                        .unwrap_or(true);
                if empty_present {
                    let empty_val = match program.node(child_id) {
                        Ok(IrNode::Element {
                            kind: crate::ir::ValueKind::Complex,
                            ..
                        }) => DfdlValue::sequence(BTreeMap::new()),
                        _ => DfdlValue::string(""),
                    };
                    map.insert(elem_name.to_string(), empty_val);
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
            IrNode::Sequence { .. } => {
                let nested = infoset_particle_to_value(program, child_id, node)?;
                if let DfdlValue::Sequence(nested) = nested {
                    map.extend(nested.fields);
                }
            }
            IrNode::Choice { branches, .. } => {
                if let Ok(DfdlValue::Sequence(matched)) =
                    choice_matched_branch_value(program, branches, node)
                {
                    map.extend(matched.fields);
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

/// When infoset lexical is invalid for the schema type, keep the text so unparse can report
/// `Unparse Error` / `not a valid xs:…` (Daffodil section 5 unparseError tests).
fn infoset_lexical_fallback(trimmed: &str, kind: ValueKind) -> DfdlValue {
    match kind {
        ValueKind::Integer => DfdlValue::Integer(trimmed.to_string()),
        ValueKind::Decimal => DfdlValue::Decimal(trimmed.to_string()),
        ValueKind::DateTime | ValueKind::Time => DfdlValue::DateTime(trimmed.to_string()),
        _ => DfdlValue::string(trimmed),
    }
}

fn parse_scalar_for_kind(text: &str, kind: ValueKind) -> Result<DfdlValue, String> {
    let trimmed = text.trim();
    match kind {
        ValueKind::String => Ok(DfdlValue::string(trimmed)),
        ValueKind::Boolean => trimmed
            .parse::<bool>()
            .or_else(|_| match trimmed {
                "TRUE" | "true" | "1" => Ok(true),
                "FALSE" | "false" | "0" => Ok(false),
                _ => Err(()),
            })
            .map(DfdlValue::Boolean)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::Byte => trimmed
            .parse::<i64>()
            .map(|v| {
                i8::try_from(v)
                    .map(DfdlValue::Byte)
                    .unwrap_or(DfdlValue::Long(v))
            })
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::Short => trimmed
            .parse::<i16>()
            .map(DfdlValue::Short)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::Int => trimmed
            .parse::<i32>()
            .map(DfdlValue::Int)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::Integer => Ok(DfdlValue::Integer(trimmed.to_string())),
        ValueKind::Long => trimmed
            .parse::<i64>()
            .map(DfdlValue::Long)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::UnsignedByte => trimmed
            .parse::<u8>()
            .map(DfdlValue::UnsignedByte)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::UnsignedShort => trimmed
            .parse::<u16>()
            .map(DfdlValue::UnsignedShort)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::UnsignedInt => trimmed
            .parse::<u32>()
            .map(DfdlValue::UnsignedInt)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::Float => trimmed
            .parse::<f32>()
            .map(DfdlValue::Float)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::Double => trimmed
            .parse::<f64>()
            .map(DfdlValue::Double)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
        ValueKind::Decimal => Ok(DfdlValue::Decimal(trimmed.to_string())),
        ValueKind::DateTime | ValueKind::Time => Ok(DfdlValue::DateTime(trimmed.to_string())),
        ValueKind::HexBinary => decode_hex(trimmed)
            .map(DfdlValue::HexBinary)
            .or_else(|_| Ok(infoset_lexical_fallback(trimmed, kind))),
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
    parse_expected_infoset_with_context(xml, &TdmlResourceContext::default())
}

pub fn parse_expected_infoset_with_context(
    xml: &str,
    ctx: &TdmlResourceContext,
) -> Result<Vec<InfosetNode>, String> {
    parse_expected_infoset_with_target_ns(xml, ctx, None)
}

pub fn parse_expected_infoset_with_target_ns(
    xml: &str,
    ctx: &TdmlResourceContext,
    target_namespace: Option<&str>,
) -> Result<Vec<InfosetNode>, String> {
    let inner = resolve_infoset_inner_xml(xml, ctx)?;
    parse_infoset_elements_with_target_ns(&inner, target_namespace)
}

fn parse_infoset_elements_with_target_ns(
    xml: &str,
    target_namespace: Option<&str>,
) -> Result<Vec<InfosetNode>, String> {
    let _ = target_namespace;
    // Only declare `xmlns:ex` on the wrapper; do not treat targetNamespace as default element NS.
    parse_infoset_elements_with_default_ns(xml, None)
}

fn reject_doctype_in_resource(text: &str, resource_name: &str) -> Result<(), String> {
    if text.contains("<!DOCTYPE") || text.contains("<!doctype") {
        return Err(alloc::format!(
            "schema error: Schema Definition Error. DOCTYPE is disallowed when parsing infoset `{resource_name}`"
        ));
    }
    Ok(())
}

fn resolve_infoset_inner_xml(xml: &str, ctx: &TdmlResourceContext) -> Result<String, String> {
    let xml = xml.trim();
    if xml.is_empty() {
        return Ok(String::new());
    }
    let wrapped = alloc::format!("<wrapper>{xml}</wrapper>");
    let mut reader = XmlReader::new(&wrapped);
    reader.expect_start("wrapper").map_err(|e| e.to_string())?;
    reader.skip_insignificant_ws().map_err(|e| e.to_string())?;
    if reader.peek_start_local().ok().flatten().as_deref() != Some("dfdlInfoset") {
        return Ok(extract_dfdl_infoset_xml(xml));
    }
    let XmlEvent::StartElement { attributes, .. } =
        reader.next_event().map_err(|e| e.to_string())?
    else {
        return Err("expected dfdlInfoset".into());
    };
    let attrs = attrs_to_map(&attributes);
    let path = reader.read_inner_xml().map_err(|e| e.to_string())?;
    if attrs.get("type").map(String::as_str) == Some("file") {
        let path = path.trim();
        let bytes = load_tdml_resource(path, ctx)?;
        let text = core::str::from_utf8(&bytes)
            .map_err(|_| alloc::format!("infoset file `{path}` is not UTF-8"))?
            .to_string();
        reject_doctype_in_resource(&text, path)?;
        return Ok(extract_dfdl_infoset_xml(&strip_infoset_prolog(&text)));
    }
    Ok(path)
}

fn strip_infoset_prolog(text: &str) -> String {
    let mut s = text.trim();
    loop {
        if let Some(idx) = s.find("?>") {
            if s.starts_with("<?") {
                s = s[idx + 2..].trim_start();
                continue;
            }
        }
        if let Some(idx) = s.find("-->") {
            if s.starts_with("<!--") {
                s = s[idx + 3..].trim_start();
                continue;
            }
        }
        break;
    }
    s.to_string()
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
    parse_infoset_elements_with_default_ns(xml, None)
}

fn infoset_root_xmlns(target_namespace: Option<&str>, inner: &str) -> String {
    match target_namespace.filter(|u| !u.is_empty()) {
        Some(ns) => alloc::format!("<infosetRoot xmlns:ex=\"{ns}\">{inner}</infosetRoot>"),
        None => alloc::format!("<infosetRoot>{inner}</infosetRoot>"),
    }
}

fn parse_infoset_elements_with_default_ns(
    xml: &str,
    default_ns: Option<String>,
) -> Result<Vec<InfosetNode>, String> {
    let wrapped = infoset_root_xmlns(default_ns.as_deref(), xml);
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
            Some(_) => nodes.push(parse_infoset_element(&mut reader, default_ns.clone())?),
            None => break,
        }
    }
    Ok(nodes)
}

fn parse_infoset_element(
    reader: &mut XmlReader<'_>,
    inherited_default_ns: Option<String>,
) -> Result<InfosetNode, String> {
    let XmlEvent::StartElement { name, attributes, .. } = reader.next_event().map_err(|e| e.to_string())?
    else {
        return Err("expected infoset element".into());
    };
    let element_name = owned_local_name(&name).to_string();
    let attrs = attrs_to_map(&attributes);
    let mut default_ns = inherited_default_ns;
    if let Some(xmlns) = attrs.get("xmlns") {
        default_ns = Some(xmlns.clone());
    }
    let namespace = name
        .namespace
        .clone()
        .filter(|ns| !ns.is_empty())
        .or_else(|| default_ns.clone());
    let is_nil = attrs
        .get("xsi:nil")
        .or_else(|| attrs.get("{http://www.w3.org/2001/XMLSchema-instance}nil"))
        .is_some_and(|v| v == "true");

    if reader.peek_is_end(&element_name).map_err(|e| e.to_string())? {
        reader.expect_end(&element_name).map_err(|e| e.to_string())?;
        return Ok(InfosetNode {
            name: element_name,
            namespace,
            text: None,
            nil: is_nil,
            children: BTreeMap::new(),
            blob_bytes: None,
        });
    }

    reader.skip_insignificant_ws().map_err(|e| e.to_string())?;
    if reader.peek_is_end(&element_name).map_err(|e| e.to_string())? {
        reader.expect_end(&element_name).map_err(|e| e.to_string())?;
        return Ok(InfosetNode {
            name: element_name,
            namespace,
            text: None,
            nil: is_nil,
            children: BTreeMap::new(),
            blob_bytes: None,
        });
    }

    match reader.peek_start_local().map_err(|e| e.to_string())? {
        Some(_) => {
            let inner = reader.read_inner_xml().map_err(|e| e.to_string())?;
            let children = parse_infoset_elements_with_default_ns(&inner, default_ns.clone())?;
            let mut map: BTreeMap<String, Vec<InfosetNode>> = BTreeMap::new();
            for child in children {
                map.entry(child.name.clone()).or_default().push(child);
            }
            Ok(InfosetNode {
                name: element_name,
                namespace,
                text: None,
                nil: is_nil,
                children: map,
                blob_bytes: None,
            })
        }
        None => {
            let text = reader.read_text_until_end(&element_name).map_err(|e| e.to_string())?;
            Ok(InfosetNode {
                name: element_name,
                namespace,
                text: Some(text.trim().to_string()),
                nil: is_nil,
                children: BTreeMap::new(),
                blob_bytes: None,
            })
        }
    }
}

fn choice_branch_fields_for_infoset(
    discriminator: String,
    value: DfdlValue,
) -> BTreeMap<String, DfdlValue> {
    match (discriminator.as_str(), value) {
        ("choice", DfdlValue::Choice { discriminator, value }) => {
            choice_branch_fields_for_infoset(discriminator, *value)
        }
        ("sequence", DfdlValue::Sequence(seq)) => seq.fields,
        (name, v) => {
            let mut map = BTreeMap::new();
            map.insert(name.to_string(), v);
            map
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
            namespace: None,
            text: None,
            nil: false,
            children: seq
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), field_values_to_infoset_nodes(k, v)))
                .collect(),
            blob_bytes: None,
        },
        DfdlValue::Array(items) => InfosetNode {
            name: name.to_string(),
            namespace: None,
            text: None,
            nil: false,
            children: BTreeMap::from([(name.to_string(), items.iter().map(|v| value_to_node(name, v)).collect())]),
            blob_bytes: None,
        },
        DfdlValue::Choice { discriminator, value } => {
            let fields = choice_branch_fields_for_infoset(discriminator.clone(), (**value).clone());
            if fields.len() == 1 && fields.contains_key(discriminator.as_str()) {
                return value_to_node(discriminator, value);
            }
            InfosetNode {
                name: name.to_string(),
                namespace: None,
                text: None,
                nil: false,
                children: fields
                    .iter()
                    .map(|(k, v)| (k.clone(), field_values_to_infoset_nodes(k, v)))
                    .collect(),
                blob_bytes: None,
            }
        }
        DfdlValue::Null => InfosetNode {
            name: name.to_string(),
            namespace: None,
            text: None,
            nil: true,
            children: BTreeMap::new(),
            blob_bytes: None,
        },
        DfdlValue::Blob(bytes) => InfosetNode {
            name: name.to_string(),
            namespace: None,
            text: None,
            nil: false,
            children: BTreeMap::new(),
            blob_bytes: Some(bytes.clone()),
        },
        scalar => InfosetNode {
            name: name.to_string(),
            namespace: None,
            text: Some(scalar_to_string(scalar)),
            nil: false,
            children: BTreeMap::new(),
            blob_bytes: None,
        },
    }
}

fn field_values_to_infoset_nodes(name: &str, value: &DfdlValue) -> Vec<InfosetNode> {
    match value {
        DfdlValue::Array(items) => items.iter().map(|v| value_to_node(name, v)).collect(),
        other => vec![value_to_node(name, other)],
    }
}

fn format_float_for_infoset(v: f32) -> String {
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

fn f32_abs(v: f32) -> f32 {
    if v.is_sign_negative() { -v } else { v }
}

fn calendar_infoset_texts_equal(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    if expected.contains('T') && !expected.contains('+') && !expected.contains('Z') {
        if actual.starts_with(expected) {
            let rest = &actual[expected.len()..];
            if rest == "+00:00" || rest == "Z" {
                return true;
            }
        }
    }
    false
}

fn float_infoset_texts_equal(expected: &str, actual: &str) -> bool {
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

fn scalar_to_string(value: &DfdlValue) -> String {
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

/// Load blob bytes for unparse from a TDML `xs:anyURI` reference or `file:` URI.
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
        std::fs::read(&path).map_err(|_| {
            alloc::format!("Unable to open blob for reading: {uri}")
        })
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
        let reference = std::fs::read(&path).map_err(|e| {
            alloc::format!("blob reference read `{path}`: {e}")
        })?;
        if reference != actual {
            return Err(alloc::format!(
                "blob bytes mismatch for `{expected_uri}`: expected {} byte(s), got {} byte(s)",
                reference.len(),
                actual.len()
            ));
        }
        return Ok(());
    }
    #[cfg(not(feature = "std"))]
    {
        let _ = (expected_uri, actual);
        Err("blob infoset compare requires the `std` feature".into())
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
