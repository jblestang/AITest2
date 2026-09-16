use super::{ChoiceBranch, IrNode, IrProgram, IrPrefixLength, IrProps, StringId, StringPool, ValueKind};
use crate::error::{Result, SchemaError};
use crate::length_validate::{
    binary_length_validation_applies, validate_data_length_schema,
    validate_alignment_units_schema, validate_fill_byte_schema,
    validate_float_double_bit_length_schema, validate_packed_binary_properties_schema,
    validate_signed_one_bit_length_schema, validate_text_alignment_schema, DaffodilTunables,
};
use crate::schema::{
    BuiltinType, ComplexContent, DfdlProps, ElementDecl, GlobalElement, GroupDecl, LengthKind,
    LengthUnits,
    OccursCountKind, Particle, Representation, SchemaDocument, SimpleBase, TextTrimKind, TypeDef,
    TypeName, get_global_element,
    expand_entities_str,
    parse_text_standard_separator_list, parse_text_standard_zero_rep_list,
    validate_length_facets_for_type, validate_length_pattern,
    validate_text_standard_distinct_values,
    validate_text_standard_exponent_rep_literal,
    validate_text_standard_separator_literal,
    validate_text_standard_special_value_literal,
    validate_text_standard_zero_rep_literal,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

struct IrBuilder<'a> {
    schema: &'a SchemaDocument,
    nodes: Vec<IrNode>,
    strings: StringPool,
    defaults: IrProps,
    tunables: DaffodilTunables,
    /// Hidden group refs currently being expanded (cycle detection).
    hidden_group_expand_stack: Vec<String>,
}

/// Merge global element DFDL props when a particle uses `ref=` (forward refs may leave
/// particle props empty at parse time — noTargetNamespace_01).
/// Schema-wide `dfdl:format` defaults apply to element particles when not overridden;
/// `particle_inherited_for_children` clears group delimiters from inheritance.
fn apply_format_default_delimiters(ir: &mut IrProps, defaults: &IrProps) {
    if ir.initiator.is_none() {
        ir.initiator = defaults.initiator;
    }
    if ir.terminator.is_none() {
        ir.terminator = defaults.terminator;
    }
}

/// Local `ref=` particles parsed before a global declaration retain a stub `xs:string` type;
/// compile using the resolved global element type when available.
fn effective_element_type_name<'a>(
    schema: &'a SchemaDocument,
    element: &'a ElementDecl,
) -> &'a TypeName {
    if let Some(ref er) = element.element_ref {
        if let Some(g) = get_global_element(schema, er) {
            return &g.type_name;
        }
    }
    &element.type_name
}

fn dfdl_props_for_element_ref(schema: &SchemaDocument, element: &ElementDecl) -> DfdlProps {
    use crate::schema::merge_dfdl_props;
    let mut props = element.props.clone();
    if let Some(ref er) = element.element_ref {
        if let Some(g) = get_global_element(schema, er) {
            props = merge_dfdl_props(g.props.clone(), props);
            let imported = g
                .source_label
                .as_deref()
                .is_some_and(|s| s != schema.schema_source_label.as_deref().unwrap_or(""));
            if imported {
                if props.initiator.is_none() {
                    props.initiator = g
                        .format_context
                        .initiator
                        .clone()
                        .filter(|s| !s.is_empty());
                }
            }
        }
    }
    props
}

impl<'a> IrBuilder<'a> {
    fn new(schema: &'a SchemaDocument, tunables: DaffodilTunables) -> Result<Self> {
        let mut strings = StringPool::new();
        let mut defaults = overlay_dfdl_to_ir(
            IrProps::default(),
            &schema.format_defaults.props,
            &mut strings,
        )?;
        defaults.binary_packed_sign_codes = strings.intern(
            schema
                .format_defaults
                .props
                .binary_packed_sign_codes
                .as_deref()
                .unwrap_or("C D F C"),
        );
        if strings.get(defaults.text_standard_exponent_rep).is_err()
            || strings
                .get(defaults.text_standard_exponent_rep)
                .ok()
                .is_some_and(|s| s.is_empty())
        {
            defaults.text_standard_exponent_rep = strings.intern("E");
        }
        if schema
            .format_defaults
            .props
            .text_standard_exponent_rep
            .is_some()
        {
            defaults.text_standard_exponent_rep_defined = true;
        }
        if schema.format_defaults.props.length_kind_defined {
            defaults.length_kind_defined = true;
        }
        if defaults.text_standard_infinity_rep == StringId(0) {
            defaults.text_standard_infinity_rep = strings.intern("Inf");
        }
        if defaults.text_standard_nan_rep == StringId(0) {
            defaults.text_standard_nan_rep = strings.intern("NaN");
        }
        if defaults.text_standard_zero_rep_defined
            && strings.get(defaults.text_standard_zero_rep).ok() == Some("")
        {
            defaults.text_standard_zero_rep_defined = false;
        }
        Ok(Self {
            schema,
            nodes: Vec::new(),
            strings,
            defaults,
            tunables,
            hidden_group_expand_stack: Vec::new(),
        })
    }

    fn build(mut self, root_name: &str) -> Result<IrProgram> {
        let root = if let Some(root_element) =
            crate::schema::get_global_element(&self.schema, root_name)
        {
            self.build_root_element_node(root_name, root_element)?
        } else if let Some(type_def) = self
            .schema
            .resolve_type(&TypeName::new(root_name))
        {
            match type_def {
                TypeDef::Complex { .. } => {
                    let type_name = TypeName::new(root_name);
                    let child = self.compile_type(
                        &type_name,
                        &DfdlProps::default(),
                        Some(root_name),
                        false,
                    )?;
                    let defaults = self.defaults.clone();
                    let mut ir_props = self.merge_props_full(
                        &defaults,
                        &DfdlProps::default(),
                        &DfdlProps::default(),
                    )?;
                    if self.defaults.length_kind_defined {
                        ir_props.length_kind = self.defaults.length_kind;
                    }
                    let ir_props = finalize_element_props(
                        ValueKind::Complex,
                        ir_props,
                        &mut self.strings,
                        self.tunables,
                        Some(self.schema),
                        Some(root_name),
                    )?;
                    let name = self.strings.intern(root_name);
                    self.push(IrNode::Element {
                        name,
                        kind: ValueKind::Complex,
                        props: ir_props,
                        child: Some(child),
                    })
                }
                _ => {
                    return Err(SchemaError::UndefinedType {
                        name: root_name.to_string(),
                    }
                    .into());
                }
            }
        } else {
            return Err(SchemaError::UndefinedType {
                name: root_name.to_string(),
            }
            .into());
        };

        let program = IrProgram {
            root_element: root_name.to_string(),
            root,
            nodes: self.nodes,
            strings: self.strings,
            tunables: self.tunables,
            variables: self.schema.variables.clone(),
        };
        validate_program_sequence_bit_orders(&program)?;
        Ok(program)
    }

    fn build_root_element_node(
        &mut self,
        root_name: &str,
        root_element: &GlobalElement,
    ) -> Result<u32> {
        let root = if let Some(builtin) =
            builtin_for_element_type_name(&self.schema, &root_element.type_name)
        {
            let kind = value_kind_from_builtin(builtin);
            let defaults = self.defaults.clone();
            let mut merged =
                self.merge_props_full(&defaults, &DfdlProps::default(), &root_element.props)?;
            apply_type_name_ir_flags(&root_element.type_name, &mut merged);
            let mut props = finalize_element_props(
                kind,
                merged,
                &mut self.strings,
                self.tunables,
                Some(self.schema),
                Some(root_name),
            )?;
            apply_restriction_facets(
                &self.schema,
                &mut props,
                &SimpleBase::Builtin(builtin),
                &mut self.strings,
                &root_element.props,
            );
            validate_binary_calendar_compile(kind, &props, &self.strings)?;
            validate_implicit_text_length(kind, &props)?;
            props.xsd_type = Some(self.strings.intern(root_element.type_name.as_str()));
            let name = self.strings.intern(root_name);
            self.push(IrNode::Element {
                name,
                kind,
                props,
                child: None,
            })
        } else {
            let type_def = self
                .schema
                .resolve_type(&root_element.type_name)
                .ok_or_else(|| SchemaError::UndefinedType {
                    name: root_element.type_name.as_str().to_string(),
                })?;

            if let TypeDef::Simple { base, props: _, .. } = type_def {
                let defaults = self.defaults.clone();
                let kind = value_kind_from_simple(&self.schema, base);
                let type_props = self
                    .schema
                    .effective_simple_type_props(&root_element.type_name)
                    .unwrap_or_default();
                validate_dfdl_prop_overlap(&root_element.props, &type_props)?;
                let mut merged = self.merge_props_full(&defaults, &type_props, &root_element.props)?;
                apply_type_name_ir_flags(&root_element.type_name, &mut merged);
                validate_length_facets_for_type(&self.schema, base, kind, &merged, None)?;
                let mut ir_props = finalize_element_props(
                    kind,
                    merged,
                    &mut self.strings,
                    self.tunables,
                    Some(self.schema),
                    Some(root_name),
                )?;
                validate_length_kind_defined(
                    kind,
                    &ir_props,
                    self.schema,
                    Some(&root_element.type_name),
                )?;
                apply_restriction_facets(
                    &self.schema,
                    &mut ir_props,
                    base,
                    &mut self.strings,
                    &root_element.props,
                );
                validate_binary_calendar_compile(kind, &ir_props, &self.strings)?;
                validate_implicit_text_length(kind, &ir_props)?;
                ir_props.xsd_type = Some(self.strings.intern(root_element.type_name.as_str()));
                let name = self.strings.intern(root_name);
                self.push(IrNode::Element {
                    name,
                    kind,
                    props: ir_props,
                    child: None,
                })
            } else {
                let child = self.compile_type(
                    &root_element.type_name,
                    &root_element.props,
                    Some(root_name),
                    false,
                )?;
                let defaults = self.defaults.clone();
                let mut ir_props = self.merge_props_full(
                    &defaults,
                    &DfdlProps::default(),
                    &root_element.props,
                )?;
                if root_element.props.length_kind.is_none() && self.defaults.length_kind_defined {
                    ir_props.length_kind = self.defaults.length_kind;
                }
                let ir_props = finalize_element_props(
                    ValueKind::Complex,
                    ir_props,
                    &mut self.strings,
                    self.tunables,
                    Some(self.schema),
                    Some(root_name),
                )?;
                let name = self.strings.intern(root_name);
                self.push(IrNode::Element {
                    name,
                    kind: ValueKind::Complex,
                    props: ir_props,
                    child: Some(child),
                })
            }
        };
        Ok(root)
    }

    fn compile_type(
        &mut self,
        type_name: &TypeName,
        element_props: &DfdlProps,
        facet_diagnostic: Option<&str>,
        hidden: bool,
    ) -> Result<u32> {
        if !self.schema.types.contains_key(type_name) {
            if let Some(BuiltinType::String | BuiltinType::HexBinary) =
                BuiltinType::from_xsd(type_name.as_str())
            {
                return Err(SchemaError::UnsupportedFeature {
                    feature: alloc::format!("simple type used as root `{type_name:?}`"),
                }
                .into());
            }
        }

        if let Some(builtin) = BuiltinType::from_xsd(type_name.as_str()) {
            if !self.schema.types.contains_key(type_name) {
                return Err(SchemaError::UnsupportedFeature {
                    feature: alloc::format!(
                        "scalar root type `{}` requires an element wrapper",
                        builtin.xsd_name()
                    ),
                }
                .into());
            }
        }

        let type_def = self
            .schema
            .resolve_type(type_name)
            .ok_or_else(|| SchemaError::UndefinedType {
                name: type_name.as_str().to_string(),
            })?;

        match type_def {
            TypeDef::Simple {
                base,
                props: _,
                format_context,
                ..
            } => {
                let kind = value_kind_from_simple(&self.schema, base);
                let type_props = self
                    .schema
                    .effective_simple_type_props(type_name)
                    .unwrap_or_default();
                validate_dfdl_prop_overlap(element_props, &type_props)?;
                let mut base_ir = self.defaults.clone();
                if !format_context.length_kind_defined {
                    base_ir.length_kind = LengthKind::Implicit;
                    base_ir.length_kind_defined = false;
                }
                base_ir = overlay_dfdl_to_ir(base_ir, format_context, &mut self.strings)?;
                let mut merged = self.merge_props_full(&base_ir, &type_props, element_props)?;
                apply_type_name_ir_flags(type_name, &mut merged);
                let mut ir_props = finalize_element_props(
                    kind,
                    merged,
                    &mut self.strings,
                    self.tunables,
                    Some(self.schema),
                    None,
                )?;
                validate_length_kind_defined(
                    kind,
                    &ir_props,
                    self.schema,
                    Some(type_name),
                )?;
                apply_restriction_facets(
                    &self.schema,
                    &mut ir_props,
                    base,
                    &mut self.strings,
                    element_props,
                );
                validate_binary_calendar_compile(kind, &ir_props, &self.strings)?;
                validate_length_facets_for_type(
                    &self.schema,
                    base,
                    kind,
                    &ir_props,
                    facet_diagnostic,
                )?;
                validate_implicit_text_length(kind, &ir_props)?;
                let name = self.strings.intern("__value");
                Ok(self.push(IrNode::Element {
                    name,
                    kind,
                    props: ir_props,
                    child: None,
                }))
            }
            TypeDef::Complex {
                content,
                props,
                format_context,
                ..
            } => {
                let mut base_ir = self.defaults.clone();
                if !format_context.length_kind_defined {
                    base_ir.length_kind = LengthKind::Implicit;
                    base_ir.length_kind_defined = false;
                }
                base_ir = overlay_dfdl_to_ir(base_ir, format_context, &mut self.strings)?;
                let inherited = element_props_for_complex_content(element_props);
                let type_base = self.merge_props_full(&base_ir, props, &inherited)?;
                self.compile_complex(content, &type_base, hidden)
            }
        }
    }

    fn compile_particle(
        &mut self,
        particle: &Particle,
        inherited: &IrProps,
        prior_element_names: &[String],
    ) -> Result<u32> {
        self.compile_particle_inner(particle, inherited, prior_element_names, false)
    }

