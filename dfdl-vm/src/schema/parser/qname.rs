use crate::schema::ast::{GlobalElement, SchemaDocument, TypeName};
use alloc::collections::BTreeMap;
use alloc::string::String;

pub fn normalize_qname(qname: &str) -> String {
    qname
        .rsplit_once(':')
        .map(|(_, l)| l)
        .unwrap_or(qname)
        .to_string()
}

pub fn namespace_uri_matches_prefix(uri: &str, prefix: &str) -> bool {
    if uri.contains(prefix) {
        return true;
    }
    let p_lower = prefix.to_lowercase();
    let uri_lower = uri.to_lowercase();
    uri_lower.contains(&p_lower)
}

pub fn format_storage_key(local: &str, namespace: Option<&str>) -> String {
    match namespace {
        Some(ns) if !ns.is_empty() => alloc::format!("{ns}|{local}"),
        _ => alloc::format!("|{local}"),
    }
}

pub fn format_local_from_storage_key(key: &str) -> &str {
    key.rsplit_once('|').map(|(_, local)| local).unwrap_or(key)
}

pub fn resolve_global_element_storage_key(schema: &SchemaDocument, qname: &str) -> String {
    let local = normalize_qname(qname);
    if let Some((prefix, _)) = qname.split_once(':') {
        if let Some(uri) = schema.namespace_prefixes.get(prefix) {
            return format_storage_key(&local, Some(uri.as_str()));
        }
    }
    format_storage_key(&local, schema.target_namespace.as_deref())
}

pub fn unique_global_element_by_local<'a>(
    schema: &'a SchemaDocument,
    local: &str,
) -> Option<&'a GlobalElement> {
    let mut matches = schema
        .global_elements
        .iter()
        .filter(|(k, _)| format_local_from_storage_key(k) == local);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.1)
}

pub fn global_element_by_prefixed_name<'a>(
    schema: &'a SchemaDocument,
    prefix: &str,
    local: &str,
) -> Option<&'a GlobalElement> {
    let mut matches = schema.global_elements.iter().filter(|(k, _)| {
        format_local_from_storage_key(k) == local
            && k.contains('|')
            && !k.starts_with('|')
            && k.split_once('|')
                .map(|(ns, _)| namespace_uri_matches_prefix(ns, prefix))
                .unwrap_or(false)
    });
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.1)
}

pub fn get_global_element<'a>(
    schema: &'a SchemaDocument,
    qname: &str,
) -> Option<&'a GlobalElement> {
    let local = normalize_qname(qname);
    if let Some((prefix, _)) = qname.split_once(':') {
        let key = resolve_global_element_storage_key(schema, qname);
        if let Some(g) = schema.global_elements.get(&key) {
            if g.type_name.as_str() != "xs:string" {
                return Some(g);
            }
        }
        if let Some(g) = global_element_by_prefixed_name(schema, prefix, &local) {
            return Some(g);
        }
    }
    let key = resolve_global_element_storage_key(schema, qname);
    if let Some(g) = schema.global_elements.get(&key) {
        return Some(g);
    }
    if !qname.contains(':') {
        if let Some(g) = unique_global_element_by_local(schema, qname) {
            return Some(g);
        }
    }
    let fallback = format_storage_key(&local, None);
    if fallback != key {
        if let Some(g) = schema.global_elements.get(&fallback) {
            return Some(g);
        }
    }
    schema
        .global_elements
        .get(qname)
        .or_else(|| schema.global_elements.get(&local))
}

pub fn get_global_element_error<'a>(
    schema: &'a SchemaDocument,
    qname: &str,
) -> Option<&'a crate::error::Error> {
    let local = normalize_qname(qname);
    if let Some((_, _)) = qname.split_once(':') {
        let key = resolve_global_element_storage_key(schema, qname);
        if let Some(e) = schema.global_element_errors.get(&key) {
            return Some(e);
        }
    }
    let key = resolve_global_element_storage_key(schema, qname);
    if let Some(e) = schema.global_element_errors.get(&key) {
        return Some(e);
    }
    if !qname.contains(':') {
        let mut matches = schema
            .global_element_errors
            .iter()
            .filter(|(k, _)| format_local_from_storage_key(k) == local);
        if let Some((_, e)) = matches.next() {
            return Some(e);
        }
    }
    let fallback = format_storage_key(&local, None);
    if fallback != key {
        if let Some(e) = schema.global_element_errors.get(&fallback) {
            return Some(e);
        }
    }
    schema
        .global_element_errors
        .get(qname)
        .or_else(|| schema.global_element_errors.get(&local))
}

pub(crate) fn resolve_type_qname_in_schema(
    doc: &SchemaDocument,
    type_attr: &str,
    scope: Option<&BTreeMap<String, String>>,
) -> core::result::Result<TypeName, crate::error::SchemaError> {
    use crate::error::SchemaError;
    use crate::schema::BuiltinType;
    const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";
    let mut prefix_map = doc.namespace_prefixes.clone();
    if let Some(s) = scope {
        for (k, v) in s {
            prefix_map.insert(k.clone(), v.clone());
        }
    }
    if !type_attr.contains(':') {
        let local = normalize_qname(type_attr);
        if BuiltinType::from_xsd(&local).is_some() {
            let q = if local.contains(':') {
                local
            } else {
                alloc::format!("xs:{local}")
            };
            return Ok(TypeName::new(q));
        }
        if doc.types.contains_key(&TypeName::new(&local)) {
            return Ok(TypeName::new(local));
        }
        return Ok(TypeName::new(local));
    }
    let Some((prefix, local)) = type_attr.split_once(':') else {
        return Ok(TypeName::new(type_attr));
    };
    let local = normalize_qname(local);
    let Some(uri) = prefix_map.get(prefix) else {
        return Ok(TypeName::new(local));
    };
    if uri == XSD_NS {
        let q = alloc::format!("xs:{local}");
        if BuiltinType::from_xsd(&q).is_some() {
            return Ok(TypeName::new(q));
        }
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: No type definition found for '{type_attr}'."
            ),
        });
    }
    if doc.types.contains_key(&TypeName::new(&local)) {
        return Ok(TypeName::new(local));
    }
    let xs_q = alloc::format!("xs:{local}");
    if prefix == "xs" && BuiltinType::from_xsd(&xs_q).is_some() {
        return Ok(TypeName::new(xs_q));
    }
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: No type definition found for '{type_attr}'."
        ),
    })
}

pub fn local_name_from_qname(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}
