//! Helper functions for VM decoder choice dispatch and branch validation.

use super::*;
use crate::error::{Error, Result, VmError};
use crate::ir::{ChoiceBranch, IrNode, IrProgram, IrProps, StringId, StringPool, ValueKind};
use crate::schema::OccursCountKind;
use crate::value::DfdlValue;
use crate::vm::runtime::{encoding_name, read_simple};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

pub(crate) fn choice_branch_element_props(program: &IrProgram, node: u32) -> Option<&IrProps> {
    match &program.nodes[node as usize] {
        IrNode::Element { props, .. } => Some(props),
        IrNode::Sequence {
            props, children, ..
        } => {
            if props.discriminator_test.is_some() {
                Some(props)
            } else if let Some(&first) = children.first() {
                choice_branch_element_props(program, first)
            } else {
                Some(props)
            }
        }
        IrNode::Choice { props, .. } => Some(props),
        _ => None,
    }
}

pub(crate) fn choice_branch_first_element(
    program: &IrProgram,
    node: u32,
) -> Option<(&IrProps, ValueKind)> {
    match &program.nodes[node as usize] {
        IrNode::Element { props, kind, .. } => Some((props, *kind)),
        IrNode::Sequence { children, .. } => {
            if let Some(&first) = children.first() {
                choice_branch_first_element(program, first)
            } else {
                None
            }
        }
        IrNode::Choice { branches, .. } => {
            if let Some(first_branch) = branches.first() {
                choice_branch_first_element(program, first_branch.node)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(crate) fn peek_choice_discriminator_dot(
    program: &IrProgram,
    branches: &[ChoiceBranch],
    cursor: &Cursor,
    strings: &StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Option<String> {
    for branch in branches {
        let Some((props, kind)) = choice_branch_first_element(program, branch.node) else {
            continue;
        };
        let mut c = cursor.clone();
        if let Ok(value) = read_simple(
            &mut c,
            kind,
            props,
            strings,
            false,
            &[],
            None,
            tunables,
            false,
            None,
            None,
            false,
            false,
            None,
        ) {
            return Some(dfdl_value_dispatch_string(&value));
        }
    }
    None
}

pub(crate) fn choice_dispatch_key_string(
    props: &IrProps,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<Option<alloc::string::String>> {
    if props.choice_dispatch_literal.is_some()
        || props.choice_dispatch_sibling.is_some()
        || props.choice_dispatch_path.is_some()
        || props.choice_dispatch_sibling_int.is_some()
    {
        if let Some(id) = props.choice_dispatch_literal {
            return Ok(Some(strings.get(id)?.to_string()));
        }
        if let Some(id) = props.choice_dispatch_sibling_int {
            let name = strings.get(id)?;
            let value = siblings
                .and_then(|m| m.get(name))
                .map(|s| &s.value)
                .ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("choice dispatch sibling `{name}` not available"),
                })?;
            let text = dfdl_value_dispatch_string(value);
            let trimmed = text.trim();
            let n: i64 = trimmed.parse().map_err(|_| VmError::InvalidValue {
                message: alloc::format!("Parse Error. Cannot convert `{trimmed}` to xs:int"),
            })?;
            return Ok(Some(n.to_string()));
        }
        if let Some(id) = props.choice_dispatch_sibling {
            let name = strings.get(id)?;
            let value = siblings
                .and_then(|m| m.get(name))
                .map(|s| &s.value)
                .ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("choice dispatch sibling `{name}` not available"),
                })?;
            return Ok(Some(dfdl_value_dispatch_string(value)));
        }
        if let Some(steps) = props.choice_dispatch_path.as_ref() {
            let value = eval_infoset_path_steps(steps, siblings, strings, tunables, None)?;
            return Ok(Some(dfdl_value_dispatch_string(&value)));
        }
    }
    Ok(None)
}

pub(crate) fn sibling_discriminator_values(
    value: &DfdlValue,
) -> alloc::vec::Vec<alloc::string::String> {
    match value {
        DfdlValue::Array(items) => items
            .iter()
            .flat_map(sibling_discriminator_values)
            .collect(),
        other => alloc::vec![dfdl_value_dispatch_string(other)],
    }
}

pub(crate) fn ir_props_is_dfdl_optional(props: &IrProps) -> bool {
    match (props.occurs_min, props.occurs_max) {
        (1, Some(1)) => false,
        (1, None) => false,
        (0, max) => match props.occurs_count_kind {
            OccursCountKind::Parsed | OccursCountKind::Expression => false,
            OccursCountKind::Implicit | OccursCountKind::Fixed => max == Some(1),
        },
        _ => false,
    }
}

pub(crate) fn choice_branch_term_node(program: &IrProgram, node_id: u32) -> Result<u32> {
    match program.node(node_id)? {
        IrNode::Sequence { children, .. } if children.len() == 1 => Ok(children[0]),
        _ => Ok(node_id),
    }
}

pub(crate) fn choice_branch_is_optional_element(
    program: &IrProgram,
    branch_node: u32,
) -> Result<bool> {
    let term = choice_branch_term_node(program, branch_node)?;
    match program.node(term)? {
        IrNode::Element { props, .. } => Ok(ir_props_is_dfdl_optional(props)),
        _ => Ok(false),
    }
}

pub(crate) fn validate_choice_branches_non_optional_runtime(
    program: &IrProgram,
    branches: &[ChoiceBranch],
) -> Result<()> {
    for branch in branches {
        let optional = choice_branch_is_optional_element(program, branch.node)?;
        if optional {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Branch of choice must be non-optional.".into(),
            }
            .into());
        }
    }
    Ok(())
}