    fn compile_particle_inner(
        &mut self,
        particle: &Particle,
        inherited: &IrProps,
        prior_element_names: &[String],
        hidden: bool,
    ) -> Result<u32> {
        match particle {
            Particle::Element(element) => {
                let element_props = dfdl_props_for_element_ref(self.schema, element);
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
                apply_restriction_facets(
                    &self.schema,
                    &mut ir_props,
                    &SimpleBase::Builtin(builtin),
                    &mut self.strings,
                    &element_props,
                );
                validate_binary_calendar_compile(kind, &ir_props, &self.strings)?;
                validate_fixed_occurs_count(&ir_props)?;
                ir_props.hidden = hidden;
                validate_implicit_text_length(kind, &ir_props)?;
                ir_props.xsd_type = Some(self.strings.intern(type_name.as_str()));
                Ok(self.push(IrNode::Element {
                    name,
                    kind,
                        props: ir_props,
                        child: None,
                    }))
                } else {
                    let props = merged;
                    if let Some(ref er) = element.element_ref {
                        if let Some(TypeDef::Simple { props: type_props, .. }) =
                            self.schema.resolve_type(type_name)
                        {
                            let type_label = crate::schema::get_global_element(self.schema, er)
                                .and_then(|g| g.type_xsd_qname.clone())
                                .unwrap_or_else(|| type_name.as_str().to_string());
                            if er.contains(':') && type_label.contains(':') {
                                validate_text_number_pad_character_overlap(
                                    er.as_str(),
                                    &type_label,
                                    &element_props,
                                    type_props,
                                )?;
                            }
                        }
                    }
                    // Simple types: compile the type without element overlays so merge_ir_props
                    // can preserve type/format alignment as framing_alignment when the element
                    // overrides dfdl:alignment (Section 12 aligned_data alignment03).
                    let compile_element_props_owned =
                        match self.schema.resolve_type(type_name) {
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
                                merged_ir.terminator = child_props
                                    .terminator
                                    .or(self.defaults.terminator);
                            }
                            if merged_ir.initiator.is_none() {
                                merged_ir.initiator = child_props
                                    .initiator
                                    .or(self.defaults.initiator);
                            }
                            if element_props.alignment.is_none() {
                                merged_ir.alignment = child_props.alignment;
                                merged_ir.alignment_implicit = child_props.alignment_implicit;
                            }
                            if let Some(type_def) = self.schema.resolve_type(&element.type_name) {
                                if let TypeDef::Simple { base, .. } = type_def {
                                    validate_length_facets_for_type(
                                        &self.schema,
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
                            if let Some(type_def) = self.schema.resolve_type(&element.type_name) {
                                if let TypeDef::Simple { base, props: type_props, .. } = type_def {
                                    apply_restriction_facets(
                                        &self.schema,
                                        &mut merged,
                                        base,
                                        &mut self.strings,
                                        &element_props,
                                    );
                                    validate_binary_calendar_compile(kind, &merged, &self.strings)?;
                                    if let Some(signed) = type_props.decimal_signed {
                                        merged.decimal_signed = signed;
                                    }
                                }
                            }
                            validate_implicit_text_length(kind, &merged)?;
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
                        let ir_props =
                            self.merge_props_full(inherited, &seq.props, &gr.props)?;
                        let child_inherited =
                            particle_inherited_for_children(inherited, &seq.props, &self.defaults);
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
                        let ir_props =
                            self.merge_props_full(inherited, &ch.props, &DfdlProps::default())?;
                        let child_inherited =
                            particle_inherited_for_children(inherited, &ch.props, &self.defaults);
                        let mut branches = Vec::new();
                        for branch in &ch.branches {
                            let node = self.compile_particle_inner(
                                branch,
                                &child_inherited,
                                &[],
                                hidden,
                            )?;
                            branches.push(ChoiceBranch {
                                name: self.strings.intern(&branch_name(branch)),
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
                let ir_props = self.merge_props_full(inherited, &sequence.props, &DfdlProps::default())?;
                let child_inherited =
                    particle_inherited_for_children(inherited, &sequence.props, &self.defaults);
                let mut children = Vec::new();
                let mut prior_element_names: Vec<String> = Vec::new();
                if let Some(ref href) = sequence.props.hidden_group_ref {
                    if href.is_empty() {
                        return Err(SchemaError::InvalidProperty {
                            message: "Schema Definition Error: dfdl:hiddenGroupRef must be a valid QName"
                                .into(),
                        }
                        .into());
                    }
                    let gname = group_local_name(href);
                    if gname.is_empty() {
                        return Err(SchemaError::InvalidProperty {
                            message: "Schema Definition Error: dfdl:hiddenGroupRef must be a valid QName"
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
                            message: alloc::format!(
                                "Schema Definition Error: Model group circular definitions. Group references, or hidden group references form a loop."
                            ),
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
                            let group_inherited = particle_inherited_for_children(
                                inherited,
                                &seq.props,
                                &self.defaults,
                            );
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
                            let inline_hidden = seq
                                .props
                                .separator
                                .as_deref()
                                .map_or(true, |s| s.is_empty())
                                && seq
                                    .props
                                    .terminator
                                    .as_deref()
                                    .map_or(true, |s| s.is_empty())
                                && seq
                                    .props
                                    .initiator
                                    .as_deref()
                                    .map_or(true, |s| s.is_empty());
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
                            let ir_props = self.merge_props_full(
                                inherited,
                                &ch.props,
                                &DfdlProps::default(),
                            )?;
                            let group_inherited = particle_inherited_for_children(
                                inherited,
                                &ch.props,
                                &self.defaults,
                            );
                            let mut branches = Vec::new();
                            for branch in &ch.branches {
                                let node = self.compile_particle_inner(
                                    branch,
                                    &group_inherited,
                                    &[],
                                    true,
                                )?;
                                branches.push(ChoiceBranch {
                                    name: self.strings.intern(&branch_name(branch)),
                                    initiator: branch_initiator(branch, &mut self.strings),
                                    branch_key: branch_choice_key(branch, self.schema, &mut self.strings),
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
                let ir_props = self.merge_props_full(inherited, &choice.props, &DfdlProps::default())?;
                let child_inherited =
                    particle_inherited_for_children(inherited, &choice.props, &self.defaults);
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

    fn compile_complex(
        &mut self,
        content: &ComplexContent,
        type_base: &IrProps,
        hidden: bool,
    ) -> Result<u32> {
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
                            message: "Schema Definition Error: dfdl:hiddenGroupRef must be a valid QName"
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
                let mut ir_props = self.merge_props_full(
                    type_base,
                    &sequence.props,
                    &DfdlProps::default(),
                )?;
                if ir_props.separator.is_none()
                    && sequence.props.separator.as_deref() != Some("")
                {
                    ir_props.separator = self.defaults.separator;
                }
                let mut child_inherited =
                    particle_inherited_for_children(type_base, &sequence.props, &self.defaults);
                let mut children = Vec::new();
                let mut prior_element_names: Vec<String> = Vec::new();
                for particle in &sequence.particles {
                    validate_initiated_content_particle(&sequence.props, particle)?;
                    if let Particle::GroupRef(gr) = particle {
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
                                ir_props = self.merge_props_full(
                                    &ir_props,
                                    &seq.props,
                                    &gr.props,
                                )?;
                                child_inherited = particle_inherited_for_children(
                                    type_base,
                                    &seq.props,
                                    &self.defaults,
                                );
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
                let ir_props = self.merge_props_full(
                    type_base,
                    &choice.props,
                    &DfdlProps::default(),
                )?;
                let child_inherited =
                    particle_inherited_for_children(type_base, &choice.props, &self.defaults);
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

    fn push(&mut self, node: IrNode) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        id
    }

    fn merge_props_full(
        &mut self,
        base: &IrProps,
        type_props: &DfdlProps,
        element_props: &DfdlProps,
    ) -> Result<IrProps> {
        validate_delimiter_props(type_props, &DfdlProps::default())?;
        validate_delimiter_props(element_props, element_props)?;
        validate_text_string_pad_props(type_props)?;
        validate_text_string_pad_props(element_props)?;
        let mut ir = merge_dfdl_props(base, type_props, element_props, &mut self.strings)?;
        resolve_escape_scheme(&self.schema, type_props, element_props, &mut ir);
        self.attach_prefix_length(type_props, element_props, &mut ir, 0)?;
        Ok(ir)
    }

    fn attach_prefix_length(
        &mut self,
        type_props: &DfdlProps,
        element_props: &DfdlProps,
        ir: &mut IrProps,
        depth: usize,
    ) -> Result<()> {
        if ir.length_kind != LengthKind::Prefixed {
            return Ok(());
        }
        if depth >= 2 {
            return Err(SchemaError::InvalidProperty {
                message:
                    "Schema Definition Error. Nested dfdl:lengthKind=\"prefixed\" is not supported"
                        .into(),
            }
            .into());
        }
        let prefix_type = element_props
            .prefix_length_type
            .as_ref()
            .or(type_props.prefix_length_type.as_ref());
        let Some(type_name) = prefix_type else {
            return Err(SchemaError::InvalidProperty {
                message: "lengthKind=prefixed requires prefixLengthType".into(),
            }
            .into());
        };
        let includes_prefix = element_props
            .prefix_includes_prefix_length
            .or(type_props.prefix_includes_prefix_length)
            .unwrap_or(false);
        if includes_prefix {
            if let Some(TypeDef::Simple { props: pprops, .. }) = self.schema.resolve_type(type_name) {
                if pprops.length_units == Some(LengthUnits::Bits)
                    && ir.length_units == LengthUnits::Bytes
                {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error. ex:{} dfdl:prefixIncludesPrefixLength=\"yes\" dfdl:prefixLengthType dfdl:lengthUnits",
                            type_name.as_str()
                        ),
                    }
                    .into());
                }
            }
        }
        ir.prefix_length = Some(alloc::boxed::Box::new(
            self.resolve_prefix_length_type(type_name, depth)?,
        ));
        ir.prefix_includes_prefix_length = element_props
            .prefix_includes_prefix_length
            .or(type_props.prefix_includes_prefix_length)
            .unwrap_or(ir.prefix_includes_prefix_length);
        if ir.prefix_includes_prefix_length {
            if let Some(ref prefix) = ir.prefix_length {
                if prefix.props.length_units == LengthUnits::Bits
                    && ir.length_units == LengthUnits::Bytes
                {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error. ex:{} dfdl:prefixIncludesPrefixLength=\"yes\" dfdl:prefixLengthType dfdl:lengthUnits",
                            type_name.as_str()
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(())
    }

    fn resolve_prefix_length_type(
        &mut self,
        type_name: &TypeName,
        depth: usize,
    ) -> Result<IrPrefixLength> {
        let type_def = self.schema.resolve_type(type_name).ok_or_else(|| {
            SchemaError::UndefinedType {
                name: type_name.as_str().to_string(),
            }
        })?;
        let TypeDef::Simple { base, props, .. } = type_def else {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error. dfdl:prefixLengthType ex:{} must be simpleType",
                    type_name.as_str()
                ),
            }
            .into());
        };
        if props.has_statement_annotation {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "prefixLengthType `{}` specifies one or more statement annotations such as dfdl:assert",
                    type_name.as_str()
                ),
            }
            .into());
        }
        let (min_inclusive, max_inclusive) = match base {
            SimpleBase::Restriction {
                min_inclusive,
                max_inclusive,
                ..
            } => (*min_inclusive, *max_inclusive),
            SimpleBase::Builtin(_) | SimpleBase::Union { .. } => (None, None),
        };
        let mut prefix_props = merge_dfdl_props(
            &self.defaults.clone(),
            props,
            &DfdlProps::default(),
            &mut self.strings,
        )?;
        let kind = value_kind_from_simple(&self.schema, base);
        validate_prefix_length_type(type_name, props, &prefix_props, kind, &self.strings)?;
        self.attach_prefix_length(props, &DfdlProps::default(), &mut prefix_props, depth + 1)?;
        if kind == ValueKind::Decimal {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error. dfdl:prefixLengthType ex:{} xs:decimal subtype xs:integer",
                    type_name.as_str()
                ),
            }
            .into());
        }
        if let Some(len) = prefix_props.length {
            validate_data_length_schema(
                kind,
                len,
                prefix_props.length_units,
                prefix_props.binary_number_rep,
            )?;
        }
        Ok(IrPrefixLength {
            kind,
            props: prefix_props,
            min_inclusive,
            max_inclusive,
        })
    }
}

fn validate_prefixed_character_encoding(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    if kind != ValueKind::Complex {
        return Ok(());
    }
    if props.length_kind != LengthKind::Prefixed {
        return Ok(());
    }
    if props.length_units != LengthUnits::Characters {
        return Ok(());
    }
    let encoding = strings
        .get(props.encoding)
        .map_err(|e| SchemaError::InvalidProperty {
            message: alloc::format!("invalid encoding reference: {e}"),
        })?;
    if encoding.eq_ignore_ascii_case("utf-8") {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error. Unparsing dfdl:lengthKind='prefixed' with dfdl:lengthUnits='characters' cannot be used with variable-width encoding".into(),
        }
        .into());
    }
    Ok(())
}

fn validate_float_double_bit_length(kind: ValueKind, length: u64, units: LengthUnits) -> Result<()> {
    validate_float_double_bit_length_schema(kind, length, units).map_err(Into::into)
}

fn text_number_pattern_requires_grouping_separator(pattern: &str) -> bool {
    let mut in_quote = false;
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            if in_quote && i + 1 < chars.len() && chars[i + 1] == '\'' {
                i += 2;
                continue;
            }
            in_quote = !in_quote;
            i += 1;
            continue;
        }
        if !in_quote && chars[i] == ',' {
            return true;
        }
        i += 1;
    }
    false
}

fn text_number_pattern_bare(pattern: &str) -> String {
    let mut out = String::new();
    let mut in_quote = false;
    for c in pattern.chars() {
        if c == '\'' {
            in_quote = !in_quote;
            continue;
        }
        if !in_quote {
            out.push(c);
        }
    }
    out
}

fn text_number_pattern_has_grouping_and_exponent(pattern: &str) -> bool {
    text_number_pattern_requires_grouping_separator(pattern)
        && text_number_pattern_requires_exponent(pattern)
}

fn validate_text_number_pattern_unquoted_special(pattern: &str) -> Result<()> {
    for sub in pattern.split(';') {
        let bare = text_number_pattern_bare(sub);
        let chars: Vec<char> = bare.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            if *c == '_' {
                let pad_suffix = i > 0 && chars[i - 1] == '*';
                if !pad_suffix {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: Invalid textNumberPattern: unquoted special character in pattern `{pattern}`"
                        ),
                    }
                    .into());
                }
            }
        }
    }
    Ok(())
}

fn text_standard_distinct_entries(
    ir: &IrProps,
    strings: &StringPool,
) -> Result<Vec<(&'static str, alloc::string::String)>> {
    let mut entries = Vec::new();
    if ir.text_standard_decimal_separator_defined
        && ir.text_standard_decimal_separator_sibling.is_none()
    {
        let raw = strings.get(ir.text_standard_decimal_separator).map_err(|e| {
            SchemaError::InvalidProperty {
                message: e.to_string(),
            }
        })?;
        entries.push(("textStandardDecimalSeparator", raw.to_string()));
    }
    if ir.text_standard_grouping_separator_defined
        && ir.text_standard_grouping_separator_sibling.is_none()
    {
        if let Some(gid) = ir.text_standard_grouping_separator {
            let raw = strings.get(gid).map_err(|e| SchemaError::InvalidProperty {
                message: e.to_string(),
            })?;
            entries.push(("textStandardGroupingSeparator", raw.to_string()));
        }
    }
    if ir.text_standard_exponent_rep_defined && ir.text_standard_exponent_rep_sibling.is_none() {
        let raw = strings.get(ir.text_standard_exponent_rep).unwrap_or("");
        entries.push(("textStandardExponentRep", raw.to_string()));
    }
    if ir.text_standard_infinity_rep != StringId(0) {
        let raw = strings.get(ir.text_standard_infinity_rep).unwrap_or("");
        entries.push(("textStandardInfinityRep", raw.to_string()));
    }
    if ir.text_standard_nan_rep != StringId(0) {
        let raw = strings.get(ir.text_standard_nan_rep).unwrap_or("");
        entries.push(("textStandardNaNRep", raw.to_string()));
    }
    if ir.text_standard_zero_rep_defined {
        let raw = strings.get(ir.text_standard_zero_rep).unwrap_or("");
        entries.push(("textStandardZeroRep", raw.to_string()));
    }
    Ok(entries)
}

fn count_pad_specifiers(bare: &str) -> Result<usize> {
    let chars: Vec<char> = bare.chars().collect();
    let mut i = 0usize;
    let mut count = 0usize;
    while i < chars.len() {
        if chars[i] == '*' {
            count += 1;
            if i + 1 >= chars.len() {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: Invalid textNumberPattern: Malformed pattern \"{bare}\""
                    ),
                }
                .into());
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    Ok(count)
}

fn validate_text_number_pad_specifiers(subpattern: &str) -> Result<()> {
    let bare = text_number_pattern_bare(subpattern);
    let count = count_pad_specifiers(&bare)?;
    if count > 1 {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Invalid textNumberPattern: multiple pad specifiers".into(),
        }
        .into());
    }
    Ok(())
}

