use super::super::{ChoiceBranch, IrNode, IrProps, StringId, StringPool, ValueKind};
use super::element::{
    apply_restriction_facets, builtin_for_element_type_name, validate_dfdl_prop_overlap,
    validate_implicit_text_length, value_kind_from_builtin,
};
use super::props::{
    apply_format_default_delimiters, apply_type_name_ir_flags, dfdl_props_for_element_ref,
    effective_element_type_name, finalize_element_props, merge_ir_props,
    particle_inherited_for_children,
};
use super::validate::{
    validate_binary_calendar_compile, validate_fixed_occurs_count,
    validate_initiated_content_particle, validate_model_group_occurs,
    validate_text_standard_sibling_order,
};
use super::IrBuilder;
use crate::error::{Result, SchemaError};
use crate::schema::{
    validate_length_facets_for_type, DfdlProps, GroupDecl, LengthKind, Particle, SchemaDocument,
    TypeDef,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

impl<'a> IrBuilder<'a> {
    pub(crate) fn compile_particle(
        &mut self,
        particle: &Particle,
        inherited: &IrProps,
        prior_element_names: &[String],
    ) -> Result<u32> {
        self.compile_particle_inner(particle, inherited, prior_element_names, false)
    }

    pub(crate) fn compile_particle_inner(
        &mut self,
        particle: &Particle,
        inherited: &IrProps,
        prior_element_names: &[String],
        hidden: bool,
    ) -> Result<u32> {
        match particle {
            Particle::Element(element) => {
                super::validate::validate_ivc_on_element_decl(element, self.schema)?;
                super::validate::validate_ovc_on_element_decl(element, self.schema)?;
                let element_props = dfdl_props_for_element_ref(self.schema, element);
                if let Some(ref expr) = element_props.input_value_calc_expression {
                    super::validate::validate_schema_ivc_expression_prefixes(expr, self.schema)?;
                }
                if let Some(ref test) = element_props.discriminator_test {
                    let empty = alloc::collections::BTreeMap::new();
                    let prefixes = element_props
                        .discriminator_xpath_prefixes
                        .as_ref()
                        .unwrap_or(&empty);
                    crate::schema_validate::validate_discriminator_xpath_prefixes(test, prefixes)?;
                }
                let mut merged =
                    self.merge_props_full(inherited, &DfdlProps::default(), &element_props)?;
                apply_format_default_delimiters(&mut merged, &self.defaults);
                if element_props.length_kind.is_none()
                    && self.defaults.length_kind == LengthKind::Delimited
                {
                    merged.length_kind = LengthKind::Delimited;
                    merged.length_kind_defined = true;
                }
                validate_text_standard_sibling_order(&merged, prior_element_names, &self.strings)?;
                let name = self.strings.intern(&element.name);
                let type_name = effective_element_type_name(self.schema, element);
                if let Some(builtin) = builtin_for_element_type_name(self.schema, type_name) {
                    let kind = value_kind_from_builtin(builtin);
                    let mut ir_props = merged;
                    apply_type_name_ir_flags(type_name, &mut ir_props);
                    let mut ir_props = finalize_element_props(
                        kind,
                        ir_props,
                        &mut self.strings,
                        self.tunables,
                        Some(self.schema),
                        Some(element.name.as_str()),
                    )?;
                    if element_props.length_kind.is_none()
                        && self.defaults.length_kind == LengthKind::Delimited
                    {
                        ir_props.length_kind = LengthKind::Delimited;
                        ir_props.length_kind_defined = true;
                    }
                    let simple_base = self
                        .schema
                        .resolve_type(type_name)
                        .and_then(|t| match t {
                            crate::schema::TypeDef::Simple { base, .. } => Some(base.clone()),
                            _ => None,
                        })
                        .unwrap_or(crate::schema::SimpleBase::Builtin(builtin));
                    apply_restriction_facets(
                        self.schema,
                        &mut ir_props,
                        &simple_base,
                        &mut self.strings,
                        &element_props,
                    );
                    validate_binary_calendar_compile(kind, &ir_props, &self.strings)?;
                    validate_fixed_occurs_count(&ir_props)?;
                    ir_props.hidden = hidden;
                    validate_implicit_text_length(
                        kind,
                        &ir_props,
                        Some(element.name.as_str()),
                        Some(&self.strings),
                    )?;
                    ir_props.xsd_type = Some(self.strings.intern(type_name.as_str()));
                    Ok(self.push(IrNode::Element {
                        name,
                        kind,
                        props: ir_props,
                        child: None,
                    }))
                } else {
                    let props = merged;
                    if element.element_ref.is_some() {
                        if let Some(TypeDef::Simple {
                            props: type_props, ..
                        }) = self.schema.resolve_type(type_name)
                        {
                            validate_dfdl_prop_overlap(&element_props, type_props)?;
                        }
                    }
                    let compile_element_props_owned = match self.schema.resolve_type(type_name) {
                        Some(TypeDef::Simple { .. }) => {
                            element_props_for_simple_type_compile(&element_props)
                        }
                        _ => element_props.clone(),
                    };
                    let child = self.compile_type(
                        type_name,
                        &compile_element_props_owned,
                        Some(element.name.as_str()),
                        hidden,
                    )?;
                    let child_node = self.nodes.get(child as usize).ok_or_else(|| {
                        SchemaError::InvalidProperty {
                            message: alloc::format!("invalid child node id {child}"),
                        }
                    })?;
                    if let IrNode::Element {
                        kind,
                        props: child_props,
                        child: nested,
                        ..
                    } = child_node.clone()
                    {
                        if nested.is_none() && kind != ValueKind::Complex {
                            let overlay = props;
                            let mut merged_ir = merge_ir_props(&child_props, &overlay);
                            if element_props.representation.is_none() {
                                merged_ir.representation = child_props.representation;
                            }
                            if element_props.encoding.is_none() {
                                merged_ir.encoding = child_props.encoding;
                            }
                            if element_props.text_string_pad_character.is_none() {
                                merged_ir.text_string_pad_character =
                                    child_props.text_string_pad_character;
                                merged_ir.text_string_pad_character_property_form =
                                    child_props.text_string_pad_character_property_form;
                            }
                            if element_props.text_string_justification.is_none() {
                                merged_ir.text_string_justification =
                                    child_props.text_string_justification;
                            }
                            if element_props.text_number_pad_character.is_none() {
                                merged_ir.text_number_pad_character =
                                    child_props.text_number_pad_character;
                                merged_ir.text_number_pad_character_property_form =
                                    child_props.text_number_pad_character_property_form;
                            }
                            if element_props.text_trim_kind.is_none() {
                                merged_ir.text_trim_kind = child_props.text_trim_kind;
                            }
                            if element_props.text_number_justification.is_none() {
                                merged_ir.text_number_justification =
                                    child_props.text_number_justification;
                            }
                            if element_props.alignment_units.is_none() {
                                merged_ir.alignment_units = child_props.alignment_units;
                            }
                            if element_props.length_units.is_none() {
                                merged_ir.length_units = child_props.length_units;
                            }
                            if element_props.leading_skip.is_none() {
                                merged_ir.leading_skip = child_props.leading_skip;
                            }
                            if element_props.trailing_skip.is_none() {
                                merged_ir.trailing_skip = child_props.trailing_skip;
                            }
                            if let Some(lk) = element_props.length_kind {
                                merged_ir.length_kind = lk;
                            } else if element_props.length_kind.is_none() {
                                merged_ir.length_kind = child_props.length_kind;
                            }
                            if let Some(len) = element_props.length {
                                merged_ir.length = Some(len);
                            } else if element_props.length.is_none() {
                                merged_ir.length = child_props.length;
                            }
                            if merged_ir.terminator.is_none() {
                                merged_ir.terminator =
                                    child_props.terminator.or(self.defaults.terminator);
                            }
                            if merged_ir.initiator.is_none() {
                                merged_ir.initiator =
                                    child_props.initiator.or(self.defaults.initiator);
                            }
                            if element_props.alignment.is_none() {
                                merged_ir.alignment = child_props.alignment;
                                merged_ir.alignment_implicit = child_props.alignment_implicit;
                            }
                            if let Some(type_def) = self.schema.resolve_type(&element.type_name) {
                                if let TypeDef::Simple { base, .. } = type_def {
                                    validate_length_facets_for_type(
                                        self.schema,
                                        base,
                                        kind,
                                        &merged_ir,
                                        Some(element.name.as_str()),
                                    )?;
                                }
                            }
                            apply_type_name_ir_flags(&element.type_name, &mut merged_ir);
                            let mut merged = finalize_element_props(
                                kind,
                                merged_ir,
                                &mut self.strings,
                                self.tunables,
                                Some(self.schema),
                                Some(element.name.as_str()),
                            )?;
                            validate_fixed_occurs_count(&merged)?;
                            let simple_base = self
                                .schema
                                .resolve_type(&element.type_name)
                                .and_then(|t| match t {
                                    TypeDef::Simple { base, .. } => Some(base.clone()),
                                    _ => None,
                                })
                                .or_else(|| {
                                    crate::schema::BuiltinType::from_xsd(element.type_name.as_str())
                                        .map(crate::schema::SimpleBase::Builtin)
                                });
                            if let Some(base) = &simple_base {
                                apply_restriction_facets(
                                    self.schema,
                                    &mut merged,
                                    base,
                                    &mut self.strings,
                                    &element_props,
                                );
                                validate_binary_calendar_compile(kind, &merged, &self.strings)?;
                                if let Some(TypeDef::Simple {
                                    props: type_props,
                                    ..
                                }) = self.schema.resolve_type(&element.type_name)
                                {
                                    if let Some(signed) = type_props.decimal_signed {
                                        merged.decimal_signed = signed;
                                    }
                                }
                            }
                            validate_implicit_text_length(
                                kind,
                                &merged,
                                Some(element.name.as_str()),
                                Some(&self.strings),
                            )?;
                            merged.hidden = hidden;
                            merged.xsd_type = Some(self.strings.intern(element.type_name.as_str()));
                            return Ok(self.push(IrNode::Element {
                                name,
                                kind,
                                props: merged,
                                child: None,
                            }));
                        }
                    }
                    let mut ir_props = props;
                    if element_props.length_kind.is_none() {
                        ir_props.length_kind = LengthKind::Implicit;
                    }
                    let mut ir_props = finalize_element_props(
                        ValueKind::Complex,
                        ir_props,
                        &mut self.strings,
                        self.tunables,
                        Some(self.schema),
                        Some(element.name.as_str()),
                    )?;
                    validate_fixed_occurs_count(&ir_props)?;
                    ir_props.hidden = hidden;
                    Ok(self.push(IrNode::Element {
                        name,
                        kind: ValueKind::Complex,
                        props: ir_props,
                        child: Some(child),
                    }))
                }
            }
            Particle::GroupRef(gr) => {
                let qname = &gr.name;
                let group = self
                    .schema
                    .groups
                    .get(group_local_name(qname))
                    .ok_or_else(|| SchemaError::InvalidProperty {
                        message: alloc::format!("unknown group `{qname}`"),
                    })?;
                match group {
                    GroupDecl::Sequence(seq) => {
                        let ir_props = self.merge_props_full(inherited, &seq.props, &gr.props)?;
                        let child_inherited = particle_inherited_for_children(&ir_props);
                        let mut children = Vec::new();
                        let mut prior_element_names: Vec<String> = Vec::new();
                        for particle in &seq.particles {
                            children.push(self.compile_particle_inner(
                                particle,
                                &child_inherited,
                                &prior_element_names,
                                hidden,
                            )?);
                            if let Particle::Element(el) = particle {
                                prior_element_names.push(el.name.clone());
                            }
                        }
                        Ok(self.push(IrNode::Sequence {
                            children,
                            props: ir_props,
                        }))
                    }
                    GroupDecl::Choice(ch) => {
                        let ir_props = self.merge_props_full(inherited, &ch.props, &gr.props)?;
                        let child_inherited = particle_inherited_for_children(&ir_props);
                        let mut branches = Vec::new();
                        for branch in &ch.branches {
                            let node =
                                self.compile_particle_inner(branch, &child_inherited, &[], hidden)?;
                            branches.push(ChoiceBranch {
                                name: self.strings.intern(branch_name(branch)),
                                initiator: branch_initiator(branch, &mut self.strings),
                                branch_key: branch_choice_key(
                                    branch,
                                    self.schema,
                                    &mut self.strings,
                                ),
                                node,
                            });
                        }
                        Ok(self.push(IrNode::Choice {
                            branches,
                            props: ir_props,
                        }))
                    }
                }
            }
            Particle::Sequence(sequence) => {
                if sequence.props.hidden_group_ref.is_some()
                    && (sequence.props.hidden_group_ref_from_appinfo_sequence
                        || !sequence.particles.is_empty()
                        || sequence.had_markup_before_particles)
                {
                    return Err(SchemaError::InvalidProperty {
                        message:
                            "Schema Definition Error: A sequence with hiddenGroupRef cannot have children."
                                .into(),
                    }
                    .into());
                }
                validate_model_group_occurs("sequence", &sequence.props)?;
                let ir_props =
                    self.merge_props_full(inherited, &sequence.props, &DfdlProps::default())?;
                let child_inherited = particle_inherited_for_children(inherited);
                let mut children = Vec::new();
                let mut prior_element_names: Vec<String> = Vec::new();
                if let Some(ref href) = sequence.props.hidden_group_ref {
                    if href.is_empty() {
                        return Err(SchemaError::InvalidProperty {
                            message:
                                "Schema Definition Error: dfdl:hiddenGroupRef must be a valid QName"
                                    .into(),
                        }
                        .into());
                    }
                    let gname = group_local_name(href);
                    if gname.is_empty() {
                        return Err(SchemaError::InvalidProperty {
                            message:
                                "Schema Definition Error: dfdl:hiddenGroupRef must be a valid QName"
                                    .into(),
                        }
                        .into());
                    }
                    if self
                        .hidden_group_expand_stack
                        .iter()
                        .any(|s| s.as_str() == gname)
                    {
                        return Err(SchemaError::InvalidProperty {
                            message: "Schema Definition Error: Model group circular definitions. Group references, or hidden group references form a loop.".to_string(),
                        }
                        .into());
                    }
                    self.hidden_group_expand_stack.push(gname.to_string());
                    let group = self
                        .schema
                        .groups
                        .get(gname)
                        .ok_or_else(|| SchemaError::InvalidProperty {
                            message: alloc::format!(
                                "Schema Definition Error: Referenced group definition not found: {href}\nSchema context: group reference\n{gname}"
                            ),
                        })?;
                    let expand_result: Result<()> = match group {
                        GroupDecl::Sequence(seq) => {
                            let ir_props = self.merge_props_full(
                                inherited,
                                &seq.props,
                                &DfdlProps::default(),
                            )?;
                            let group_inherited = particle_inherited_for_children(inherited);
                            let mut group_children = Vec::new();
                            let mut group_prior: Vec<String> = Vec::new();
                            for particle in &seq.particles {
                                group_children.push(self.compile_particle_inner(
                                    particle,
                                    &group_inherited,
                                    &group_prior,
                                    true,
                                )?);
                                if let Particle::Element(el) = particle {
                                    group_prior.push(el.name.clone());
                                }
                            }
                            let inline_hidden =
                                seq.props.separator.as_deref().is_none_or(|s| s.is_empty())
                                    && seq.props.terminator.as_deref().is_none_or(|s| s.is_empty())
                                    && seq.props.initiator.as_deref().is_none_or(|s| s.is_empty());
                            if inline_hidden {
                                children.extend(group_children);
                            } else {
                                children.push(self.push(IrNode::Sequence {
                                    children: group_children,
                                    props: ir_props,
                                }));
                            }
                            Ok(())
                        }
                        GroupDecl::Choice(ch) => {
                            let ir_props =
                                self.merge_props_full(inherited, &ch.props, &DfdlProps::default())?;
                            let group_inherited = particle_inherited_for_children(inherited);
                            let mut branches = Vec::new();
                            for branch in &ch.branches {
                                let node = self.compile_particle_inner(
                                    branch,
                                    &group_inherited,
                                    &[],
                                    true,
                                )?;
                                branches.push(ChoiceBranch {
                                    name: self.strings.intern(branch_name(branch)),
                                    initiator: branch_initiator(branch, &mut self.strings),
                                    branch_key: branch_choice_key(
                                        branch,
                                        self.schema,
                                        &mut self.strings,
                                    ),
                                    node,
                                });
                            }
                            children.push(self.push(IrNode::Choice {
                                branches,
                                props: ir_props,
                            }));
                            Ok(())
                        }
                    };
                    self.hidden_group_expand_stack.pop();
                    expand_result?;
                }
                validate_implicit_unbounded_in_sequence(
                    &sequence.particles,
                    sequence.props.hidden_group_ref.is_some(),
                )?;
                for particle in &sequence.particles {
                    validate_initiated_content_particle(&sequence.props, particle)?;
                    children.push(self.compile_particle_inner(
                        particle,
                        &child_inherited,
                        &prior_element_names,
                        hidden,
                    )?);
                    if let Particle::Element(el) = particle {
                        prior_element_names.push(el.name.clone());
                    }
                }
                Ok(self.push(IrNode::Sequence {
                    children,
                    props: ir_props,
                }))
            }
            Particle::Choice(choice) => {
                validate_model_group_occurs("choice", &choice.props)?;
                if choice.branches.is_empty() {
                    return Err(SchemaError::InvalidProperty {
                        message:
                            "Schema Definition Error. choice element must contain one or more branches"
                                .into(),
                    }
                    .into());
                }
                let ir_props =
                    self.merge_props_full(inherited, &choice.props, &DfdlProps::default())?;
                let child_inherited = particle_inherited_for_children(inherited);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node =
                        self.compile_particle_inner(branch, &child_inherited, &[], hidden)?;
                    let name = branch_name(branch);
                    let initiator = branch_initiator(branch, &mut self.strings);
                    branches.push(ChoiceBranch {
                        name: self.strings.intern(&name),
                        initiator,
                        branch_key: branch_choice_key(branch, self.schema, &mut self.strings),
                        node,
                    });
                }
                Ok(self.push(IrNode::Choice {
                    branches,
                    props: ir_props,
                }))
            }
        }
    }

    pub(crate) fn compile_group_decl(
        &mut self,
        group: &GroupDecl,
        inherited: &IrProps,
        prior_element_names: &[String],
        hidden: bool,
    ) -> Result<u32> {
        match group {
            GroupDecl::Sequence(seq) => {
                let ir_props =
                    self.merge_props_full(inherited, &seq.props, &DfdlProps::default())?;
                let child_inherited = particle_inherited_for_children(inherited);
                let mut children = Vec::new();
                let mut prior: Vec<String> = prior_element_names.to_vec();
                for particle in &seq.particles {
                    children.push(self.compile_particle_inner(
                        particle,
                        &child_inherited,
                        &prior,
                        hidden,
                    )?);
                    if let Particle::Element(el) = particle {
                        prior.push(el.name.clone());
                    }
                }
                Ok(self.push(IrNode::Sequence {
                    children,
                    props: ir_props,
                }))
            }
            GroupDecl::Choice(ch) => {
                let ir_props =
                    self.merge_props_full(inherited, &ch.props, &DfdlProps::default())?;
                let child_inherited = particle_inherited_for_children(inherited);
                let mut branches = Vec::new();
                for branch in &ch.branches {
                    let node =
                        self.compile_particle_inner(branch, &child_inherited, &[], hidden)?;
                    branches.push(ChoiceBranch {
                        name: self.strings.intern(branch_name(branch)),
                        initiator: branch_initiator(branch, &mut self.strings),
                        branch_key: branch_choice_key(branch, self.schema, &mut self.strings),
                        node,
                    });
                }
                Ok(self.push(IrNode::Choice {
                    branches,
                    props: ir_props,
                }))
            }
        }
    }

    pub(crate) fn compile_complex(
        &mut self,
        content: &crate::schema::ComplexContent,
        type_base: &IrProps,
        hidden: bool,
    ) -> Result<u32> {
        use crate::schema::ComplexContent;
        match content {
            ComplexContent::Sequence(sequence) => {
                if sequence.props.hidden_group_ref.is_some()
                    && (sequence.props.hidden_group_ref_from_appinfo_sequence
                        || !sequence.particles.is_empty()
                        || sequence.had_markup_before_particles)
                {
                    return Err(SchemaError::InvalidProperty {
                        message:
                            "Schema Definition Error: A sequence with hiddenGroupRef cannot have children."
                                .into(),
                    }
                    .into());
                }
                if let Some(ref href) = sequence.props.hidden_group_ref {
                    if href.is_empty() {
                        return Err(SchemaError::InvalidProperty {
                            message:
                                "Schema Definition Error: dfdl:hiddenGroupRef must be a valid QName"
                                    .into(),
                        }
                        .into());
                    }
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: complex type cannot have sequence with a hiddenGroupRef model group"
                            .into(),
                    }
                    .into());
                }
                validate_model_group_occurs("sequence", &sequence.props)?;
                validate_implicit_unbounded_in_sequence(&sequence.particles, false)?;
                let mut ir_props =
                    self.merge_props_full(type_base, &sequence.props, &DfdlProps::default())?;
                if ir_props.separator.is_none() && sequence.props.separator.as_deref() != Some("") {
                    ir_props.separator = self.defaults.separator;
                }
                let mut child_inherited = particle_inherited_for_children(type_base);
                let mut children = Vec::new();
                let mut prior_element_names: Vec<String> = Vec::new();
                for particle in &sequence.particles {
                    validate_initiated_content_particle(&sequence.props, particle)?;
                    if let Particle::GroupRef(gr) = particle {
                        let qname = &gr.name;
                        let group =
                            self.schema
                                .groups
                                .get(group_local_name(qname))
                                .ok_or_else(|| SchemaError::InvalidProperty {
                                    message: alloc::format!("unknown group `{qname}`"),
                                })?;
                        match group {
                            GroupDecl::Sequence(seq) => {
                                ir_props =
                                    self.merge_props_full(&ir_props, &seq.props, &gr.props)?;
                                child_inherited = particle_inherited_for_children(type_base);
                                for p in &seq.particles {
                                    validate_initiated_content_particle(&seq.props, p)?;
                                    children.push(self.compile_particle_inner(
                                        p,
                                        &child_inherited,
                                        &prior_element_names,
                                        hidden,
                                    )?);
                                    if let Particle::Element(el) = p {
                                        prior_element_names.push(el.name.clone());
                                    }
                                }
                                continue;
                            }
                            GroupDecl::Choice(_) => {
                                children.push(self.compile_particle_inner(
                                    particle,
                                    &child_inherited,
                                    &prior_element_names,
                                    hidden,
                                )?);
                            }
                        }
                    } else {
                        children.push(self.compile_particle_inner(
                            particle,
                            &child_inherited,
                            &prior_element_names,
                            hidden,
                        )?);
                        if let Particle::Element(el) = particle {
                            prior_element_names.push(el.name.clone());
                        }
                    }
                }
                Ok(self.push(IrNode::Sequence {
                    children,
                    props: ir_props,
                }))
            }
            ComplexContent::Choice(choice) => {
                validate_model_group_occurs("choice", &choice.props)?;
                if choice.branches.is_empty() {
                    return Err(SchemaError::InvalidProperty {
                        message:
                            "Schema Definition Error. choice element must contain one or more branches"
                                .into(),
                    }
                    .into());
                }
                let ir_props =
                    self.merge_props_full(type_base, &choice.props, &DfdlProps::default())?;
                let child_inherited = particle_inherited_for_children(type_base);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node =
                        self.compile_particle_inner(branch, &child_inherited, &[], hidden)?;
                    branches.push(ChoiceBranch {
                        name: self.strings.intern(branch_name(branch)),
                        initiator: branch_initiator(branch, &mut self.strings),
                        branch_key: branch_choice_key(branch, self.schema, &mut self.strings),
                        node,
                    });
                }
                Ok(self.push(IrNode::Choice {
                    branches,
                    props: ir_props,
                }))
            }
            ComplexContent::Empty => Ok(self.push(IrNode::Sequence {
                children: Vec::new(),
                props: type_base.clone(),
            })),
        }
    }
}