pub(crate) fn ir_element_has_input_value_calc(props: &IrProps) -> bool {
    props.input_value_calc.is_some()
        || props.input_value_calc_literal.is_some()
        || props.input_value_calc_sibling.is_some()
        || props.input_value_calc_segments.is_some()
        || props.input_value_calc_path.is_some()
        || props.input_value_calc_expression.is_some()
}

pub(crate) fn choice_branch_has_input_value_calc(
    program: &IrProgram,
    branch_node: u32,
) -> Result<bool> {
    match program.node(branch_node)? {
        IrNode::Element { props, .. } if !props.hidden => {
            Ok(ir_element_has_input_value_calc(props))
        }
        _ => Ok(false),
    }
}

pub(crate) fn validate_choice_branches_not_ivc_runtime(
    program: &IrProgram,
    branches: &[ChoiceBranch],
) -> Result<()> {
    for branch in branches {
        if choice_branch_has_input_value_calc(program, branch.node)? {
            return Err(VmError::InvalidValue {
                message:
                    "Schema Definition Error: Branch of choice cannot have the dfdl:inputValueCalc property."
                        .into(),
            }
            .into());
        }
    }
    Ok(())
}

pub(crate) fn validate_choice_branch_element_name_upa_runtime(
    program: &IrProgram,
    branches: &[ChoiceBranch],
) -> Result<()> {
    let mut seen: BTreeMap<alloc::string::String, ()> = BTreeMap::new();
    for branch in branches {
        let term = choice_branch_term_node(program, branch.node)?;
        let IrNode::Element { name, .. } = program.node(term)? else {
            continue;
        };
        let local = program.strings.get(*name)?.to_string();
        if seen.contains_key(&local) {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Schema Definition Error: Unique Particle Attribution violation for element '{local}' in choice"
                ),
            }
            .into());
        }
        seen.insert(local, ());
    }
    Ok(())
}