fn validate_zoned_text_number_pattern(
    kind: ValueKind,
    pattern: &str,
    check_policy: crate::schema::BinaryNumberCheckPolicy,
    decimal_signed: bool,
) -> Result<()> {
    use crate::schema::BinaryNumberCheckPolicy;
    let bare = text_number_pattern_bare(pattern);
    if bare.contains('@') {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: The '@' symbol may not be used in textNumberPattern for textNumberRep='zoned'".into(),
        }
        .into());
    }
    if bare.contains('E') {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: The 'E' symbol may not be used in textNumberPattern for textNumberRep='zoned'".into(),
        }
        .into());
    }
    let has_leading_plus = bare.starts_with('+');
    let has_trailing_plus = bare.ends_with('+');
    if has_leading_plus && has_trailing_plus {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: The textNumberPattern may either begin or end with a '+', not both.".into(),
        }
        .into());
    }
    let _ = (kind, check_policy, decimal_signed);
    Ok(())
}

fn validate_text_standard_sibling_order(
    props: &IrProps,
    prior_element_names: &[String],
    strings: &StringPool,
) -> Result<()> {
    for sib_id in [
        props.text_standard_decimal_separator_sibling,
        props.text_standard_grouping_separator_sibling,
    ]
    .into_iter()
    .flatten()
    {
        let name = strings.get(sib_id).map_err(|e| SchemaError::InvalidProperty {
            message: e.to_string(),
        })?;
        if !prior_element_names.iter().any(|n| n == name) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {name} does not exist"),
            }
            .into());
        }
    }
    Ok(())
}

fn validate_text_standard_separator_semantics(
    ir: &IrProps,
    pattern: Option<&str>,
    strings: &StringPool,
) -> Result<()> {
    if let Some(pat) = pattern {
        if text_number_pattern_requires_decimal_separator(pat)
            && ir.text_standard_decimal_separator_defined
            && ir.text_standard_decimal_separator_sibling.is_none()
        {
            let dec = strings.get(ir.text_standard_decimal_separator).map_err(|e| {
                SchemaError::InvalidProperty {
                    message: e.to_string(),
                }
            })?;
            let list = parse_text_standard_separator_list(dec);
            if list.len() > 1 {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: textStandardDecimalSeparator lists more than one character".into(),
                }
                .into());
            }
        }
        if text_number_pattern_requires_grouping_separator(pat)
            && ir.text_standard_grouping_separator.is_none()
            && ir.text_standard_grouping_separator_defined
            && ir.text_standard_grouping_separator_sibling.is_none()
        {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: textStandardGroupingSeparator length must be exactly 1".into(),
            }
            .into());
        }
    }
    Ok(())
}

fn text_number_pattern_requires_decimal_separator(pattern: &str) -> bool {
    let mut in_quote = false;
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            if in_quote && i + 1 < chars.len() && chars[i + 1] == '\'' {
                i += 2;
                continue;
            }
            in_quote = !in_quote;
            i += 1;
            continue;
        }
        if !in_quote && matches!(chars[i], '.' | 'E' | 'e' | '@') {
            return true;
        }
        i += 1;
    }
    false
}

fn text_number_pattern_requires_exponent(pattern: &str) -> bool {
    let mut in_quote = false;
    for c in pattern.chars() {
        if c == '\'' {
            in_quote = !in_quote;
            continue;
        }
        if !in_quote && matches!(c, 'E' | 'e') {
            return true;
        }
    }
    false
}

fn finalize_element_props(
    kind: ValueKind,
    mut ir: IrProps,
    strings: &mut StringPool,
    tunables: DaffodilTunables,
    schema: Option<&SchemaDocument>,
    element_name: Option<&str>,
) -> Result<IrProps> {
    use crate::schema::{NilKind, ObjectKind, TextPadKind, TextTrimKind};
    if kind == ValueKind::Complex && ir.length_kind == LengthKind::Delimited {
        ir.length_kind = LengthKind::Implicit;
    }
    if ir.object_kind == ObjectKind::Chars {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property value objectKind='chars' is not supported."
                .into(),
        }
        .into());
    }
    if ir.default_value.is_some() {
        let max = ir.occurs_max.unwrap_or(u64::MAX);
        if ir.occurs_min > 0 && max != 1 {
            let default_text = ir
                .default_value
                .and_then(|id| strings.get(id).ok())
                .unwrap_or("?");
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error. subset: XSD default='{default_text}' is not implemented."
                ),
            }
            .into());
        }
    }
    if ir.object_kind == ObjectKind::Bytes && ir.length_kind != LengthKind::Explicit {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: objectKind='bytes' must have dfdl:lengthKind='explicit'"
                .into(),
        }
        .into());
    }
    if ir.object_kind == ObjectKind::Bytes
        && ir.length_units == crate::schema::LengthUnits::Characters
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: lengthUnits 'characters' is not valid for blob data with lengthKind 'explicit'."
                .into(),
        }
        .into());
    }
    if ir.nillable && ir.nil_value.is_some() && ir.nil_kind.is_none() {
        ir.nil_kind = Some(NilKind::LiteralValue);
    }
    if ir.nillable {
        if let Some(nil_id) = ir.nil_value {
            let raw = strings.get(nil_id).map_err(|e| SchemaError::InvalidProperty {
                message: e.to_string(),
            })?;
            if let Err(msg) = crate::schema::validate_nil_value_compile(
                raw,
                ir.nil_kind,
                ir.representation,
            ) {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error: {msg}"),
                }
                .into());
            }
        }
    }
    if let Some(ref scheme) = ir.escape_scheme {
        use crate::schema::{
            EscapeKind, validate_escape_block_property, validate_escape_character_property,
        };
        match scheme.escape_kind {
            EscapeKind::EscapeBlock => {
                for raw in [
                    scheme.escape_block_start_raw.as_deref(),
                    scheme.escape_block_end_raw.as_deref(),
                ] {
                    if let Some(s) = raw {
                        if let Err(msg) = validate_escape_block_property(s) {
                            return Err(SchemaError::InvalidProperty {
                                message: alloc::format!("Schema Definition Error: {msg}"),
                            }
                            .into());
                        }
                    }
                }
            }
            EscapeKind::EscapeCharacter => {
                for raw in [
                    scheme.escape_character_raw.as_deref(),
                    scheme.escape_escape_character_raw.as_deref(),
                ] {
                    if let Some(s) = raw {
                        if let Err(msg) = validate_escape_character_property(s) {
                            return Err(SchemaError::InvalidProperty {
                                message: alloc::format!("Schema Definition Error: {msg}"),
                            }
                            .into());
                        }
                    }
                }
            }
        }
    }
    validate_binary_delimited(kind, &ir)?;
    validate_hex_binary_delimited_encoding(kind, &ir, strings)?;
    crate::length_validate::validate_binary_decimal_virtual_point_schema(kind, &ir)?;
    crate::vm::calendar_binary::validate_implicit_binary_length_schema(kind, &ir, strings)?;
    validate_trailing_skip_delimited(&ir)?;
    validate_bcd_signed_integer_type(kind, &ir)?;
    validate_packed_binary_properties_schema(kind, &ir, strings)?;
    validate_text_alignment_schema(kind, &ir, strings)?;
    validate_alignment_units_schema(&ir)?;
    validate_prefixed_character_encoding(kind, &ir, strings)?;
    validate_end_of_parent(kind, &ir)?;
    if let Some(id) = ir.text_string_pad_character {
        let raw = strings.get(id).map_err(|e| SchemaError::InvalidProperty {
            message: e.to_string(),
        })?;
        if let Err(msg) = crate::schema::validate_text_string_pad_character_merged(
            raw,
            ir.length_units == LengthUnits::Bytes,
            ir.text_string_pad_character_property_form,
        ) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
            .into());
        }
    }
    if kind == ValueKind::Boolean {
        if let Some(id) = ir.default_value {
            let raw = strings.get(id).map_err(|e| SchemaError::InvalidProperty {
                message: e.to_string(),
            })?;
            if !matches!(raw, "true" | "false" | "0" | "1") {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Invalid value constraint value".into(),
                }
                .into());
            }
        }
        if ir.representation == Representation::Text {
            if ir.text_boolean_true_rep_defined && !ir.text_boolean_false_rep_defined {
                return Err(SchemaError::InvalidProperty {
                    message:
                        "Schema Definition Error: Property textBooleanFalseRep is not defined."
                            .into(),
                }
                .into());
            }
            if ir.text_boolean_false_rep_defined && !ir.text_boolean_true_rep_defined {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property textBooleanTrueRep is not defined."
                        .into(),
                }
                .into());
            }
            for id in [ir.text_boolean_true_rep, ir.text_boolean_false_rep].into_iter().flatten() {
                let raw = strings.get(id).map_err(|e| SchemaError::InvalidProperty {
                    message: e.to_string(),
                })?;
                if let Err(msg) = crate::schema::validate_text_boolean_rep_value(raw) {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!("Schema Definition Error. {msg}"),
                    }
                    .into());
                }
            }
            if matches!(ir.length_kind, LengthKind::Explicit | LengthKind::Implicit)
                && (ir.text_pad_kind == TextPadKind::None
                    || ir.text_trim_kind == TextTrimKind::None)
                && ir.text_boolean_true_rep_defined
                && ir.text_boolean_false_rep_defined
            {
                validate_text_boolean_same_length(&ir, strings)?;
            }
        }
        if ir.representation == Representation::Binary {
            if !ir.binary_boolean_true_rep_defined {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property binaryBooleanTrueRep is not defined."
                        .into(),
                }
                .into());
            }
            if !ir.binary_boolean_false_rep_defined {
                return Err(SchemaError::InvalidProperty {
                    message:
                        "Schema Definition Error: Property binaryBooleanFalseRep is not defined."
                            .into(),
                }
                .into());
            }
        }
    }
    if ir.length_self_value_length {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Value length unknown".into(),
        }
        .into());
    }
    if matches!(ir.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        && ir.length.is_none()
        && ir.length_sibling.is_none()
        && ir.length_pattern.is_none()
        && !ir.length_expr_unparsed
        && ir.input_value_calc.is_none()
        && ir.input_value_calc_sibling.is_none()
        && ir.input_value_calc_segments.is_none()
        && ir.input_value_calc_path.is_none()
        && ir.input_value_calc_expression.is_none()
    {
        let message = schema
            .map(|s| crate::schema_validate::length_not_defined_message(s, element_name))
            .unwrap_or_else(|| "Schema Definition Error: Property length is not defined".into());
        return Err(SchemaError::InvalidProperty { message }.into());
    }
    if matches!(ir.length_kind, LengthKind::Explicit | LengthKind::Fixed) {
        if let Some(len) = ir.length {
            if ir.representation == Representation::Binary {
                validate_float_double_bit_length(kind, len, ir.length_units)?;
                if binary_length_validation_applies(kind, ir.binary_number_rep) {
                    validate_data_length_schema(kind, len, ir.length_units, ir.binary_number_rep)?;
                    validate_signed_one_bit_length_schema(kind, len, ir.length_units, &tunables)?;
                }
            }
        }
    }
    if ir.length_kind == LengthKind::Pattern {
        if let Some(id) = ir.length_pattern {
            let pat = strings.get(id).map_err(|e| SchemaError::InvalidProperty {
                message: e.to_string(),
            })?;
            validate_length_pattern(pat).map_err(|msg| SchemaError::InvalidProperty { message: msg })?;
        }
    }
    if matches!(kind, ValueKind::Float | ValueKind::Double)
        && matches!(ir.length_kind, LengthKind::Explicit)
        && ir.length_sibling.is_some()
    {
        return Err(SchemaError::InvalidProperty {
            message: "floating point binary numbers may not have runtime-specified lengths".into(),
        }
        .into());
    }
    // Binary schemas default to binary representation, but length-delimited string
    // payloads are still textual. HexBinary keeps binary bytes even with a text prefix.
    if kind == ValueKind::String
        && ir.representation == Representation::Binary
        && matches!(
            ir.length_kind,
            LengthKind::Delimited | LengthKind::Prefixed | LengthKind::Explicit
        )
    {
        ir.representation = Representation::Text;
    }
    if !matches!(ir.text_standard_base, 2 | 8 | 10 | 16) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: For property textStandardBase, value must be 2, 8, 10, or 16. Found: {}",
                ir.text_standard_base
            ),
        }
        .into());
    }
    if ir.text_standard_base != 10
        && matches!(kind, ValueKind::Float | ValueKind::Double | ValueKind::Decimal)
    {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: dfdl:textStandardBase=\"{}\" cannot be used with xs:{}",
                ir.text_standard_base,
                match kind {
                    ValueKind::Float => "float",
                    ValueKind::Double => "double",
                    ValueKind::Decimal => "decimal",
                    _ => "number",
                }
            ),
        }
        .into());
    }
    if ir.custom_text_number_pattern {
        if let Some(id) = ir.text_number_pattern {
            let pat = strings.get(id).map_err(|e| SchemaError::InvalidProperty {
                message: e.to_string(),
            })?;
            for sub in pat.split(';') {
                validate_text_number_pad_specifiers(sub)?;
            }
            validate_text_number_pattern_unquoted_special(pat)?;
            if text_number_pattern_has_grouping_and_exponent(pat) {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: Invalid textNumberPattern: Cannot have grouping separator in scientific notation {pat}"
                    ),
                }
                .into());
            }
            if ir.text_standard_decimal_separator_defined {
                let dec = strings.get(ir.text_standard_decimal_separator).unwrap_or("");
                if dec.is_empty()
                    && text_number_pattern_requires_decimal_separator(pat)
                {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Property textStandardDecimalSeparator cannot be empty".into(),
                    }
                    .into());
                }
            }
            if ir.text_number_rep == crate::schema::TextNumberRep::Zoned {
                if matches!(kind, ValueKind::Float | ValueKind::Double) {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: textNumberRep=\"zoned\" cannot be used with {}",
                            value_kind_type_name(kind)
                        ),
                    }
                    .into());
                }
                validate_zoned_text_number_pattern(
                    kind,
                    pat,
                    ir.text_number_check_policy,
                    ir.decimal_signed,
                )?;
                if let Err(e) = crate::vm::zoned_text::validate_zoned_v_in_pattern(pat) {
                    return Err(SchemaError::InvalidProperty {
                        message: match e {
                            crate::error::VmError::InvalidValue { message } => message,
                            _ => "invalid zoned textNumberPattern".into(),
                        },
                    }
                    .into());
                }
                if pat.contains('V') && kind != ValueKind::Decimal {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: The dfdl:textNumberPattern has a virtual decimal point 'V' or decimal scaling 'P' and dfdl:textNumberRep='zoned'. The type must be xs:decimal but was: {}",
                            value_kind_type_name(kind)
                        ),
                    }
                    .into());
                }
            }
            if pat.starts_with(';') && ir.text_number_rep != crate::schema::TextNumberRep::Zoned {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: The positive part of the dfdl:textNumberPattern is mandatory. The dfdl:textNumberPattern cannot begin with ';'.".into(),
                }
                .into());
            }
            if text_number_pattern_requires_decimal_separator(pat)
                && !ir.text_standard_decimal_separator_defined
            {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property textStandardDecimalSeparator is not defined".into(),
                }
                .into());
            }
            if text_number_pattern_requires_grouping_separator(pat)
                && !ir.text_standard_grouping_separator_defined
            {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property textStandardGroupingSeparator is not defined".into(),
                }
                .into());
            }
        }
    }
    let pattern_for_sep = if ir.custom_text_number_pattern {
        ir.text_number_pattern
            .and_then(|id| strings.get(id).ok())
    } else {
        None
    };
    validate_text_standard_separator_semantics(&ir, pattern_for_sep, strings)?;
    if matches!(kind, ValueKind::Float | ValueKind::Double)
        && ir.text_number_rep == crate::schema::TextNumberRep::Standard
        && ir.representation == Representation::Text
        && !ir.text_standard_exponent_rep_defined
        && ir.text_standard_exponent_rep_sibling.is_none()
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property textStandardExponentRep is not defined.".into(),
        }
        .into());
    }
    let distinct_entries = text_standard_distinct_entries(&ir, strings)?;
    if distinct_entries.len() >= 2 {
        let refs: Vec<(&str, &str)> = distinct_entries
            .iter()
            .map(|(name, raw)| (*name, raw.as_str()))
            .collect();
        validate_text_standard_distinct_values(&refs).map_err(|msg| SchemaError::InvalidProperty {
            message: alloc::format!("Schema Definition Error: {msg}"),
        })?;
    }
    if ir.text_standard_zero_rep_defined {
        let raw = strings.get(ir.text_standard_zero_rep).unwrap_or("");
        validate_text_standard_zero_rep_literal(raw).map_err(|msg| {
            SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
        })?;
    }
    validate_bit_order_byte_order(kind, &ir)?;
    Ok(ir)
}

