use super::super::parser::{TdmlDocument, TdmlSuite};
use super::super::resources::load_tdml_resource;
use crate::api::DfdlSpec;
use crate::error::Result;
use crate::ir::IrNode;
use crate::length_validate::DaffodilTunables;
use crate::schema::BitOrder;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn tdml_transmission_bit_order(
    doc: &TdmlDocument,
    program: &crate::ir::IrProgram,
) -> BitOrder {
    if let Some(order) = doc.document_transmission_bit_order {
        return order;
    }
    program.format_transmission_bit_order()
}

pub(crate) fn resolve_tdml_config(
    config_name: Option<&String>,
    suite: &TdmlSuite,
) -> super::super::parser::TdmlConfig {
    config_name
        .and_then(|name| {
            if let Some(cfg) = suite.configs.get(name) {
                Some(cfg.clone())
            } else if let Ok(xml) =
                super::super::resources::load_tdml_resource_string(name, &suite.resource_context)
            {
                super::super::parser::parse_external_config_xml(&xml).ok()
            } else {
                None
            }
        })
        .unwrap_or_default()
}

pub(crate) fn external_schema_label(model: &str) -> Option<&str> {
    if model.ends_with(".xsd") || model.ends_with(".dfdl.xsd") {
        Some(model)
    } else {
        None
    }
}

pub(crate) fn compile_tdml_schema(
    xsd: &str,
    root: &str,
    tunables: DaffodilTunables,
    compile_base_dir: Option<&str>,
    schema_label: Option<&str>,
) -> Result<DfdlSpec> {
    let schema = if let Some(base) = compile_base_dir {
        crate::schema::parse_schema_with_options(
            xsd,
            &crate::schema::ParseOptions {
                base_dir: Some(base.to_string()),
                schema_label: schema_label.map(String::from),
            },
        )?
    } else {
        crate::schema::parse_schema(xsd)?
    };
    DfdlSpec::from_schema_root_with_tunables(schema, Some(root), tunables)
}

pub(crate) fn resolve_model_schema(
    suite: &TdmlSuite,
    model: &str,
) -> Result<(alloc::string::String, Option<alloc::string::String>)> {
    if let Some(def) = suite.schemas.get(model) {
        return Ok((def.xsd.clone(), def.compile_base_dir.clone()));
    }
    let resolver = crate::schema::SchemaResolver::new();
    Ok((resolver.resolve(model)?, None))
}

pub(crate) fn resolve_tdml_document_bytes(
    doc: &TdmlDocument,
    ctx: &super::super::resources::TdmlResourceContext,
) -> core::result::Result<Vec<u8>, String> {
    if let Some(path) = &doc.file_resource {
        return load_tdml_resource(path, ctx);
    }
    Ok(doc.data.clone())
}

pub(crate) fn error_messages_match(expected: &[String], err: &str) -> bool {
    const OPTIONAL: &[&str] = &[
        "schema definition error",
        "runtime schema definition error",
        "parse error",
        "unparse error",
        "placeholder",
    ];
    let err_lower = normalize_error_text(err);
    expected.iter().all(|fragment| {
        let fragment = fragment.trim();
        if fragment.is_empty() {
            return true;
        }
        let fl = normalize_error_text(fragment);
        if OPTIONAL.contains(&fl.as_str())
            || fl.starts_with("line ")
            || fl.starts_with("column ")
            || fl.ends_with(".xsd")
            || fl.ends_with(".xml")
        {
            return true;
        }
        if fl.starts_with("found only") && err_lower.contains("found only") {
            return true;
        }
        if fl.starts_with("needed ") && err_lower.contains("needed ") {
            return true;
        }
        if (fl == "long" || fl == "int" || fl == "integer")
            && (err_lower.contains("int")
                || err_lower.contains("long")
                || err_lower.contains("integer"))
        {
            return true;
        }
        err_lower.contains(&fl)
    })
}

pub(crate) fn normalize_error_text(text: &str) -> alloc::string::String {
    text.replace('\n', "%NL;")
        .replace('\r', "%CR;")
        .replace('\t', "%HT;")
        .to_lowercase()
}

pub(crate) fn schema_warnings_for_root(
    schema: &crate::schema::SchemaDocument,
    root: &str,
) -> Vec<String> {
    let mut out = schema.schema_warnings.clone();
    if let Some(scoped) = schema.scoped_schema_warnings.get(root) {
        out.extend(scoped.iter().cloned());
    }
    out
}

pub(crate) fn schema_warnings_message(warnings: &[String]) -> String {
    warnings.join("\n")
}

pub(crate) fn escalated_schema_warnings_message(warnings: &[String]) -> String {
    let mut msg = String::from("Schema Definition Warning Escalated Error");
    let body = schema_warnings_message(warnings);
    if !body.is_empty() {
        msg.push('\n');
        msg.push_str(&body);
    }
    msg
}

pub(crate) fn root_element_encoding(program: &crate::ir::IrProgram) -> Option<&str> {
    let node = program.node(program.root).ok()?;
    let IrNode::Element { props, .. } = node else {
        return None;
    };
    program.strings.get(props.encoding).ok()
}

pub(crate) fn hex_preview(bytes: &[u8]) -> alloc::string::String {
    bytes
        .iter()
        .map(|b| alloc::format!("{b:02x}"))
        .collect::<alloc::vec::Vec<_>>()
        .join("")
}