pub(crate) fn parse_discriminator_path_step(step: &str) -> Result<(&str, Option<usize>)> {
    let step = step.trim();
    if let Some(open) = step.find('[') {
        if !step.ends_with(']') {
            return Err(VmError::InvalidValue {
                message: "invalid discriminator path step".into(),
            }
            .into());
        }
        let local = step[..open].trim();
        let idx: usize =
            step[open + 1..step.len() - 1]
                .trim()
                .parse()
                .map_err(|_| VmError::InvalidValue {
                    message: "invalid discriminator array index".into(),
                })?;
        return Ok((local, Some(idx)));
    }
    Ok((step, None))
}

pub(crate) fn single_or_array_item_at(
    value: &DfdlValue,
    one_based_index: usize,
) -> Result<DfdlValue> {
    if one_based_index == 1 && matches!(value, DfdlValue::Sequence(_)) {
        return Ok(value.clone());
    }
    array_item_at(value, one_based_index)
}

pub(crate) fn array_item_at(value: &DfdlValue, one_based_index: usize) -> Result<DfdlValue> {
    let DfdlValue::Array(items) = value else {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Indexing is only allowed on arrays".into(),
        }
        .into());
    };
    let idx = one_based_index
        .checked_sub(1)
        .ok_or_else(|| VmError::InvalidValue {
            message: "invalid discriminator array index".into(),
        })?;
    items.get(idx).cloned().ok_or_else(|| {
        VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: No element corresponding to step index {one_based_index} found."
            ),
        }
        .into()
    })
}

pub(crate) fn navigate_discriminator_path_step(
    value: &DfdlValue,
    local: &str,
    index: Option<usize>,
) -> Result<DfdlValue> {
    let field =
        sequence_field_by_local_value(value, local).ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: No element corresponding to step {local} found."
            ),
        })?;
    if let Some(idx) = index {
        single_or_array_item_at(field, idx)
    } else {
        Ok(field.clone())
    }
}

pub(crate) fn sequence_field_by_local_value<'a>(
    value: &'a DfdlValue,
    local: &str,
) -> Option<&'a DfdlValue> {
    match value {
        DfdlValue::Sequence(seq) => sequence_field_by_local(&seq.fields, local),
        _ => None,
    }
}

pub(crate) fn choice_branch_names_collide(branches: &[ChoiceBranch], name: StringId) -> bool {
    branches.iter().filter(|b| b.name == name).count() > 1
}