fn binary_value_kind_requires_byte_order(kind: ValueKind) -> bool {
    matches!(
        kind,
        ValueKind::Boolean
            | ValueKind::Byte
            | ValueKind::Short
            | ValueKind::Int
            | ValueKind::Long
            | ValueKind::Integer
            | ValueKind::UnsignedInt
            | ValueKind::UnsignedShort
            | ValueKind::UnsignedByte
            | ValueKind::Float
            | ValueKind::Double
            | ValueKind::Decimal
            | ValueKind::DateTime
            | ValueKind::Time
    )
}

fn validate_bit_order_byte_order(kind: ValueKind, props: &IrProps) -> Result<()> {
    use crate::schema::{BitOrder, ByteOrder, Representation};
    if props.representation != Representation::Binary {
        return Ok(());
    }
    if !binary_value_kind_requires_byte_order(kind) {
        return Ok(());
    }
    if !props.byte_order_defined {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property byteOrder is not defined.".into(),
        }
        .into());
    }
    if props.byte_order == ByteOrder::BigEndian
        && props.bit_order == BitOrder::LeastSignificantBitFirst
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: bitOrder 'leastSignificantBitFirst' cannot be used with byteOrder 'bigEndian'.".into(),
        }
        .into());
    }
    Ok(())
}

fn validate_program_sequence_bit_orders(program: &IrProgram) -> Result<()> {
    use crate::ir::{IrNode, ValueKind};
    use crate::schema::{BitOrder, LengthKind, LengthUnits, Representation};
    for node in &program.nodes {
        let IrNode::Sequence { children, .. } = node else {
            continue;
        };
        let mut cumulative = 0usize;
        let mut prev: Option<BitOrder> = None;
        for &child_id in children {
            let Ok(IrNode::Element { props, kind, .. }) = program.node(child_id) else {
                continue;
            };
            if *kind == ValueKind::Complex
                || props.representation != Representation::Binary
                || props.length_units != LengthUnits::Bits
            {
                continue;
            }
            if !matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed) {
                continue;
            }
            let Some(len) = props.length else {
                continue;
            };
            let order = props.bit_order;
            if let Some(p) = prev {
                if p != order && cumulative % 8 != 0 {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Changing bitOrder requires a byte boundary.".into(),
                    }
                    .into());
                }
            }
            cumulative = cumulative.saturating_add(len as usize);
            prev = Some(order);
        }
    }
    Ok(())
}

fn calendar_first_day_of_week_iso(raw: &str) -> u32 {
    match raw.trim().to_ascii_lowercase().as_str() {
        "sunday" => 7,
        "monday" => 1,
        "tuesday" => 2,
        "wednesday" => 3,
        "thursday" => 4,
        "friday" => 5,
        "saturday" => 6,
        _ => 1,
    }
}

fn validate_binary_calendar_compile(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    crate::vm::calendar_binary::validate_calendar_schema(kind, props, strings).map_err(Into::into)
}

fn apply_unsigned_long_flag(type_name: &TypeName, props: &mut IrProps) {
    let local = type_name.as_str().rsplit(':').next().unwrap_or(type_name.as_str());
    if matches!(local, "unsignedLong") {
        props.unsigned_integer = true;
    }
}

fn apply_integer_type_flags(type_name: &TypeName, props: &mut IrProps) {
    let local = type_name.as_str().rsplit(':').next().unwrap_or(type_name.as_str());
    if matches!(local, "nonNegativeInteger") {
        props.non_negative_integer = true;
    }
}

fn apply_calendar_type_flags(type_name: &TypeName, props: &mut IrProps) {
    let local = type_name.as_str().rsplit(':').next().unwrap_or(type_name.as_str());
    if matches!(local, "date") {
        props.calendar_date_only = true;
    }
}

fn apply_type_name_ir_flags(type_name: &TypeName, props: &mut IrProps) {
    apply_unsigned_long_flag(type_name, props);
    apply_integer_type_flags(type_name, props);
    apply_calendar_type_flags(type_name, props);
}

fn validate_delimiter_at_compile(prop: &str, raw: &str, props: &DfdlProps) -> Result<()> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        if let Err(msg) = crate::schema::validate_runtime_delimiter_expression(prop, trimmed) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error. {msg}"),
            }
            .into());
        }
        if prop == "terminator"
            && props.length_kind == Some(LengthKind::Delimited)
            && crate::schema::delimited_terminator_expression_uses_es_literal(trimmed)
        {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error. dfdl:terminator — ES entity cannot appear on its own when dfdl:lengthKind=\"delimited\"".into(),
            }
            .into());
        }
        return Ok(());
    }
    let skip_entity_check = prop == "initiator" && props.initiator_percent_escaped;
    if !skip_entity_check {
        if let Err(msg) = crate::schema::validate_delimiter_property_value(raw) {
            let msg = if raw.trim() == "%" && prop == "terminator" {
                alloc::format!("{msg}\n%%")
            } else {
                msg
            };
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error. {msg}"),
            }
            .into());
        }
    }
    if let Err(msg) = crate::schema::validate_delimiter_es_restriction(prop, raw) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!("Schema Definition Error. {msg}"),
        }
        .into());
    }
    Ok(())
}

fn validate_delimiter_props(props: &DfdlProps, escape_ctx: &DfdlProps) -> Result<()> {
    for (prop, s) in [
        ("initiator", props.initiator.as_deref()),
        ("separator", props.separator.as_deref()),
        ("terminator", props.terminator.as_deref()),
    ] {
        if let Some(v) = s {
            if !v.is_empty() {
                validate_delimiter_at_compile(prop, v, escape_ctx)?;
            }
        }
    }
    Ok(())
}

fn validate_text_boolean_same_length(props: &IrProps, strings: &StringPool) -> Result<()> {
    let (Some(true_id), Some(false_id)) = (props.text_boolean_true_rep, props.text_boolean_false_rep)
    else {
        return Ok(());
    };
    let true_raw = strings.get(true_id).map_err(|e| SchemaError::InvalidProperty {
        message: e.to_string(),
    })?;
    let false_raw = strings.get(false_id).map_err(|e| SchemaError::InvalidProperty {
        message: e.to_string(),
    })?;
    let true_tokens = crate::schema::boolean_reps::tokenize_text_boolean_rep_list(true_raw);
    let false_tokens = crate::schema::boolean_reps::tokenize_text_boolean_rep_list(false_raw);
    let true_len = true_tokens
        .first()
        .and_then(|t| {
            crate::schema::boolean_reps::resolve_text_boolean_rep_token(t, None, None).ok()
        })
        .map(|s| s.chars().count())
        .unwrap_or(0);
    let false_len = false_tokens
        .first()
        .and_then(|t| {
            crate::schema::boolean_reps::resolve_text_boolean_rep_token(t, None, None).ok()
        })
        .map(|s| s.chars().count())
        .unwrap_or(0);
    if true_len != false_len
        || true_tokens.iter().any(|t| {
            crate::schema::boolean_reps::resolve_text_boolean_rep_token(t, None, None)
                .map(|s| s.chars().count())
                .unwrap_or(0)
                != true_len
        })
        || false_tokens.iter().any(|t| {
            crate::schema::boolean_reps::resolve_text_boolean_rep_token(t, None, None)
                .map(|s| s.chars().count())
                .unwrap_or(0)
                != false_len
        })
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: dfdl:textBooleanTrueRep and dfdl:textBooleanFalseRep must have the same length".into(),
        }
        .into());
    }
    Ok(())
}

fn validate_text_string_pad_props(props: &DfdlProps) -> Result<()> {
    if let Some(raw) = props.text_string_pad_character.as_deref() {
        if let Err(msg) = crate::schema::validate_text_string_pad_character_compile(raw) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
            .into());
        }
    }
    Ok(())
}

fn validate_initiated_content_particle(sequence_props: &DfdlProps, particle: &Particle) -> Result<()> {
    if !sequence_props.initiated_content.unwrap_or(false) {
        return Ok(());
    }
    let Particle::Element(element) = particle else {
        return Ok(());
    };
    let initiator = element.props.initiator.as_deref().unwrap_or("");
    if initiator.is_empty() {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error. initiatedContent yes requires initiator not defined"
                .into(),
        }
        .into());
    }
    let trimmed = initiator.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        if crate::schema::runtime_delimiter_expression_may_be_zero_length(trimmed) {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error. initiatedContent yes requires initiator zero length"
                    .into(),
            }
            .into());
        }
    } else if crate::schema::is_zero_length_delimiter(initiator) {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error. initiatedContent yes requires initiator zero length"
                .into(),
        }
        .into());
    }
    Ok(())
}

fn validate_bcd_signed_integer_type(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.representation != Representation::Binary {
        return Ok(());
    }
    if props.binary_number_rep != crate::schema::BinaryNumberRep::Bcd {
        return Ok(());
    }
    let type_name = match kind {
        ValueKind::Byte => "byte",
        ValueKind::Short => "short",
        ValueKind::Int => "int",
        ValueKind::Long => "long",
        _ => return Ok(()),
    };
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error. {type_name} is not an allowed type for bcd binary values"
        ),
    }
    .into())
}

fn validate_trailing_skip_delimited(props: &IrProps) -> Result<()> {
    if props.trailing_skip == 0 || props.length_kind != LengthKind::Delimited {
        return Ok(());
    }
    let has_terminator = props
        .terminator
        .is_some();
    if has_terminator {
        return Ok(());
    }
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error. Property terminator must be defined when trailingSkip > 0 and lengthKind='delimited'"
        ),
    }
    .into())
}

fn validate_hex_binary_delimited_encoding(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    if kind != ValueKind::HexBinary {
        return Ok(());
    }
    if !matches!(props.length_kind, LengthKind::Delimited | LengthKind::Pattern) {
        return Ok(());
    }
    let enc = strings.get(props.encoding).unwrap_or("US-ASCII");
    if !enc.eq_ignore_ascii_case("ISO-8859-1") {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: xs:hexBinary with dfdl:lengthKind=\"{}\" must have dfdl:encoding=\"ISO-8859-1\", but was \"{enc}\"",
                length_kind_label(props.length_kind)
            ),
        }
        .into());
    }
    Ok(())
}

fn validate_binary_delimited(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.representation == Representation::Binary
        && matches!(
            props.length_kind,
            LengthKind::Explicit | LengthKind::Fixed | LengthKind::Prefixed
        )
    {
        return Ok(());
    }
    if props.representation == Representation::Binary
        && props.length_kind == LengthKind::Delimited
        && props.length.is_some()
    {
        return Ok(());
    }
    if props.representation == Representation::Binary
        && props.length_kind == LengthKind::Delimited
    {
        if matches!(kind, ValueKind::String | ValueKind::HexBinary | ValueKind::Complex) {
            return Ok(());
        }
        if matches!(kind, ValueKind::DateTime | ValueKind::Time) {
            if matches!(
                props.binary_calendar_rep,
                crate::schema::BinaryNumberRep::BinarySeconds
                    | crate::schema::BinaryNumberRep::BinaryMilliseconds
            ) {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: lengthKind='delimited' only supported for packed binary formats.".into(),
                }
                .into());
            }
            if matches!(
                props.binary_calendar_rep,
                crate::schema::BinaryNumberRep::PackedBcd
                    | crate::schema::BinaryNumberRep::Bcd
                    | crate::schema::BinaryNumberRep::Ibm4690Packed
            ) {
                return Ok(());
            }
        }
        if matches!(
            props.binary_number_rep,
            crate::schema::BinaryNumberRep::PackedBcd
                | crate::schema::BinaryNumberRep::Bcd
                | crate::schema::BinaryNumberRep::Ibm4690Packed
        ) {
            return Ok(());
        }
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: lengthKind='delimited' only supported for packed binary formats.".into(),
        }
        .into());
    }
    Ok(())
}

fn validate_model_group_occurs(group: &str, props: &DfdlProps) -> Result<()> {
    let min = props.occurs_min.unwrap_or(1);
    if min != 1 || props.max_occurs_specified {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. xs:minOccurs and xs:maxOccurs cannot appear in xs:{group} model group"
            ),
        }
        .into());
    }
    Ok(())
}

fn validate_text_number_pad_character_overlap(
    element_label: &str,
    type_label: &str,
    element: &DfdlProps,
    type_props: &DfdlProps,
) -> Result<()> {
    let el = element
        .text_number_pad_character
        .as_ref()
        .is_some_and(|s| !s.is_empty());
    let ty = type_props
        .text_number_pad_character
        .as_ref()
        .is_some_and(|s| !s.is_empty());
    if el && ty {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. Overlapping properties: textNumberPadCharacter overlaps between {element_label} and {type_label}. Overlap is not allowed."
            ),
        }
        .into());
    }
    Ok(())
}

fn validate_dfdl_prop_overlap(element: &DfdlProps, type_props: &DfdlProps) -> Result<()> {
    let overlaps = |a: bool, b: bool| a && b;
    let check = |prop: &str, a: bool, b: bool| -> Result<()> {
        if overlaps(a, b) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error. Property overlap on `{prop}` between element and simple type"
                ),
            }
            .into());
        }
        Ok(())
    };
    check(
        "byteOrder",
        element.byte_order.is_some(),
        type_props.byte_order.is_some(),
    )?;
    check(
        "bitOrder",
        element.bit_order.is_some(),
        type_props.bit_order.is_some(),
    )?;
    check(
        "representation",
        element.representation.is_some(),
        type_props.representation.is_some(),
    )?;
    check(
        "lengthKind",
        element.length_kind.is_some(),
        type_props.length_kind.is_some(),
    )?;
    check(
        "length",
        element.length.is_some(),
        type_props.length.is_some(),
    )?;
    check(
        "encoding",
        element.encoding.is_some(),
        type_props.encoding.is_some(),
    )?;
    check(
        "alignment",
        element.alignment.is_some(),
        type_props.alignment.is_some(),
    )?;
    check(
        "initiator",
        element.initiator.as_ref().is_some_and(|s| !s.is_empty()),
        type_props.initiator.as_ref().is_some_and(|s| !s.is_empty()),
    )?;
    check(
        "terminator",
        element.terminator.as_ref().is_some_and(|s| !s.is_empty()),
        type_props.terminator.as_ref().is_some_and(|s| !s.is_empty()),
    )?;
    check(
        "separator",
        element.separator.as_ref().is_some_and(|s| !s.is_empty()),
        type_props.separator.as_ref().is_some_and(|s| !s.is_empty()),
    )?;
    Ok(())
}