pub(crate) fn group_local_name(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

pub(crate) fn branch_name(particle: &Particle) -> String {
    match particle {
        Particle::Element(e) => e.name.clone(),
        Particle::Sequence(_) => "sequence".to_string(),
        Particle::Choice(_) => "choice".to_string(),
        Particle::GroupRef(gr) => group_local_name(&gr.name).to_string(),
    }
}

pub(crate) fn branch_initiator(particle: &Particle, strings: &mut StringPool) -> Option<StringId> {
    let raw = match particle {
        Particle::Element(e) => e.props.initiator.as_deref(),
        Particle::Sequence(s) => s.props.initiator.as_deref(),
        Particle::Choice(c) => c.props.initiator.as_deref(),
        Particle::GroupRef(_) => None,
    };
    raw.map(|s| strings.intern(s))
}

pub(crate) fn branch_choice_key(
    particle: &Particle,
    schema: &SchemaDocument,
    strings: &mut StringPool,
) -> Option<StringId> {
    let raw = match particle {
        Particle::Element(e) => e.props.choice_branch_key.as_deref(),
        Particle::Sequence(s) => s.props.choice_branch_key.as_deref(),
        Particle::Choice(c) => c.props.choice_branch_key.as_deref(),
        Particle::GroupRef(gr) => gr.props.choice_branch_key.as_deref().or_else(|| {
            schema
                .groups
                .get(group_local_name(&gr.name))
                .and_then(|group| match group {
                    crate::schema::GroupDecl::Sequence(s) => s.props.choice_branch_key.as_deref(),
                    crate::schema::GroupDecl::Choice(c) => c.props.choice_branch_key.as_deref(),
                })
        }),
    };
    raw.map(|s| strings.intern(s))
}

fn element_props_for_simple_type_compile(element_props: &DfdlProps) -> DfdlProps {
    let mut out = element_props.clone();
    out.alignment = None;
    out.alignment_implicit = None;
    out
}

fn validate_implicit_unbounded_in_sequence(
    particles: &[Particle],
    is_hidden_expansion: bool,
) -> Result<()> {
    let mut saw_implicit_unbounded = false;
    for particle in particles {
        let Particle::Element(element) = particle else {
            continue;
        };
        let is_unbounded = element.props.occurs_max.is_none() && element.props.max_occurs_specified;
        let is_implicit =
            element.props.occurs_count_kind == Some(crate::schema::OccursCountKind::Implicit);
        if is_unbounded && is_implicit {
            if is_hidden_expansion {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: maxOccurs='unbounded' with occursCountKind='implicit' inside hidden sequence".to_string(),
                }.into());
            } else {
                if saw_implicit_unbounded {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Multiple maxOccurs='unbounded' elements with occursCountKind='implicit' in sequence".to_string(),
                    }.into());
                }
                saw_implicit_unbounded = true;
            }
        }
    }
    Ok(())
}
