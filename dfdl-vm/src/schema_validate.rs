pub(crate) mod choice_dispatch;
pub(crate) mod delimiters;
pub(crate) mod discriminators;
pub(crate) mod groups;
pub(crate) mod restrictions;
pub(crate) mod types;

use crate::error::SchemaError;
use crate::length_validate::DaffodilTunables;
use crate::schema::SchemaDocument;
use alloc::string::{String, ToString};

pub use discriminators::validate_discriminator_xpath_prefixes;

pub fn length_not_defined_message(schema: &SchemaDocument, element_name: Option<&str>) -> String {
    let mut msg = "Schema Definition Error: Property length is not defined".to_string();
    let Some(text) = schema.schema_source_text.as_deref() else {
        return msg;
    };
    let Some(label) = schema.schema_source_label.as_deref() else {
        return msg;
    };
    let needle = element_name
        .map(|n| alloc::format!("name=\"{n}\""))
        .unwrap_or_else(|| "lengthKind=\"explicit\"".to_string());
    for (i, line) in text.lines().enumerate() {
        if line.contains(&needle) {
            msg.push('\n');
            msg.push_str(label);
            msg.push('\n');
            msg.push_str(&alloc::format!("line {}", i + 1));
            if let Some(col) = line.find("<xs:element").map(|c| c + 2) {
                msg.push('\n');
                msg.push_str(&alloc::format!("column {col}"));
            }
            break;
        }
    }
    msg
}

fn schema_diagnostics_block_compile(schema: &SchemaDocument, root: &str) -> bool {
    if schema.schema_diagnostics.is_empty() {
        return false;
    }
    let ncname_only = schema
        .schema_diagnostics
        .iter()
        .all(|d| d.contains("not a valid NCName") || d == "NCName");
    if ncname_only && crate::schema::get_global_element(schema, root).is_some() {
        return false;
    }
    true
}

pub fn validate_compiled_schema(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if schema_diagnostics_block_compile(schema, root) {
        return Err(SchemaError::InvalidProperty {
            message: schema.schema_diagnostics.join("\n"),
        });
    }
    if !schema.dfdl_annotations_seen {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Non-DFDL Schema file".into(),
        });
    }
    types::validate_type_references(schema)?;
    types::validate_element_type_qnames(schema, root)?;
    types::validate_simple_restriction_bases(schema, root)?;
    types::validate_name_and_ref(schema)?;
    types::validate_element_default_values(schema)?;
    delimiters::validate_escape_separator_distinct(schema, root)?;
    restrictions::validate_invalid_restrictions(schema, root, tunables)?;
    restrictions::validate_max_hex_binary_length(schema, root, tunables)?;
    restrictions::validate_unique_particle_attribution(schema)?;
    delimiters::validate_sequence_separator_encoding(schema, root)?;
    discriminators::validate_discriminators_in_reachable_schema(schema, root)?;
    groups::validate_reachable_complex_type_model_groups(schema, root)?;
    groups::validate_group_definitions_no_hidden_group_ref(schema)?;
    groups::validate_hidden_group_ref_notation(schema)?;
    if let Err(msg) = crate::unparse_validate::validate_hidden_groups_unparse(schema, root) {
        return Err(SchemaError::InvalidProperty { message: msg });
    }
    unordered::validate_reachable_unordered_sequences(schema, root)?;
    choice_dispatch::validate_choice_dispatch_schema(schema, root)?;
    Ok(())
}

mod unordered;