fn validate_length_kind_defined(
    kind: ValueKind,
    ir: &IrProps,
    schema: &SchemaDocument,
    type_name: Option<&TypeName>,
) -> Result<()> {
    if matches!(kind, ValueKind::Complex) {
        return Ok(());
    }
    if ir.length_kind_defined {
        return Ok(());
    }
    let Some(type_name) = type_name else {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property lengthKind is not defined.".into(),
        }
        .into());
    };
    let mut labels = schema.length_kind_missing_diag_labels(type_name);
    labels.sort();
    labels.dedup();
    let nd_list = labels
        .into_iter()
        .map(|l| alloc::format!("\n{l}"))
        .collect::<alloc::string::String>();
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: Property lengthKind is not defined.\nNon-default properties were combined from these locations:{nd_list}\n"
        ),
    }
    .into())
}

fn validate_implicit_text_length(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.length_kind != LengthKind::Implicit {
        return Ok(());
    }
    if props.representation != Representation::Text {
        return Ok(());
    }
    if props.input_value_calc.is_some()
        || props.input_value_calc_sibling.is_some()
        || props.input_value_calc_segments.is_some()
        || props.input_value_calc_path.is_some()
        || props.input_value_calc_expression.is_some()
    {
        return Ok(());
    }
    if matches!(kind, ValueKind::String | ValueKind::HexBinary | ValueKind::Complex) {
        return Ok(());
    }
    if kind == ValueKind::Time {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. type {} lengthKind='implicit' representation='text' is not allowed",
                value_kind_type_name(kind)
            ),
        }
        .into());
    }
    let type_label = if kind == ValueKind::Integer && props.non_negative_integer {
        "nonNegativeInteger"
    } else {
        implicit_text_length_type_label(kind)
    };
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error. type {type_label} lengthKind='implicit' representation='text' is not allowed"
        ),
    }
    .into())
}

fn implicit_text_length_type_label(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Integer => "Integer",
        ValueKind::Decimal => "Decimal",
        ValueKind::Double => "Double",
        ValueKind::Float => "Float",
        ValueKind::Boolean => "Boolean",
        ValueKind::Byte => "Byte",
        ValueKind::Short => "Short",
        ValueKind::Int => "Int",
        ValueKind::Long => "Long",
        ValueKind::UnsignedByte => "UnsignedByte",
        ValueKind::UnsignedShort => "UnsignedShort",
        ValueKind::UnsignedInt => "UnsignedInt",
        ValueKind::DateTime => "DateTime",
        _ => value_kind_type_name(kind),
    }
}

fn validate_end_of_parent(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.length_kind != LengthKind::EndOfParent {
        return Ok(());
    }
    if kind == ValueKind::Complex {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error. not implemented endOfParent complex type".into(),
        }
        .into());
    }
    Err(SchemaError::InvalidProperty {
        message: "Schema Definition Error. not implemented endOfParent simple type".into(),
    }
    .into())
}

fn value_kind_type_name(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Boolean => "xs:boolean",
        ValueKind::Byte => "xs:byte",
        ValueKind::Short => "xs:short",
        ValueKind::Int => "xs:int",
        ValueKind::Integer => "xs:integer",
        ValueKind::Long => "xs:long",
        ValueKind::UnsignedByte => "xs:unsignedByte",
        ValueKind::UnsignedShort => "xs:unsignedShort",
        ValueKind::UnsignedInt => "xs:unsignedInt",
        ValueKind::Float => "xs:float",
        ValueKind::Double => "xs:double",
        ValueKind::Decimal => "xs:decimal",
        ValueKind::DateTime => "xs:dateTime",
        ValueKind::Time => "xs:time",
        ValueKind::String => "xs:string",
        ValueKind::HexBinary => "xs:hexBinary",
        ValueKind::Complex => "complex",
    }
}

fn validate_prefix_length_type(
    type_name: &TypeName,
    raw_props: &DfdlProps,
    prefix_props: &IrProps,
    kind: ValueKind,
    strings: &StringPool,
) -> Result<()> {
    let qname = alloc::format!("ex:{}", type_name.as_str());
    let prefix_label = alloc::format!("dfdl:prefixLengthType {qname}");

    match prefix_props.length_kind {
        LengthKind::Explicit | LengthKind::Fixed | LengthKind::Implicit | LengthKind::Prefixed => {}
        other => {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error. {qname} {prefix_label} lengthKind {}",
                    length_kind_label(other)
                ),
            }
            .into());
        }
    }

    if matches!(
        prefix_props.length_kind,
        LengthKind::Explicit | LengthKind::Fixed
    ) && prefix_props.length.is_none()
    {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} expression"
            ),
        }
        .into());
    }

    if raw_props.output_value_calc.is_some() {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:outputValueCalc"
            ),
        }
        .into());
    }
    if raw_props.initiator.as_ref().is_some_and(|s| !s.is_empty()) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:initiator"
            ),
        }
        .into());
    }
    if raw_props.terminator.as_ref().is_some_and(|s| !s.is_empty()) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:terminator"
            ),
        }
        .into());
    }
    if raw_props.alignment.is_some() && raw_props.alignment != Some(0) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:alignment"
            ),
        }
        .into());
    }
    if raw_props.leading_skip.is_some() && raw_props.leading_skip != Some(0) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:leadingSkip"
            ),
        }
        .into());
    }
    if raw_props.trailing_skip.is_some() && raw_props.trailing_skip != Some(0) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:trailingSkip"
            ),
        }
        .into());
    }

    let encoding = strings
        .get(prefix_props.encoding)
        .unwrap_or("utf-8");
    if prefix_props.length_units == LengthUnits::Bits
        && encoding.eq_ignore_ascii_case("US-ASCII")
        && prefix_props.representation == Representation::Text
        && !prefix_props.alignment_implicit
    {
        let enc_align = crate::length_validate::implicit_text_encoding_alignment_bits_for_kind(
            kind,
            encoding,
        );
        let bit_align = if prefix_props.alignment == 0 {
            1
        } else {
            prefix_props.alignment
        };
        if enc_align != 0 && bit_align % enc_align != 0 {
            let type_name = value_kind_type_name(kind);
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: The given alignment ({bit_align} bits) must be a multiple of the encoding specified alignment ({enc_align} bits) for {type_name} when representation='text'. Encoding: {encoding}"
                ),
            }
            .into());
        }
    }

    Ok(())
}

fn length_kind_label(kind: LengthKind) -> &'static str {
    match kind {
        LengthKind::Implicit => "implicit",
        LengthKind::Explicit => "explicit",
        LengthKind::Fixed => "fixed",
        LengthKind::Delimited => "delimited",
        LengthKind::Prefixed => "prefixed",
        LengthKind::Pattern => "pattern",
        LengthKind::EndOfParent => "endOfParent",
    }
}

fn group_local_name(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

fn resolve_escape_scheme(
    schema: &SchemaDocument,
    type_props: &DfdlProps,
    element_props: &DfdlProps,
    ir: &mut IrProps,
) {
    let ref_name = element_props
        .escape_scheme_ref
        .as_ref()
        .or(type_props.escape_scheme_ref.as_ref());
    let Some(ref_name) = ref_name else {
        return;
    };
    if ref_name.is_empty() {
        ir.escape_scheme = None;
        return;
    }
    if ref_name.contains('|') {
        if let Some(scheme) = schema.named_escape_schemes.get(ref_name) {
            ir.escape_scheme = Some(scheme.clone());
            return;
        }
    }
    if let Some(scheme) = crate::schema::lookup_named_escape_scheme_in_document(schema, ref_name) {
        ir.escape_scheme = Some(scheme);
    }
}

fn validate_fixed_occurs_count(props: &IrProps) -> Result<()> {
    if props.occurs_count_kind != OccursCountKind::Fixed {
        return Ok(());
    }
    let Some(max) = props.occurs_max else {
        return Err(SchemaError::InvalidProperty {
            message:
                "Schema Definition Error: occursCountKind='fixed' not allowed with unbounded maxOccurs"
                    .into(),
        }
        .into());
    };
    if props.occurs_min != max {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: occursCountKind='fixed' requires minOccurs and maxOccurs to be equal ({} != {})",
                props.occurs_min, max
            ),
        }
        .into());
    }
    Ok(())
}

fn validate_implicit_unbounded_in_sequence(
    particles: &[Particle],
    has_hidden_prefix: bool,
) -> Result<()> {
    let mut saw_implicit_unbounded = false;
    for (idx, particle) in particles.iter().enumerate() {
        if let Particle::Element(el) = particle {
            let implicit = el.props.occurs_count_kind == Some(OccursCountKind::Implicit);
            let unbounded = el.props.max_occurs_specified && el.props.occurs_max.is_none();
            if implicit && unbounded {
                if saw_implicit_unbounded || idx + 1 < particles.len() || has_hidden_prefix {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: occursCountKind='implicit' with unbounded maxOccurs only allowed for last element of a positional sequence".into(),
                    }
                    .into());
                }
                saw_implicit_unbounded = true;
            }
        }
    }
    Ok(())
}

fn branch_name(particle: &Particle) -> String {
    match particle {
        Particle::Element(e) => e.name.clone(),
        Particle::Sequence(_) => "sequence".to_string(),
        Particle::Choice(_) => "choice".to_string(),
        Particle::GroupRef(gr) => group_local_name(&gr.name).to_string(),
    }
}

fn branch_initiator(particle: &Particle, strings: &mut StringPool) -> Option<StringId> {
    let raw = match particle {
        Particle::Element(e) => e.props.initiator.as_deref(),
        Particle::Sequence(s) => s.props.initiator.as_deref(),
        Particle::Choice(c) => c.props.initiator.as_deref(),
        Particle::GroupRef(_) => None,
    };
    raw.map(|s| strings.intern(s))
}

fn branch_choice_key(
    particle: &Particle,
    schema: &SchemaDocument,
    strings: &mut StringPool,
) -> Option<StringId> {
    let raw = match particle {
        Particle::Element(e) => e.props.choice_branch_key.as_deref(),
        Particle::Sequence(s) => s.props.choice_branch_key.as_deref(),
        Particle::Choice(c) => c.props.choice_branch_key.as_deref(),
        Particle::GroupRef(gr) => gr
            .props
            .choice_branch_key
            .as_deref()
            .or_else(|| {
                schema
                    .groups
                    .get(group_local_name(&gr.name))
                    .and_then(|group| match group {
                        crate::schema::GroupDecl::Sequence(s) => {
                            s.props.choice_branch_key.as_deref()
                        }
                        crate::schema::GroupDecl::Choice(c) => {
                            c.props.choice_branch_key.as_deref()
                        }
                    })
            }),
    };
    raw.map(|s| strings.intern(s))
}

fn value_kind_from_simple(schema: &SchemaDocument, base: &SimpleBase) -> ValueKind {
    schema
        .builtin_for_simple_base(base)
        .map(value_kind_from_builtin)
        .unwrap_or(ValueKind::String)
}

/// XSD built-in names like `int` must not shadow a user simpleType with the same local name (DFDL-563).
fn builtin_for_element_type_name(
    schema: &SchemaDocument,
    type_name: &TypeName,
) -> Option<BuiltinType> {
    if schema.types.contains_key(type_name) {
        return None;
    }
    BuiltinType::from_xsd(type_name.as_str())
}

fn apply_restriction_facets(
    schema: &SchemaDocument,
    props: &mut IrProps,
    base: &SimpleBase,
    strings: &mut StringPool,
    element_props: &DfdlProps,
) {
    let eff = schema.effective_facets(base);
    crate::schema::apply_effective_facets_to_ir(&eff, props, strings);
    if let Some(builtin) = schema.builtin_for_simple_base(base) {
        if builtin == crate::schema::BuiltinType::NonNegativeInteger {
            props.non_negative_integer = true;
        }
    }
    if schema.simple_base_is_xs_date(base) {
        props.calendar_date_only = true;
    }
    if element_props.facet_check_constraints {
        props.facet_check_constraints = true;
    }
    if let Some(msg) = &element_props.assert_message {
        props.facet_assert_message = Some(strings.intern(msg.clone()));
    }
    if let Some(segs) = &element_props.assert_message_segments {
        props.facet_assert_message_segments =
            Some(intern_input_value_calc_segments(segs, strings));
    }
    if let Some(n) = element_props.assert_int_eq {
        props.assert_int_eq = Some(n);
    }
    if let Some(n) = element_props.assert_eq_occurs_index_addend {
        props.assert_eq_occurs_index_addend = Some(n);
    }
    if let SimpleBase::Restriction {
        base,
        enumerations,
        ..
    } = base
    {
        if !enumerations.is_empty() {
            props.facet_assert_daffodil_prefix = true;
        }
        match base {
            crate::schema::RestrictionBase::Named(_) => {
                props.facet_assert_daffodil_prefix = true;
            }
            crate::schema::RestrictionBase::Builtin(b) if *b != crate::schema::BuiltinType::String => {
                props.facet_assert_daffodil_prefix = true;
            }
            _ => {}
        }
    }
}

fn value_kind_from_builtin(builtin: BuiltinType) -> ValueKind {
    match builtin {
        BuiltinType::Boolean => ValueKind::Boolean,
        BuiltinType::Int => ValueKind::Int,
        BuiltinType::Integer | BuiltinType::NonNegativeInteger => ValueKind::Integer,
        BuiltinType::Long => ValueKind::Long,
        BuiltinType::Short => ValueKind::Short,
        BuiltinType::Byte => ValueKind::Byte,
        BuiltinType::UnsignedInt => ValueKind::UnsignedInt,
        BuiltinType::UnsignedShort => ValueKind::UnsignedShort,
        BuiltinType::UnsignedByte => ValueKind::UnsignedByte,
        BuiltinType::Float => ValueKind::Float,
        BuiltinType::Double => ValueKind::Double,
        BuiltinType::Decimal => ValueKind::Decimal,
        BuiltinType::DateTime => ValueKind::DateTime,
        BuiltinType::Time => ValueKind::Time,
        BuiltinType::String => ValueKind::String,
        BuiltinType::HexBinary => ValueKind::HexBinary,
    }
}

/// Inherited props for particles inside a sequence/choice group.
///
/// `lengthKind` on a complex element applies to that element's span in its parent,
/// not to descendants — reset to schema format defaults unless the group sets it.
/// Properties on an element wrapper that apply to its complex-type children.
fn element_props_for_complex_content(element_props: &DfdlProps) -> DfdlProps {
    DfdlProps {
        representation: element_props.representation,
        byte_order: element_props.byte_order,
        bit_order: element_props.bit_order,
        encoding: element_props.encoding.clone(),
        encoding_error_policy: element_props.encoding_error_policy,
        separator_suppression_policy: element_props.separator_suppression_policy,
        ignore_case: element_props.ignore_case,
        ..DfdlProps::default()
    }
}

fn particle_inherited_for_children(
    parent_inherited: &IrProps,
    group_props: &DfdlProps,
    defaults: &IrProps,
) -> IrProps {
    // Group-level delimiter and ignoreCase properties apply to the group node, not descendants.
    let mut inherited = parent_inherited.clone();
    if group_props.length_kind.is_none()
        && defaults.length_kind_defined
        && !inherited.length_kind_defined
    {
        inherited.length_kind = defaults.length_kind;
        inherited.length_kind_defined = true;
    }
    // Element occurrence limits apply to the particle, not descendants.
    if group_props.occurs_min.is_none() {
        inherited.occurs_min = defaults.occurs_min;
    }
    if !group_props.max_occurs_specified {
        inherited.occurs_max = defaults.occurs_max;
    }
    // Initiator/terminator/separator on a group apply to the group node itself, not its children.
    inherited.initiator = None;
    inherited.terminator = None;
    inherited.separator = None;
    // Preserve ancestor `ignoreCase` for descendant parsing; explicit group `ignoreCase`
    // still overlays via merge_props_full on the group node itself.
    inherited
}