pub(crate) fn choice_branch_leading_element_name(
    program: &IrProgram,
    branch_node: u32,
) -> Result<Option<alloc::string::String>> {
    let term = choice_branch_term_node(program, branch_node)?;
    match program.node(term)? {
        IrNode::Element { name, .. } => Ok(Some(program.strings.get(*name)?.to_string())),
        IrNode::Sequence { children, .. } => {
            for &child in children {
                if let IrNode::Element { name, .. } = program.node(child)? {
                    return Ok(Some(program.strings.get(*name)?.to_string()));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

pub(crate) fn choice_branch_discriminator_for_infoset(
    branch: &ChoiceBranch,
    _branches: &[ChoiceBranch],
    strings: &StringPool,
    program: &IrProgram,
) -> Result<alloc::string::String> {
    if let Some(key_id) = branch.branch_key {
        if let Ok(key) = strings.get(key_id) {
            return Ok(key.to_string());
        }
    }
    if !matches!(program.node(branch.node), Ok(IrNode::Choice { .. })) {
        if let Ok(Some(elem_name)) = choice_branch_leading_element_name(program, branch.node) {
            return Ok(elem_name);
        }
    }
    Ok(strings.get(branch.name)?.to_string())
}

pub(crate) fn choice_branch_discriminator_matches_name(
    program: &IrProgram,
    branch: &ChoiceBranch,
    discriminator: &str,
) -> bool {
    if discriminator.is_empty() {
        return true;
    }
    if branch
        .branch_key
        .and_then(|id| program.strings.get(id).ok())
        .is_some_and(|k| k == discriminator)
    {
        return true;
    }
    if choice_branch_leading_element_name(program, branch.node)
        .ok()
        .flatten()
        .is_some_and(|n| n == discriminator)
    {
        return true;
    }
    program
        .strings
        .get(branch.name)
        .ok()
        .is_some_and(|n| n == discriminator)
}

pub(crate) fn sibling_state<'a>(
    props: &IrProps,
    siblings: Option<&'a BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
) -> Result<&'a SiblingState> {
    let name_id = props
        .input_value_calc_literal
        .or(props.input_value_calc_sibling)
        .ok_or_else(|| VmError::InvalidValue {
            message: "missing sibling name for boolean inputValueCalc".into(),
        })?;
    let name = strings.get(name_id)?;
    siblings
        .and_then(|m| m.get(name))
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("inputValueCalc sibling `{name}` not available"),
        })
        .map_err(Into::into)
}

pub(crate) fn format_choice_branch_error(
    branch: &ChoiceBranch,
    strings: &StringPool,
    err: &Error,
) -> String {
    let branch_name = strings.get(branch.name).unwrap_or("?");
    let msg = err.to_string();
    let msg = msg.strip_prefix("vm error: ").unwrap_or(msg.as_str());
    if msg.contains("Init('") || msg.contains("initiator mismatch") {
        if let Some(id) = branch.initiator {
            if let Ok(pat) = strings.get(id) {
                if msg.contains("Was looking for") {
                    let clean_msg = if let Some(idx) = msg.find("Delimiter not found") {
                        &msg[idx..]
                    } else {
                        msg
                    };
                    return alloc::format!(
                        "{branch_name}: Initiator '{pat}' not found. Alternative failed. Reason(s): List(Parse Error: Init('{pat}') - {branch_name}: {clean_msg})"
                    );
                }
                return alloc::format!("{branch_name}: Initiator '{pat}' not found");
            }
        }
        return alloc::format!("{branch_name}: Initiator not found");
    }
    if msg.contains("unexpected end of input") || msg.is_empty() {
        return alloc::format!("{branch_name}: {msg}");
    }
    msg.to_string()
}

impl<'a> Decoder<'a> {
    pub(crate) fn choice_branch_lacks_initiator(&self, branch_node: u32) -> Result<bool> {
        let Some(props) = choice_branch_element_props(self.ctx.program, branch_node) else {
            return Ok(true);
        };
        let Some(id) = props.initiator else {
            return Ok(true);
        };
        Ok(self.ctx.strings().get(id)?.is_empty())
    }

    pub(crate) fn choice_branch_initiator_present(
        &self,
        cursor: &Cursor<'_>,
        branch_node: u32,
    ) -> Result<bool> {
        let Some(props) = choice_branch_element_props(self.ctx.program, branch_node) else {
            return Ok(false);
        };
        self.initiator_present_at_cursor(cursor, props)
    }

    pub(crate) fn initiator_present_at_cursor(
        &self,
        cursor: &Cursor<'_>,
        props: &IrProps,
    ) -> Result<bool> {
        let Some(id) = props.initiator else {
            return Ok(false);
        };
        let pat = self.resolve_delimiter_property(
            self.ctx.strings().get(id)?,
            self.current_delimiter_occurrence_index(),
        );
        if pat.is_empty() {
            return Ok(false);
        }
        let enc = encoding_name(props, self.ctx.strings()).ok();
        Ok(crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            &pat,
            props.ignore_case,
            enc,
        )
        .is_some())
    }

    pub(crate) fn choice_branch_needs_post_decode_facet_check(&self, branch_node: u32) -> bool {
        let Some((props, _)) = choice_branch_first_element(self.ctx.program, branch_node) else {
            return false;
        };
        crate::vm::facet_validate::needs_choice_discriminator_facet_check(props)
    }

    pub(crate) fn validate_choice_branch_value(
        &self,
        branch_node: u32,
        value: &DfdlValue,
    ) -> Result<()> {
        let Some((props, kind)) = choice_branch_first_element(self.ctx.program, branch_node) else {
            return Ok(());
        };
        crate::vm::facet_validate::validate_choice_discriminator_facets(
            value,
            kind,
            props,
            self.ctx.strings(),
        )?;
        Ok(())
    }

    pub(crate) fn decode_choice(
        &self,
        node_id: u32,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        stop_sequences: &[&IrProps],
    ) -> Result<DfdlValue> {
        let IrNode::Choice { branches, props } = self.ctx.program.node(node_id)? else {
            return Err(VmError::TypeMismatch {
                expected: "choice".into(),
            }
            .into());
        };
        validate_choice_branches_not_ivc_runtime(self.ctx.program, branches)?;
        validate_choice_branch_element_name_upa_runtime(self.ctx.program, branches)?;
        validate_choice_branches_non_optional_runtime(self.ctx.program, branches)?;

        self.evaluate_and_set_variables(&props.set_variables, siblings)?;
        let _var_scope =
            self.enter_variable_scope(&props.new_variable_instances, siblings, false)?;

        let mut child_stops = stop_sequences.to_vec();
        if props.separator.is_some()
            || crate::vm::runtime::has_non_empty_terminator(props, self.ctx.strings())?
        {
            child_stops.push(props);
        }

        let dispatch_key = choice_dispatch_key_string(
            props,
            siblings,
            self.ctx.strings(),
            &self.ctx.program.tunables,
        )?;
        let using_dispatch = dispatch_key.is_some();
        let dot_peek = if using_dispatch {
            None
        } else {
            peek_choice_discriminator_dot(
                self.ctx.program,
                branches,
                cursor,
                self.ctx.strings(),
                &self.ctx.program.tunables,
            )
        };
        let dot = dot_peek.as_deref().unwrap_or("");
        let choice_start = cursor.pos;
        let mut branch_errors = Vec::new();

        for branch in branches {
            if let Some(ref d_key) = dispatch_key {
                if !choice_branch_discriminator_matches_name(self.ctx.program, branch, d_key) {
                    continue;
                }
            } else if !self.choice_branch_discriminator_matches(branch.node, dot) {
                continue;
            }

            let mut branch_cursor = cursor.clone();
            let saved_discriminator_state = self.discriminator_committed_branch.get();
            self.discriminator_committed_branch.set(false);

            let res = self.decode_node(
                branch.node,
                &mut branch_cursor,
                has_following_sibling,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                &child_stops,
            );

            let branch_committed = self.discriminator_committed_branch.get();
            self.discriminator_committed_branch
                .set(saved_discriminator_state || branch_committed);

            match res {
                Ok(val) => {
                    if self.choice_branch_needs_post_decode_facet_check(branch.node) {
                        if let Err(e) = self.validate_choice_branch_value(branch.node, &val) {
                            if branch_committed {
                                return Err(e);
                            }
                            branch_errors.push(format_choice_branch_error(
                                branch,
                                self.ctx.strings(),
                                &e,
                            ));
                            cursor.pos = choice_start;
                            continue;
                        }
                    }
                    *cursor = branch_cursor;
                    let disc_name = choice_branch_discriminator_for_infoset(
                        branch,
                        branches,
                        self.ctx.strings(),
                        self.ctx.program,
                    )?;
                    return Ok(DfdlValue::Choice {
                        discriminator: disc_name,
                        value: alloc::boxed::Box::new(val),
                    });
                }
                Err(e) => {
                    if branch_committed || is_schema_definition_error(&e) {
                        return Err(e);
                    }
                    branch_errors.push(format_choice_branch_error(branch, self.ctx.strings(), &e));
                    cursor.pos = choice_start;
                }
            }
        }

        if let Some(frame) = choice_explicit_frame_bytes(props, cursor, self.ctx.strings())? {
            cursor.pos = choice_start.saturating_add(frame);
        }
        if using_dispatch && !branch_errors.is_empty() {
            branch_errors.insert(0, "Choice dispatch branch failed".into());
        }
        Err(VmError::InvalidChoice { branch_errors }.into())
    }
}
