pub(crate) mod hidden_groups;
pub(crate) mod infoset;
pub(crate) mod value_map;

use crate::ir::IrProgram;
use crate::schema::SchemaDocument;
use crate::tdml::InfosetNode;
use alloc::format;
use alloc::string::String;

pub use hidden_groups::validate_hidden_groups_unparse;
#[allow(unused_imports)]
pub use value_map::validate_unparse_value_map;

pub fn validate_unparse_infoset_nodes(
    schema: &SchemaDocument,
    program: &IrProgram,
    root: &str,
    nodes: &[InfosetNode],
) -> Result<(), String> {
    validate_unparse_infoset_nodes_inner(schema, program, root, nodes, true)
}

pub fn validate_unparse_infoset_cardinality(
    schema: &SchemaDocument,
    program: &IrProgram,
    root: &str,
    nodes: &[InfosetNode],
) -> Result<(), String> {
    validate_unparse_infoset_nodes_inner(schema, program, root, nodes, false)
}

fn validate_unparse_infoset_nodes_inner(
    schema: &SchemaDocument,
    program: &IrProgram,
    root: &str,
    nodes: &[InfosetNode],
    enforce_element_form: bool,
) -> Result<(), String> {
    let enforce_element_form = enforce_element_form && schema.element_form_default_explicit;
    let qualified = schema.element_form_default_qualified;
    let tns = schema.target_namespace.as_deref();
    let root_node = nodes
        .iter()
        .find(|n| crate::xml_util::local_name_str(&n.name) == root)
        .ok_or_else(|| format!("infoset missing root `{root}`"))?;
    if let Some(ns) = root_node.namespace.as_deref().filter(|u| !u.is_empty()) {
        if tns.is_some_and(|expected| ns != expected) {
            return Err(format!(
                "Schema Definition Error: No global element {{{ns}}}{root}"
            ));
        }
    } else if let Some(ns) = tns.filter(|u| !u.is_empty()) {
        return Err(format!(
            "Unparse Error: expected element start\n{{{ns}}}{root}\n{{}}{root}"
        ));
    }
    let root_parent = infoset::parent_qname(root, tns, qualified);
    if enforce_element_form && crate::schema::get_global_element(schema, root).is_some() {
        let local = crate::xml_util::local_name_str(&root_node.name);
        let has_ns = root_node
            .namespace
            .as_deref()
            .is_some_and(|u| !u.is_empty());
        if !has_ns {
            return Err(format!(
                "Unparse Error: {} expected element start, but received start event for {} at {}",
                infoset::element_qname_in_errors(local, tns),
                infoset::qname_for_error(local, None),
                infoset::element_qname_in_errors(local, tns),
            ));
        }
    }
    if enforce_element_form {
        infoset::validate_element_form(root_node, qualified, tns, None, &root_parent, true)?;
    }
    validate_hidden_groups_unparse(schema, root)?;
    let root_id = program.root;
    infoset::validate_infoset_particle(
        program,
        root_id,
        root_node,
        qualified,
        tns,
        &root_parent,
        enforce_element_form,
    )?;
    Ok(())
}