fn merge_dfdl_props(
    base: &IrProps,
    type_props: &DfdlProps,
    element_props: &DfdlProps,
    strings: &mut StringPool,
) -> Result<IrProps> {
    let mut out = base.clone();
    out = overlay_dfdl_to_ir(out, type_props, strings)?;
    out = overlay_dfdl_to_ir(out, element_props, strings)?;
    if type_props.text_number_pattern.is_some() || element_props.text_number_pattern.is_some() {
        out.custom_text_number_pattern = true;
    }
    Ok(out)
}

fn overlay_dfdl_to_ir(
    mut base: IrProps,
    props: &DfdlProps,
    strings: &mut StringPool,
) -> Result<IrProps> {
    if let Some(v) = props.representation {
        base.representation = v;
    }
    if let Some(v) = props.byte_order {
        base.byte_order = v;
        base.byte_order_defined = true;
    }
    if let Some(v) = props.bit_order {
        base.bit_order = v;
        base.bit_order_defined = true;
    }
    if let Some(v) = props.length_kind {
        base.length_kind = v;
        base.length_kind_defined = true;
    } else if props.length_kind_defined {
        base.length_kind_defined = true;
    }
    if props.length.is_some() {
        base.length = props.length;
    }
    if props.length_sibling.is_some() {
        base.length_sibling = props
            .length_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.length_sibling_cast_long {
        base.length_sibling_cast_long = true;
    }
    if props.length_expr_unparsed {
        base.length_expr_unparsed = true;
    }
    if props.length_self_string_max_cap.is_some() {
        base.length_self_string_max_cap = props.length_self_string_max_cap;
    }
    if props.length_self_value_length {
        base.length_self_value_length = true;
    }
    if let Some(v) = props.length_units {
        base.length_units = v;
    }
    if props.encoding.is_some() {
        base.encoding = strings.intern(props.encoding.as_deref().unwrap_or("UTF-8"));
    }
    if let Some(v) = props.encoding_error_policy {
        base.encoding_error_policy = v;
    }
    if let Some(v) = props.nillable {
        base.nillable = v;
    }
    if let Some(v) = props.nil_kind {
        base.nil_kind = Some(v);
    }
    if props.nil_value.is_some() {
        base.nil_value = props
            .nil_value
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.separator_suppression_policy {
        base.separator_suppression_policy = Some(v);
    }
    if let Some(v) = props.empty_element_parse_policy {
        base.empty_element_parse_policy = v;
    }
    if let Some(v) = props.occurs_count_kind {
        base.occurs_count_kind = v;
    }
    if let Some(v) = props.ignore_case {
        base.ignore_case = v;
    }
    if let Some(v) = props.initiated_content {
        base.initiated_content = v;
    }
    if let Some(v) = props.text_trim_kind {
        base.text_trim_kind = v;
    }
    if let Some(v) = props.text_pad_kind {
        base.text_pad_kind = v;
    }
    if let Some(v) = props.truncate_specified_length_string {
        base.truncate_specified_length_string = v;
    }
    if props.text_number_pad_character.is_some() {
        let raw = props.text_number_pad_character.as_deref().unwrap();
        if let Err(msg) = crate::schema::validate_text_number_pad_character_merged(
            raw,
            props.text_number_pad_character_property_form,
        ) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
            .into());
        }
        let expanded = crate::schema::expand_entities_str(raw);
        base.text_number_pad_character = Some(strings.intern(expanded));
        base.text_number_pad_character_property_form =
            props.text_number_pad_character_property_form;
    }
    if props.text_string_pad_character.is_some() {
        base.text_string_pad_character = props
            .text_string_pad_character
            .as_ref()
            .map(|s| strings.intern(s.clone()));
        base.text_string_pad_character_property_form =
            props.text_string_pad_character_property_form;
    }
    if props.text_calendar_pad_character.is_some() {
        base.text_calendar_pad_character = props
            .text_calendar_pad_character
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.text_calendar_justification {
        base.text_calendar_justification = Some(v);
    }
    if let Some(v) = props.binary_number_rep {
        base.binary_number_rep = v;
    }
    if props.binary_packed_sign_codes.is_some() {
        base.binary_packed_sign_codes =
            strings.intern(props.binary_packed_sign_codes.as_deref().unwrap_or("C D F C"));
    }
    if props.binary_packed_sign_codes_defined {
        base.binary_packed_sign_codes_defined = true;
    }
    if let Some(v) = props.binary_number_check_policy {
        base.binary_number_check_policy = v;
    }
    if let Some(v) = props.binary_calendar_rep {
        base.binary_calendar_rep = v;
    }
    if props.binary_calendar_epoch.is_some() {
        base.binary_calendar_epoch = props
            .binary_calendar_epoch
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.binary_float_rep {
        base.binary_float_rep = v;
    }
    if props.binary_decimal_virtual_point.is_some() {
        base.binary_decimal_virtual_point = props.binary_decimal_virtual_point.unwrap_or(0);
    }
    if props.binary_decimal_virtual_point_sde.is_some() {
        base.binary_decimal_virtual_point_signed = props.binary_decimal_virtual_point_sde;
    }
    if let Some(signed) = props.decimal_signed {
        base.decimal_signed = signed;
    }
    if props.calendar_pattern.is_some() {
        base.calendar_pattern = props
            .calendar_pattern
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.calendar_pattern_kind {
        base.calendar_pattern_kind = v;
    }
    if props.calendar_time_zone.is_some() {
        base.calendar_time_zone = props
            .calendar_time_zone
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.calendar_time_zone_defined {
        base.calendar_time_zone_defined = true;
    }
    if let Some(start) = props.calendar_century_start {
        base.calendar_century_start = start;
    }
    if props.calendar_language.is_some() {
        base.calendar_language = props
            .calendar_language
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(segs) = &props.calendar_language_segments {
        base.calendar_language_segments = Some(intern_input_value_calc_segments(segs, strings));
    }
    if let Some(v) = props.calendar_days_in_first_week {
        base.calendar_days_in_first_week = v;
    }
    if let Some(ref raw) = props.calendar_first_day_of_week {
        base.calendar_first_day_of_week = calendar_first_day_of_week_iso(raw);
    }
    if props.calendar_check_policy_lax == Some(true) {
        base.calendar_check_policy_lax = true;
    }
    if props.text_number_pattern.is_some() {
        base.text_number_pattern = props
            .text_number_pattern
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.text_number_check_policy {
        base.text_number_check_policy = v;
    }
    if let Some(v) = props.text_number_rep {
        base.text_number_rep = v;
    }
    if let Some(v) = props.text_number_rounding {
        base.text_number_rounding = v;
    }
    if props.text_number_rounding_increment.is_some() {
        let raw = props.text_number_rounding_increment.as_deref().unwrap_or("0");
        base.text_number_rounding_increment = strings.intern(raw);
        base.text_number_rounding_increment_defined = true;
    }
    if let Some(v) = props.text_number_rounding_mode {
        base.text_number_rounding_mode = v;
    }
    if props.text_zoned_sign_style.is_some() {
        base.text_zoned_sign_style = props.text_zoned_sign_style;
    }
    if props.text_standard_decimal_separator_sibling.is_some() {
        base.text_standard_decimal_separator_sibling = props
            .text_standard_decimal_separator_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
        base.text_standard_decimal_separator_defined = true;
    } else if props.text_standard_decimal_separator.is_some() {
        let raw = props
            .text_standard_decimal_separator
            .as_deref()
            .unwrap_or(".");
        if let Err(detail) =
            validate_text_standard_separator_literal("textStandardDecimalSeparator", raw)
        {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {detail}"),
            }
            .into());
        }
        base.text_standard_decimal_separator = strings.intern(expand_entities_str(raw));
        base.text_standard_decimal_separator_defined = true;
    }
    if props.text_standard_grouping_separator_sibling.is_some() {
        base.text_standard_grouping_separator_sibling = props
            .text_standard_grouping_separator_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
        base.text_standard_grouping_separator_defined = true;
    } else if props.text_standard_grouping_separator.is_some() {
        let raw = props.text_standard_grouping_separator.as_deref().unwrap_or(",");
        if let Err(detail) =
            validate_text_standard_separator_literal("textStandardGroupingSeparator", raw)
        {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {detail}"),
            }
            .into());
        }
        let g = expand_entities_str(raw);
        base.text_standard_grouping_separator = if g.is_empty() {
            None
        } else {
            Some(strings.intern(g))
        };
        base.text_standard_grouping_separator_defined = true;
    }
    if props.text_standard_exponent_rep_sibling.is_some() {
        base.text_standard_exponent_rep_sibling = props
            .text_standard_exponent_rep_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    } else if props.text_standard_exponent_rep.is_some() {
        let raw = props.text_standard_exponent_rep.as_deref().unwrap_or("E");
        if !raw.is_empty() {
            validate_text_standard_exponent_rep_literal(raw).map_err(|msg| {
                SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error: {msg}"),
                }
            })?;
        }
        base.text_standard_exponent_rep = strings.intern(expand_entities_str(raw));
        base.text_standard_exponent_rep_defined = true;
    }
    if props.text_standard_zero_rep.is_some() {
        let raw = props.text_standard_zero_rep.as_deref().unwrap_or("");
        validate_text_standard_zero_rep_literal(raw).map_err(|msg| {
            SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
        })?;
        base.text_standard_zero_rep = strings.intern(raw);
        base.text_standard_zero_rep_defined = true;
    }
    if props.text_standard_infinity_rep.is_some() {
        let raw = props
            .text_standard_infinity_rep
            .as_deref()
            .unwrap_or("Inf");
        validate_text_standard_special_value_literal("textStandardInfinityRep", raw).map_err(
            |msg| SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {msg}"),
            },
        )?;
        base.text_standard_infinity_rep = strings.intern(expand_entities_str(raw));
    }
    if props.text_standard_nan_rep.is_some() {
        let raw = props.text_standard_nan_rep.as_deref().unwrap_or("NaN");
        validate_text_standard_special_value_literal("textStandardNaNRep", raw).map_err(|msg| {
            SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
        })?;
        base.text_standard_nan_rep = strings.intern(expand_entities_str(raw));
    }
    if let Some(ref s) = props.initiator {
        if !s.is_empty() {
            base.initiator = Some(strings.intern(s.clone()));
        }
    }
    if let Some(ref s) = props.terminator {
        if !s.is_empty() {
            base.terminator = Some(strings.intern(s.clone()));
        }
    }
    if let Some(ref s) = props.separator {
        if s.is_empty() {
            base.separator = None;
        } else {
            base.separator = Some(strings.intern(s.clone()));
        }
    }
    if let Some(ref s) = props.output_new_line {
        if !s.is_empty() {
            base.output_new_line = Some(strings.intern(s.clone()));
        }
    }
    if props.occurs_min.is_some() {
        base.occurs_min = props.occurs_min.unwrap_or(1);
    }
    if props.max_occurs_specified {
        base.occurs_max = props.occurs_max;
    } else if props.occurs_count_kind == Some(OccursCountKind::Expression)
        && props.occurs_max.is_some()
    {
        base.occurs_max = props.occurs_max;
    } else if props.occurs_count_kind == Some(OccursCountKind::Parsed)
        && props.occurs_min.unwrap_or(1) == 0
    {
        base.occurs_max = None;
    }
    if props.length_pattern.is_some() {
        base.length_pattern = props
            .length_pattern
            .as_ref()
            .map(|p| strings.intern(p.clone()));
    }
    if let Some(v) = props.separator_position {
        base.separator_position = v;
    }
    if props.text_boolean_true_rep.is_some() {
        base.text_boolean_true_rep = props
            .text_boolean_true_rep
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.text_boolean_false_rep.is_some() {
        base.text_boolean_false_rep = props
            .text_boolean_false_rep
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.text_boolean_true_rep_defined {
        base.text_boolean_true_rep_defined = true;
    }
    if props.text_boolean_false_rep_defined {
        base.text_boolean_false_rep_defined = true;
    }
    if props.text_boolean_pad_character.is_some() {
        base.text_boolean_pad_character = props
            .text_boolean_pad_character
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.binary_boolean_true_rep_defined {
        base.binary_boolean_true_rep_defined = true;
        base.binary_boolean_true_rep = props.binary_boolean_true_rep;
    }
    if props.binary_boolean_false_rep_defined {
        base.binary_boolean_false_rep_defined = true;
        base.binary_boolean_false_rep = props.binary_boolean_false_rep;
    }
    if props.default_value.is_some() {
        base.default_value = props
            .default_value
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.sequence_kind {
        base.sequence_kind = v;
    }
    if let Some(v) = props.choice_length_kind {
        base.choice_length_kind = v;
    }
    if props.choice_length.is_some() {
        base.choice_length = props.choice_length;
    }
    if let Some(v) = props.object_kind {
        base.object_kind = v;
    }
    if let Some(v) = props.input_value_calc {
        base.input_value_calc = Some(v);
    }
    if props.input_value_calc_literal.is_some() {
        base.input_value_calc_literal = props
            .input_value_calc_literal
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.input_value_calc_sibling.is_some() {
        base.input_value_calc_sibling = props
            .input_value_calc_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(segments) = &props.input_value_calc_segments {
        base.input_value_calc_segments = Some(intern_input_value_calc_segments(segments, strings));
    }
    if let Some(steps) = &props.input_value_calc_path {
        base.input_value_calc_path = Some(intern_input_path_steps(steps, strings));
    }
    if let Some(expr) = &props.input_value_calc_expression {
        base.input_value_calc_expression = Some(intern_input_value_calc_expression(expr, strings));
    }
    if props.choice_dispatch_sibling.is_some() {
        base.choice_dispatch_sibling = props
            .choice_dispatch_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(steps) = &props.choice_dispatch_path {
        base.choice_dispatch_path = Some(intern_input_path_steps(steps, strings));
    }
    if let Some(steps) = &props.occurs_count_fn_path {
        base.occurs_count_fn_path = Some(intern_input_path_steps(steps, strings));
    }
    if props.choice_dispatch_literal.is_some() {
        base.choice_dispatch_literal = props
            .choice_dispatch_literal
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.choice_dispatch_sibling_int.is_some() {
        base.choice_dispatch_sibling_int = props
            .choice_dispatch_sibling_int
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.discriminator_test.is_some() {
        base.discriminator_test = props
            .discriminator_test
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    for (name, val) in &props.set_variables {
        base.set_variables.push((
            strings.intern(name.clone()),
            strings.intern(val.clone()),
        ));
    }
    if let Some(v) = props.output_value_calc {
        base.output_value_calc = Some(v);
    }
    if props.output_value_calc_literal.is_some() {
        base.output_value_calc_literal = props
            .output_value_calc_literal
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.output_value_calc_conditional {
        base.output_value_calc_conditional = true;
    }
    if props.output_value_calc_sibling.is_some() {
        base.output_value_calc_sibling = props
            .output_value_calc_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(steps) = &props.output_value_calc_path {
        base.output_value_calc_path = Some(intern_input_path_steps(steps, strings));
    }
    if props.output_value_calc_path_addend.is_some() {
        base.output_value_calc_path_addend = props.output_value_calc_path_addend;
    }
    if let Some(v) = props.text_string_justification {
        base.text_string_justification = v;
    }
    if let Some(v) = props.text_number_justification {
        base.text_number_justification = v;
    }
    if let Some(v) = props.text_standard_base {
        base.text_standard_base = v;
    }
    if props.alignment_implicit == Some(true) {
        base.alignment_implicit = true;
        base.alignment = 0;
    } else if props.alignment.is_some() {
        base.alignment = props.alignment.unwrap_or(0);
        base.alignment_implicit = false;
    }
    if let Some(v) = props.alignment_units {
        base.alignment_units = v;
    }
    if props.leading_skip.is_some() {
        base.leading_skip = props.leading_skip.unwrap_or(0);
    }
    if props.trailing_skip.is_some() {
        base.trailing_skip = props.trailing_skip.unwrap_or(0);
    }
    if let Some(ref raw) = props.fill_byte_raw {
        if raw.trim() == "%NUL;" {
            base.fill_byte = 0;
            base.fill_byte_defined = true;
            base.fill_byte_explicit = true;
            base.fill_byte_utf8 = Some(alloc::vec![0]);
        } else if let Some(ref bytes) = props.fill_byte {
            let encoding = strings
                .get(base.encoding)
                .unwrap_or("ISO-8859-1");
            validate_fill_byte_schema(raw, bytes, encoding)?;
            let char_literal = !raw.trim().starts_with('%');
            if char_literal
                && (crate::vm::encoding::bits_charset_spec(encoding).is_some()
                    || encoding
                        .to_ascii_uppercase()
                        .contains("US-ASCII-7-BIT-PACKED"))
            {
                let ch = core::str::from_utf8(bytes).unwrap_or("");
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: The fillByte property cannot be specified as a character ('{ch}') when the dfdl:encoding property is '{encoding}' because that encoding is not a single-byte character set."
                    ),
                }
                .into());
            }
            base.fill_byte = bytes.first().copied().unwrap_or(0);
            base.fill_byte_defined = true;
            base.fill_byte_explicit = true;
            base.fill_byte_utf8 = Some(bytes.clone());
        }
    } else if let Some(ref bytes) = props.fill_byte {
        base.fill_byte = bytes.first().copied().unwrap_or(0);
        base.fill_byte_defined = true;
        base.fill_byte_utf8 = Some(bytes.clone());
    }
    if let Some(v) = props.prefix_includes_prefix_length {
        base.prefix_includes_prefix_length = v;
    }
    Ok(base)
}

/// Element props that affect simple-type compile-time validation but must not
/// pre-merge alignment (handled in [`merge_ir_props`] for framing split).
fn element_props_for_simple_type_compile(element: &DfdlProps) -> DfdlProps {
    DfdlProps {
        length: element.length,
        length_kind: element.length_kind,
        length_units: element.length_units,
        length_sibling: element.length_sibling.clone(),
        length_sibling_cast_long: element.length_sibling_cast_long,
        length_expr_unparsed: element.length_expr_unparsed,
        length_self_string_max_cap: element.length_self_string_max_cap,
        length_self_value_length: element.length_self_value_length,
        length_pattern: element.length_pattern.clone(),
        prefix_length_type: element.prefix_length_type.clone(),
        prefix_includes_prefix_length: element.prefix_includes_prefix_length,
        input_value_calc: element.input_value_calc,
        input_value_calc_literal: element.input_value_calc_literal.clone(),
        input_value_calc_sibling: element.input_value_calc_sibling.clone(),
        input_value_calc_segments: element.input_value_calc_segments.clone(),
        input_value_calc_path: element.input_value_calc_path.clone(),
        input_value_calc_expression: element.input_value_calc_expression.clone(),
        ..DfdlProps::default()
    }
}

fn merge_ir_props(base: &IrProps, overlay: &IrProps) -> IrProps {
    let mut out = base.clone();
    out.representation = overlay.representation;
    out.byte_order = overlay.byte_order;
    if overlay.byte_order_defined {
        out.byte_order_defined = true;
    }
    out.bit_order = overlay.bit_order;
    if overlay.bit_order_defined {
        out.bit_order_defined = true;
    }
    if matches!(
        base.length_kind,
        LengthKind::Explicit
            | LengthKind::Fixed
            | LengthKind::Prefixed
            | LengthKind::Delimited
            | LengthKind::Pattern
    ) && overlay.length_kind == LengthKind::Implicit
        && overlay.length.is_none()
        && overlay.length_pattern.is_none()
    {
        // Keep type-derived lengthKind when overlay only carries inherited implicit defaults
        // (including when the overlay adds a runtime length expression via length_sibling).
    } else if matches!(
        base.length_kind,
        LengthKind::Explicit | LengthKind::Fixed | LengthKind::Prefixed | LengthKind::Pattern
    ) && overlay.length_kind == LengthKind::Delimited
        && overlay.length.is_none()
        && overlay.length_pattern.is_none()
    {
        // Format default lengthKind=delimited must not override explicit simpleType / element length.
    } else {
        out.length_kind = overlay.length_kind;
    }
    if overlay.length_kind_defined {
        out.length_kind_defined = true;
    }
    if overlay.length.is_some() {
        out.length = overlay.length;
    }
    if overlay.length_sibling.is_some() {
        out.length_sibling = overlay.length_sibling;
    }
    if overlay.length_sibling_cast_long {
        out.length_sibling_cast_long = true;
    }
    if overlay.length_expr_unparsed {
        out.length_expr_unparsed = true;
    }
    if overlay.length_self_string_max_cap.is_some() {
        out.length_self_string_max_cap = overlay.length_self_string_max_cap;
    }
    if overlay.length_self_value_length {
        out.length_self_value_length = true;
    }
    if matches!(
        base.length_kind,
        LengthKind::Explicit | LengthKind::Fixed | LengthKind::Prefixed
    ) && matches!(overlay.length_kind, LengthKind::Implicit)
        && overlay.length.is_none()
        && overlay.length_pattern.is_none()
    {
        // Preserve type/element lengthUnits; ancestor overlay only carries format defaults.
    } else {
        out.length_units = overlay.length_units;
    }
    out.encoding = overlay.encoding;
    out.encoding_error_policy = overlay.encoding_error_policy;
    out.nillable = overlay.nillable;
    if overlay.nil_kind.is_some() {
        out.nil_kind = overlay.nil_kind;
    }
    if overlay.nil_value.is_some() {
        out.nil_value = overlay.nil_value;
    }
    if overlay.separator_suppression_policy.is_some() {
        out.separator_suppression_policy = overlay.separator_suppression_policy;
    }
    out.empty_element_parse_policy = overlay.empty_element_parse_policy;
    out.occurs_count_kind = overlay.occurs_count_kind;
    out.ignore_case = overlay.ignore_case;
    if overlay.text_trim_kind != TextTrimKind::None {
        out.text_trim_kind = overlay.text_trim_kind;
    }
    out.text_pad_kind = overlay.text_pad_kind;
    if overlay.text_number_pad_character.is_some() {
        out.text_number_pad_character = overlay.text_number_pad_character;
    }
    out.text_number_pad_character_property_form =
        overlay.text_number_pad_character_property_form;
    out.text_string_pad_character = overlay.text_string_pad_character;
    out.text_string_pad_character_property_form =
        overlay.text_string_pad_character_property_form;
    if overlay.text_calendar_pad_character.is_some() {
        out.text_calendar_pad_character = overlay.text_calendar_pad_character;
    }
    if overlay.text_calendar_justification.is_some() {
        out.text_calendar_justification = overlay.text_calendar_justification;
    }
    out.binary_number_rep = overlay.binary_number_rep;
    out.binary_packed_sign_codes = overlay.binary_packed_sign_codes;
    if overlay.binary_packed_sign_codes_defined {
        out.binary_packed_sign_codes_defined = true;
    }
    out.binary_number_check_policy = overlay.binary_number_check_policy;
    out.binary_calendar_rep = overlay.binary_calendar_rep;
    if overlay.binary_calendar_epoch.is_some() {
        out.binary_calendar_epoch = overlay.binary_calendar_epoch;
    }
    out.binary_float_rep = overlay.binary_float_rep;
    out.binary_decimal_virtual_point = overlay.binary_decimal_virtual_point;
    if overlay.binary_decimal_virtual_point_signed.is_some() {
        out.binary_decimal_virtual_point_signed = overlay.binary_decimal_virtual_point_signed;
    }
    out.decimal_signed = overlay.decimal_signed;
    if overlay.calendar_pattern.is_some() {
        out.calendar_pattern = overlay.calendar_pattern;
        if base.calendar_pattern_kind == crate::schema::CalendarPatternKind::Explicit
            || overlay.calendar_pattern_kind == crate::schema::CalendarPatternKind::Explicit
        {
            out.calendar_pattern_kind = crate::schema::CalendarPatternKind::Explicit;
        } else {
            out.calendar_pattern_kind = overlay.calendar_pattern_kind;
        }
    } else {
        out.calendar_pattern_kind = overlay.calendar_pattern_kind;
    }
    if overlay.calendar_time_zone.is_some() {
        out.calendar_time_zone = overlay.calendar_time_zone;
    }
    if overlay.calendar_time_zone_defined {
        out.calendar_time_zone_defined = true;
    }
    out.calendar_century_start = overlay.calendar_century_start;
    if overlay.calendar_language.is_some() {
        out.calendar_language = overlay.calendar_language;
    }
    if overlay.calendar_language_segments.is_some() {
        out.calendar_language_segments = overlay.calendar_language_segments.clone();
    }
    out.calendar_days_in_first_week = overlay.calendar_days_in_first_week;
    out.calendar_first_day_of_week = overlay.calendar_first_day_of_week;
    if overlay.calendar_check_policy_lax {
        out.calendar_check_policy_lax = true;
    }
    if overlay.text_number_pattern.is_some() {
        out.text_number_pattern = overlay.text_number_pattern;
    }
    out.text_number_check_policy = overlay.text_number_check_policy;
    out.text_number_rep = overlay.text_number_rep;
    out.text_zoned_sign_style = overlay.text_zoned_sign_style;
    if overlay.text_standard_decimal_separator != StringId(0) {
        out.text_standard_decimal_separator = overlay.text_standard_decimal_separator;
    }
    if overlay.text_standard_decimal_separator_defined {
        out.text_standard_decimal_separator_defined = true;
    }
    if overlay.text_standard_decimal_separator_sibling.is_some() {
        out.text_standard_decimal_separator_sibling = overlay.text_standard_decimal_separator_sibling;
    }
    out.text_standard_grouping_separator = overlay.text_standard_grouping_separator;
    if overlay.text_standard_grouping_separator_defined {
        out.text_standard_grouping_separator_defined = true;
    }
    if overlay.text_standard_grouping_separator_sibling.is_some() {
        out.text_standard_grouping_separator_sibling = overlay.text_standard_grouping_separator_sibling;
    }
    if overlay.text_standard_exponent_rep != StringId(0) {
        out.text_standard_exponent_rep = overlay.text_standard_exponent_rep;
    }
    if overlay.text_standard_exponent_rep_sibling.is_some() {
        out.text_standard_exponent_rep_sibling = overlay.text_standard_exponent_rep_sibling;
    }
    if overlay.text_standard_infinity_rep != StringId(0) {
        out.text_standard_infinity_rep = overlay.text_standard_infinity_rep;
    }
    if overlay.text_standard_nan_rep != StringId(0) {
        out.text_standard_nan_rep = overlay.text_standard_nan_rep;
    }
    if overlay.text_standard_zero_rep != StringId(0) {
        out.text_standard_zero_rep = overlay.text_standard_zero_rep;
        out.text_standard_zero_rep_defined = overlay.text_standard_zero_rep_defined;
    } else if overlay.text_standard_zero_rep_defined {
        out.text_standard_zero_rep_defined = true;
    }
    if overlay.text_standard_exponent_rep_defined {
        out.text_standard_exponent_rep_defined = true;
    }
    if overlay.initiator.is_some() {
        out.initiator = overlay.initiator;
    }
    if overlay.terminator.is_some() {
        out.terminator = overlay.terminator;
    }
    if overlay.separator.is_some() {
        out.separator = overlay.separator;
    }
    if overlay.output_new_line.is_some() {
        out.output_new_line = overlay.output_new_line;
    }
    if overlay.length_pattern.is_some() {
        out.length_pattern = overlay.length_pattern;
    }
    out.separator_position = overlay.separator_position;
    if overlay.text_boolean_true_rep.is_some() {
        out.text_boolean_true_rep = overlay.text_boolean_true_rep;
    }
    if overlay.text_boolean_false_rep.is_some() {
        out.text_boolean_false_rep = overlay.text_boolean_false_rep;
    }
    if overlay.text_boolean_true_rep_defined {
        out.text_boolean_true_rep_defined = true;
    }
    if overlay.text_boolean_false_rep_defined {
        out.text_boolean_false_rep_defined = true;
    }
    if overlay.text_boolean_pad_character.is_some() {
        out.text_boolean_pad_character = overlay.text_boolean_pad_character;
    }
    if overlay.binary_boolean_true_rep_defined {
        out.binary_boolean_true_rep_defined = true;
        out.binary_boolean_true_rep = overlay.binary_boolean_true_rep;
    }
    if overlay.binary_boolean_false_rep_defined {
        out.binary_boolean_false_rep_defined = true;
        out.binary_boolean_false_rep = overlay.binary_boolean_false_rep;
    }
    if overlay.default_value.is_some() {
        out.default_value = overlay.default_value;
    }
    out.sequence_kind = overlay.sequence_kind;
    out.occurs_min = overlay.occurs_min;
    out.occurs_max = overlay.occurs_max;
    if overlay.alignment_implicit {
        out.alignment_implicit = true;
        out.alignment = 0;
    } else if overlay.alignment != 0 {
        if out.framing_alignment == 0 {
            if out.alignment != 0
                && !out.alignment_implicit
                && overlay.alignment != out.alignment
            {
                out.framing_alignment = out.alignment;
                out.framing_alignment_units = out.alignment_units;
            } else if out.alignment_implicit {
                // Element pre-align override; post-read keeps schema format bit alignment.
                out.framing_alignment = 4;
                out.framing_alignment_units = LengthUnits::Bits;
            }
        }
        out.alignment = overlay.alignment;
        out.alignment_implicit = false;
    }
    out.alignment_units = overlay.alignment_units;
    if overlay.leading_skip != 0 {
        out.leading_skip = overlay.leading_skip;
    }
    if overlay.trailing_skip != 0 {
        out.trailing_skip = overlay.trailing_skip;
    }
    if overlay.fill_byte_defined {
        out.fill_byte = overlay.fill_byte;
        out.fill_byte_defined = true;
        if overlay.fill_byte_explicit {
            out.fill_byte_explicit = true;
        }
        if overlay.fill_byte_utf8.is_some() {
            out.fill_byte_utf8 = overlay.fill_byte_utf8.clone();
        }
    } else if overlay.fill_byte != 0 && !out.fill_byte_defined {
        out.fill_byte = overlay.fill_byte;
    }
    out.input_value_calc = overlay.input_value_calc;
    out.input_value_calc_literal = overlay.input_value_calc_literal;
    out.input_value_calc_sibling = overlay.input_value_calc_sibling;
    out.input_value_calc_segments = overlay.input_value_calc_segments.clone();
    if overlay.input_value_calc_path.is_some() {
        out.input_value_calc_path = overlay.input_value_calc_path.clone();
    }
    if overlay.input_value_calc_expression.is_some() {
        out.input_value_calc_expression = overlay.input_value_calc_expression.clone();
    }
    if overlay.calendar_date_only {
        out.calendar_date_only = true;
    }
    out.output_value_calc = overlay.output_value_calc;
    out.output_value_calc_literal = overlay.output_value_calc_literal;
    out.output_value_calc_sibling = overlay.output_value_calc_sibling;
    if overlay.output_value_calc_path.is_some() {
        out.output_value_calc_path = overlay.output_value_calc_path.clone();
    }
    if overlay.output_value_calc_path_addend.is_some() {
        out.output_value_calc_path_addend = overlay.output_value_calc_path_addend;
    }
    if overlay.output_value_calc_conditional {
        out.output_value_calc_conditional = true;
    }
    out.text_string_justification = overlay.text_string_justification;
    out.text_number_justification = overlay.text_number_justification;
    out.text_standard_base = overlay.text_standard_base;
    out.truncate_specified_length_string = overlay.truncate_specified_length_string;
    if overlay.min_length.is_some() {
        out.min_length = overlay.min_length;
    }
    if overlay.max_length.is_some() {
        out.max_length = overlay.max_length;
    }
    if overlay.facet_length.is_some() {
        out.facet_length = overlay.facet_length;
    }
    if overlay.implicit_facet_length.is_some() {
        out.implicit_facet_length = overlay.implicit_facet_length;
    }
    if !overlay.facet_pattern_groups.is_empty() {
        out.facet_pattern_groups = overlay.facet_pattern_groups.clone();
    }
    if !overlay.facet_enumeration.is_empty() {
        out.facet_enumeration = overlay.facet_enumeration.clone();
    }
    if overlay.value_min_inclusive.is_some() {
        out.value_min_inclusive = overlay.value_min_inclusive;
    }
    if overlay.value_max_inclusive.is_some() {
        out.value_max_inclusive = overlay.value_max_inclusive;
    }
    if overlay.value_min_exclusive.is_some() {
        out.value_min_exclusive = overlay.value_min_exclusive;
    }
    if overlay.value_max_exclusive.is_some() {
        out.value_max_exclusive = overlay.value_max_exclusive;
    }
    if overlay.value_min_inclusive_lexical.is_some() {
        out.value_min_inclusive_lexical = overlay.value_min_inclusive_lexical;
    }
    if overlay.value_max_inclusive_lexical.is_some() {
        out.value_max_inclusive_lexical = overlay.value_max_inclusive_lexical;
    }
    if overlay.value_min_exclusive_lexical.is_some() {
        out.value_min_exclusive_lexical = overlay.value_min_exclusive_lexical;
    }
    if overlay.value_max_exclusive_lexical.is_some() {
        out.value_max_exclusive_lexical = overlay.value_max_exclusive_lexical;
    }
    if overlay.total_digits.is_some() {
        out.total_digits = overlay.total_digits;
    }
    if overlay.fraction_digits.is_some() {
        out.fraction_digits = overlay.fraction_digits;
    }
    if overlay.facet_check_constraints {
        out.facet_check_constraints = true;
    }
    if overlay.assert_int_eq.is_some() {
        out.assert_int_eq = overlay.assert_int_eq;
    }
    if overlay.assert_eq_occurs_index_addend.is_some() {
        out.assert_eq_occurs_index_addend = overlay.assert_eq_occurs_index_addend;
    }
    if overlay.discriminator_test.is_some() {
        out.discriminator_test = overlay.discriminator_test;
    }
    if overlay.facet_assert_message.is_some() {
        out.facet_assert_message = overlay.facet_assert_message;
    }
    if overlay.facet_assert_message_segments.is_some() {
        out.facet_assert_message_segments = overlay.facet_assert_message_segments.clone();
    }
    if overlay.facet_assert_daffodil_prefix {
        out.facet_assert_daffodil_prefix = true;
    }
    if overlay.prefix_length.is_some() {
        out.prefix_length = overlay.prefix_length.clone();
    }
    out.prefix_includes_prefix_length = overlay.prefix_includes_prefix_length;
    if overlay.xsd_type.is_some() {
        out.xsd_type = overlay.xsd_type;
    }
    if overlay.escape_scheme.is_some() {
        out.escape_scheme = overlay.escape_scheme.clone();
    }
    if (out.input_value_calc.is_some()
        || out.input_value_calc_sibling.is_some()
        || out.input_value_calc_segments.is_some()
        || out.input_value_calc_path.is_some()
        || out.input_value_calc_expression.is_some())
        && out.length.is_none()
        && matches!(out.length_kind, LengthKind::Explicit | LengthKind::Fixed)
    {
        out.length_kind = LengthKind::Implicit;
    }
    out
}

/// Compile a parsed XSD/DFDL schema document into an [`IrProgram`].
pub fn compile(schema: &SchemaDocument) -> Result<IrProgram> {
    compile_named(schema, None)
}

/// Compile using an explicit root element name, or the sole global element.
pub fn compile_named(schema: &SchemaDocument, root: Option<&str>) -> Result<IrProgram> {
    compile_named_with_tunables(schema, root, DaffodilTunables::default())
}

/// Compile with Daffodil tunables from TDML test configuration.
pub fn compile_named_with_tunables(
    schema: &SchemaDocument,
    root: Option<&str>,
    tunables: DaffodilTunables,
) -> Result<IrProgram> {
    let root_name = match root {
        Some(name) => name.to_string(),
        None => {
            if schema.global_elements.is_empty() {
                return Err(SchemaError::NoRootElement.into());
            }
            if schema.global_elements.len() > 1 {
                return Err(SchemaError::AmbiguousRootElement.into());
            }
            schema
                .global_elements
                .keys()
                .next()
                .cloned()
                .ok_or(SchemaError::NoRootElement)?
        }
    };

    crate::parse_unparse_policy::validate_parse_unparse_policy(schema, &root_name)?;
    crate::tunable_validate::validate_tunable_schema_requirements(schema, &root_name, &tunables)?;
    crate::schema_validate::validate_compiled_schema(schema, &root_name, &tunables)?;
    IrBuilder::new(schema, tunables)?.build(&root_name)
}

fn intern_input_path_steps(
    steps: &[(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
    )],
    strings: &mut StringPool,
) -> alloc::vec::Vec<crate::ir::IrInputPathStep> {
    steps
        .iter()
        .map(|(prefix, local, index)| crate::ir::IrInputPathStep {
            prefix: prefix.as_ref().map(|p| strings.intern(p.clone())),
            local: strings.intern(local.clone()),
            index: *index,
        })
        .collect()
}

fn intern_input_path_steps_legacy(
    steps: &[(
        Option<alloc::string::String>,
        alloc::string::String,
    )],
    strings: &mut StringPool,
) -> alloc::vec::Vec<crate::ir::IrInputPathStep> {
    steps
        .iter()
        .map(|(prefix, local)| crate::ir::IrInputPathStep {
            prefix: prefix.as_ref().map(|p| strings.intern(p.clone())),
            local: strings.intern(local.clone()),
            index: None,
        })
        .collect()
}

fn intern_input_value_calc_expression(
    expr: &crate::schema::InputValueCalcExpression,
    strings: &mut crate::ir::StringPool,
) -> crate::ir::IrInputValueCalcExpression {
    use crate::ir::IrInputValueCalcExpression;
    use crate::schema::InputValueCalcExpression;
    match expr {
        InputValueCalcExpression::Add(items) => IrInputValueCalcExpression::Add(
            items
                .iter()
                .map(|e| intern_input_value_calc_expression(e, strings))
                .collect(),
        ),
        InputValueCalcExpression::Mul(items) => IrInputValueCalcExpression::Mul(
            items
                .iter()
                .map(|e| intern_input_value_calc_expression(e, strings))
                .collect(),
        ),
        InputValueCalcExpression::Path(steps) => {
            IrInputValueCalcExpression::Path(intern_input_path_steps(steps, strings))
        }
        InputValueCalcExpression::StringOf(inner) => IrInputValueCalcExpression::StringOf(
            alloc::boxed::Box::new(intern_input_value_calc_expression(inner, strings)),
        ),
    }
}

fn intern_input_value_calc_segments(
    segments: &[crate::schema::InputValueCalcSegment],
    strings: &mut crate::ir::StringPool,
) -> alloc::vec::Vec<crate::ir::IrInputValueCalcSegment> {
    segments
        .iter()
        .map(|seg| match seg {
            crate::schema::InputValueCalcSegment::Sibling(name) => {
                crate::ir::IrInputValueCalcSegment::Sibling(strings.intern(name.clone()))
            }
            crate::schema::InputValueCalcSegment::Literal(text) => {
                crate::ir::IrInputValueCalcSegment::Literal(strings.intern(text.clone()))
            }
            crate::schema::InputValueCalcSegment::Substring {
                sibling,
                start,
                length,
            } => crate::ir::IrInputValueCalcSegment::Substring {
                sibling: strings.intern(sibling.clone()),
                start: *start as u32,
                length: *length as u32,
            },
            crate::schema::InputValueCalcSegment::InfosetPath(steps) => {
                crate::ir::IrInputValueCalcSegment::InfosetPath(intern_input_path_steps(
                    steps, strings,
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::parse_schema;
    use crate::value::DfdlValue;

    #[test]
    fn test05_choice_dispatch_ir() {
        use crate::schema::parse_schema_with_options;
        use std::fs;
        use std::path::Path;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section06/namespaces");
        let xsd = fs::read_to_string(dir.join("test05sch1.dfdl.xsd")).unwrap();
        let schema = parse_schema_with_options(
            &xsd,
            &crate::schema::ParseOptions {
                base_dir: Some(dir.to_string_lossy().into()),
                schema_label: Some("test05sch1.dfdl.xsd".into()),
            },
        )
        .expect("parse");
        let ty = schema
            .resolve_type(&crate::schema::TypeName::new("rootType"))
            .unwrap();
        if let crate::schema::TypeDef::Complex { content, .. } = ty {
            if let crate::schema::ComplexContent::Sequence(seq) = content {
                if let crate::schema::Particle::Choice(ch) = &seq.particles[1] {
                    assert_eq!(
                        ch.props.choice_dispatch_sibling.as_deref(),
                        Some("elem")
                    );
                    if let crate::schema::Particle::Element(el) = &ch.branches[0] {
                        assert_eq!(el.props.choice_branch_key.as_deref(), Some("1"));
                    }
                }
            }
        }
        let program = compile_named(&schema, Some("root")).expect("compile");
        let root = program.node(program.root).unwrap();
        if let IrNode::Element { child: Some(id), .. } = root {
            if let IrNode::Sequence { children, .. } = program.node(*id).unwrap() {
                if let IrNode::Choice { branches, props } = program.node(children[1]).unwrap() {
                    assert!(props.choice_dispatch_sibling.is_some());
                    assert!(branches[0].branch_key.is_some());
                }
            }
        }
        let spec =
            crate::api::DfdlSpec::from_schema_root_with_tunables(schema, Some("root"), Default::default())
                .expect("spec");
        let val = spec.decode(b"1ab").expect("decode");
        let DfdlValue::Sequence(root) = val else {
            panic!("expected root sequence");
        };
        let inner = root.fields.get("root").expect("root element");
        let DfdlValue::Sequence(fields) = inner else {
            panic!("expected root complex");
        };
        assert!(fields.fields.contains_key("elem1"));
        assert!(!fields.fields.contains_key("elem1_fields"));
    }

    #[test]
    fn compile_record_schema() {
        let xsd = include_str!("../../tests/fixtures/record.xsd");
        let schema = parse_schema(xsd).expect("parse");
        let program = compile(&schema).expect("compile");
        assert_eq!(program.root_element, "Record");
        assert!(!program.nodes.is_empty());
    }

    #[test]
    fn text_message_tag_has_fixed_length() {
        use crate::schema::{LengthKind, Representation};
        let xsd = include_str!("../../tests/fixtures/text_message.xsd");
        let schema = parse_schema(xsd).expect("parse");
        let ty = schema.resolve_type(&crate::schema::TypeName::new("MessageType")).unwrap();
        if let crate::schema::TypeDef::Complex { content, .. } = ty {
            if let crate::schema::ComplexContent::Sequence(seq) = content {
                let tag = &seq.particles[0];
                if let crate::schema::Particle::Element(el) = tag {
                    assert_eq!(el.props.length_kind, Some(LengthKind::Fixed));
                    assert_eq!(el.props.length, Some(3));
                    assert_eq!(el.props.representation, Some(Representation::Text));
                }
            }
        }
        let program = compile(&schema).expect("compile");
        let tag_node = program.nodes.iter().find_map(|n| {
            if let IrNode::Element { name, props, .. } = n {
                if program.strings.get(*name).ok() == Some("tag") {
                    return Some(props.clone());
                }
            }
            None
        }).expect("tag node");
        assert_eq!(tag_node.length_kind, LengthKind::Fixed);
        assert_eq!(tag_node.length, Some(3));
    }

    #[test]
    fn simple_type_leading_skip_on_referenced_element() {
        use crate::schema::LengthUnits;
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
            xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
            xmlns:ex="http://example.com">
          <dfdl:format representation="binary" encoding="utf-8" alignmentUnits="bits"/>
          <xs:simpleType name="uByte2Bits" dfdl:lengthKind="explicit" dfdl:lengthUnits="bits"
            dfdl:length="2" dfdl:leadingSkip="4">
            <xs:restriction base="xs:unsignedByte"/>
          </xs:simpleType>
          <xs:element name="root">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="one" type="ex:uByte2Bits"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let schema = crate::schema::parse_schema(xsd).expect("parse");
        let program = compile_named(&schema, Some("root")).expect("compile");
        let one = program
            .nodes
            .iter()
            .find_map(|n| match n {
                IrNode::Element { name, props, .. }
                    if program.strings.get(*name).ok() == Some("one") =>
                {
                    Some(props.clone())
                }
                _ => None,
            })
            .expect("one");
        assert_eq!(one.leading_skip, 4, "leadingSkip from simpleType");
        assert_eq!(one.length_units, LengthUnits::Bits);
        assert_eq!(one.length, Some(2));
    }

    #[test]
    fn simple_type_leading_skip_decodes() {
        use crate::DfdlSpec;
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
            xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
            xmlns:ex="http://example.com">
          <dfdl:format representation="binary" encoding="utf-8" alignmentUnits="bits"/>
          <xs:simpleType name="uByte2Bits" dfdl:lengthKind="explicit" dfdl:lengthUnits="bits"
            dfdl:length="2" dfdl:leadingSkip="4">
            <xs:restriction base="xs:unsignedByte"/>
          </xs:simpleType>
          <xs:element name="root">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="one" type="ex:uByte2Bits"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let schema = parse_schema(xsd).expect("parse");
        let spec = DfdlSpec::from_schema_root_with_tunables(
            schema,
            Some("root"),
            crate::length_validate::DaffodilTunables::default(),
        )
        .expect("spec");
        let one_props = spec
            .program()
            .nodes
            .iter()
            .find_map(|n| match n {
                IrNode::Element { name, props, .. }
                    if spec.program().strings.get(*name).ok() == Some("one") =>
                {
                    Some(props.leading_skip)
                }
                _ => None,
            })
            .expect("one props");
        assert_eq!(one_props, 4);
        // skip 0000 then value 11 (3)
        let data = [0x0E_u8];
        let value = spec.decode(&data).expect("decode");
        let crate::value::DfdlValue::Sequence(fields) = value else {
            panic!("expected sequence");
        };
        let inner = match fields.fields.get("root") {
            Some(crate::value::DfdlValue::Sequence(seq)) => seq,
            other => panic!("expected root sequence, got {other:?}"),
        };
        assert_eq!(
            inner.fields.get("one"),
            Some(&crate::value::DfdlValue::UnsignedByte(3))
        );
    }

    #[test]
    fn fixed_occurs_min_max_mismatch_is_sde() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
            xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" xmlns:ex="http://example.com">
          <dfdl:format representation="text" encoding="utf-8" occursCountKind="fixed"
            lengthKind="delimited" separatorSuppressionPolicy="never"/>
          <xs:complexType name="basicType">
            <xs:sequence dfdl:separator="|">
              <xs:element name="data" type="xs:int"
                dfdl:occursCountKind="fixed" minOccurs="2" maxOccurs="3"
                dfdl:textNumberRep="standard" dfdl:lengthKind="delimited" />
            </xs:sequence>
          </xs:complexType>
          <xs:element name="root">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="basic" type="ex:basicType" dfdl:lengthKind="implicit"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let schema = parse_schema(xsd).expect("parse");
        let err = compile_named(&schema, Some("root")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("occursCountKind='fixed'"), "{msg}");
        assert!(msg.contains("equal"), "{msg}");
    }
}
