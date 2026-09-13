use super::{ChoiceBranch, IrNode, IrProgram, IrPrefixLength, IrProps, StringId, StringPool, ValueKind};
use crate::error::{Result, SchemaError};
use crate::length_validate::{
    binary_length_validation_applies, validate_data_length_schema,
    validate_float_double_bit_length_schema, validate_packed_binary_properties_schema,
    validate_signed_one_bit_length_schema, DaffodilTunables,
};
use crate::schema::{
    BuiltinType, ComplexContent, DfdlProps, LengthKind, LengthUnits, Particle, Representation,
    SchemaDocument, SimpleBase, TypeDef, TypeName, expand_entities_str,
    parse_text_standard_separator_list, parse_text_standard_zero_rep_list,
    validate_length_pattern,
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
        })
    }

    fn build(mut self, root_name: &str) -> Result<IrProgram> {
        let root_element = self
            .schema
            .global_elements
            .get(root_name)
            .ok_or_else(|| SchemaError::UndefinedType {
                name: root_name.to_string(),
            })?;

        let root = if let Some(builtin) = BuiltinType::from_xsd(root_element.type_name.as_str()) {
            let kind = value_kind_from_builtin(builtin);
            let defaults = self.defaults.clone();
            let mut props = finalize_element_props(
                kind,
                self.merge_props_full(&defaults, &DfdlProps::default(), &root_element.props)?,
                &self.strings,
                self.tunables,
            )?;
            apply_unsigned_long_flag(&root_element.type_name, &mut props);
            apply_integer_type_flags(&root_element.type_name, &mut props);
            validate_implicit_text_length(kind, &props)?;
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

            if let TypeDef::Simple { base, props, .. } = type_def {
                let defaults = self.defaults.clone();
                let kind = value_kind_from_simple(base);
                let mut ir_props = finalize_element_props(
                    kind,
                    self.merge_props_full(&defaults, props, &root_element.props)?,
                    &self.strings,
                    self.tunables,
                )?;
                apply_unsigned_long_flag(&root_element.type_name, &mut ir_props);
                apply_integer_type_flags(&root_element.type_name, &mut ir_props);
                apply_restriction_facets(&mut ir_props, base);
                validate_implicit_text_length(kind, &ir_props)?;
                let name = self.strings.intern(root_name);
                self.push(IrNode::Element {
                    name,
                    kind,
                    props: ir_props,
                    child: None,
                })
            } else {
                let child = self.compile_type(&root_element.type_name, &root_element.props)?;
                let defaults = self.defaults.clone();
                let mut ir_props = self.merge_props_full(
                    &defaults,
                    &DfdlProps::default(),
                    &root_element.props,
                )?;
                if root_element.props.length_kind.is_none() {
                    ir_props.length_kind = LengthKind::Implicit;
                }
                let ir_props = finalize_element_props(
                    ValueKind::Complex,
                    ir_props,
                    &self.strings,
                    self.tunables,
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
        Ok(IrProgram {
            root_element: root_name.to_string(),
            root,
            nodes: self.nodes,
            strings: self.strings,
            tunables: self.tunables,
        })
    }

    fn compile_type(&mut self, type_name: &TypeName, element_props: &DfdlProps) -> Result<u32> {
        if let Some(BuiltinType::String | BuiltinType::HexBinary) = BuiltinType::from_xsd(type_name.as_str()) {
            return Err(SchemaError::UnsupportedFeature {
                feature: alloc::format!("simple type used as root `{type_name:?}`"),
            }
            .into());
        }

        if let Some(builtin) = BuiltinType::from_xsd(type_name.as_str()) {
            return Err(SchemaError::UnsupportedFeature {
                feature: alloc::format!(
                    "scalar root type `{}` requires an element wrapper",
                    builtin.xsd_name()
                ),
            }
            .into());
        }

        let type_def = self
            .schema
            .resolve_type(type_name)
            .ok_or_else(|| SchemaError::UndefinedType {
                name: type_name.as_str().to_string(),
            })?;

        match type_def {
            TypeDef::Simple { base, props, .. } => {
                let defaults = self.defaults.clone();
                let kind = value_kind_from_simple(base);
                let mut ir_props = finalize_element_props(
                    kind,
                    self.merge_props_full(&defaults, props, element_props)?,
                    &self.strings,
                    self.tunables,
                )?;
                apply_restriction_facets(&mut ir_props, base);
                validate_implicit_text_length(kind, &ir_props)?;
                let name = self.strings.intern("__value");
                Ok(self.push(IrNode::Element {
                    name,
                    kind,
                    props: ir_props,
                    child: None,
                }))
            }
            TypeDef::Complex { content, props, .. } => {
                let defaults = self.defaults.clone();
                let inherited = element_props_for_complex_content(element_props);
                let type_base = self.merge_props_full(&defaults, props, &inherited)?;
                self.compile_complex(content, &type_base)
            }
        }
    }

    fn compile_particle(
        &mut self,
        particle: &Particle,
        inherited: &IrProps,
        prior_element_names: &[String],
    ) -> Result<u32> {
        match particle {
            Particle::Element(element) => {
                let merged =
                    self.merge_props_full(inherited, &element.props, &DfdlProps::default())?;
                validate_text_standard_sibling_order(&merged, prior_element_names, &self.strings)?;
                let name = self.strings.intern(&element.name);
                if let Some(builtin) = BuiltinType::from_xsd(element.type_name.as_str()) {
                    let kind = value_kind_from_builtin(builtin);
                    let mut ir_props = finalize_element_props(
                        kind,
                        merged,
                        &self.strings,
                        self.tunables,
                    )?;
                    apply_unsigned_long_flag(&element.type_name, &mut ir_props);
                    apply_integer_type_flags(&element.type_name, &mut ir_props);
                    validate_implicit_text_length(kind, &ir_props)?;
                    Ok(self.push(IrNode::Element {
                        name,
                        kind,
                        props: ir_props,
                        child: None,
                    }))
                } else {
                    let props = merged;
                    let child = self.compile_type(&element.type_name, &element.props)?;
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
                            let mut merged = finalize_element_props(
                                kind,
                                merge_ir_props(&child_props, &overlay),
                                &self.strings,
                                self.tunables,
                            )?;
                            if let Some(type_def) = self.schema.resolve_type(&element.type_name) {
                                if let TypeDef::Simple { base, .. } = type_def {
                                    apply_restriction_facets(&mut merged, base);
                                }
                            }
                            validate_implicit_text_length(kind, &merged)?;
                            return Ok(self.push(IrNode::Element {
                                name,
                                kind,
                                props: merged,
                                child: None,
                            }));
                        }
                    }
                    let mut ir_props = props;
                    if element.props.length_kind.is_none() {
                        ir_props.length_kind = LengthKind::Implicit;
                    }
                    let ir_props =
                        finalize_element_props(ValueKind::Complex, ir_props, &self.strings, self.tunables)?;
                    Ok(self.push(IrNode::Element {
                        name,
                        kind: ValueKind::Complex,
                        props: ir_props,
                        child: Some(child),
                    }))
                }
            }
            Particle::Sequence(sequence) => {
                let ir_props = self.merge_props_full(inherited, &sequence.props, &DfdlProps::default())?;
                let child_inherited =
                    particle_inherited_for_children(&ir_props, &sequence.props, &self.defaults);
                let mut children = Vec::new();
                let mut prior_element_names: Vec<String> = Vec::new();
                for particle in &sequence.particles {
                    validate_initiated_content_particle(&sequence.props, particle)?;
                    children.push(self.compile_particle(
                        particle,
                        &child_inherited,
                        &prior_element_names,
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
                let ir_props = self.merge_props_full(inherited, &choice.props, &DfdlProps::default())?;
                let child_inherited =
                    particle_inherited_for_children(&ir_props, &choice.props, &self.defaults);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node = self.compile_particle(branch, &child_inherited, &[])?;
                    let name = branch_name(branch);
                    let initiator = branch_initiator(branch, &mut self.strings);
                    branches.push(ChoiceBranch {
                        name: self.strings.intern(&name),
                        initiator,
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

    fn compile_complex(&mut self, content: &ComplexContent, type_base: &IrProps) -> Result<u32> {
        match content {
            ComplexContent::Sequence(sequence) => {
                let ir_props = self.merge_props_full(
                    type_base,
                    &sequence.props,
                    &DfdlProps::default(),
                )?;
                let child_inherited =
                    particle_inherited_for_children(&ir_props, &sequence.props, &self.defaults);
                let mut children = Vec::new();
                let mut prior_element_names: Vec<String> = Vec::new();
                for particle in &sequence.particles {
                    validate_initiated_content_particle(&sequence.props, particle)?;
                    children.push(self.compile_particle(
                        particle,
                        &child_inherited,
                        &prior_element_names,
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
            ComplexContent::Choice(choice) => {
                let ir_props = self.merge_props_full(
                    type_base,
                    &choice.props,
                    &DfdlProps::default(),
                )?;
                let child_inherited =
                    particle_inherited_for_children(&ir_props, &choice.props, &self.defaults);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node = self.compile_particle(branch, &child_inherited, &[])?;
                    branches.push(ChoiceBranch {
                        name: self.strings.intern(branch_name(branch)),
                        initiator: branch_initiator(branch, &mut self.strings),
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
        validate_delimiter_props(type_props)?;
        validate_delimiter_props(element_props)?;
        validate_text_string_pad_props(type_props)?;
        validate_text_string_pad_props(element_props)?;
        let mut ir = merge_dfdl_props(base, type_props, element_props, &mut self.strings)?;
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
            SimpleBase::Builtin(_) => (None, None),
        };
        let mut prefix_props = merge_dfdl_props(
            &self.defaults.clone(),
            props,
            &DfdlProps::default(),
            &mut self.strings,
        )?;
        validate_prefix_length_type(type_name, props, &prefix_props)?;
        if prefix_props.length_kind == LengthKind::Prefixed && depth >= 1 {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error. Nested dfdl:lengthKind=\"prefixed\" not supported"
                    .into(),
            }
            .into());
        }
        self.attach_prefix_length(props, &DfdlProps::default(), &mut prefix_props, depth + 1)?;
        let kind = value_kind_from_simple(base);
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
    if bare.contains(';') {
        let bare_ref = bare.as_str();
        let (pos, neg) = bare_ref.split_once(';').unwrap_or((bare_ref, ""));
        if !neg.is_empty() && !pos.is_empty() {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: Negative patterns may not be used in textNumberPattern for textNumberRep='zoned'".into(),
            }
            .into());
        }
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
    strings: &StringPool,
    tunables: DaffodilTunables,
) -> Result<IrProps> {
    validate_binary_delimited(kind, &ir)?;
    validate_bcd_signed_integer_type(kind, &ir)?;
    validate_packed_binary_properties_schema(kind, &ir, strings)?;
    validate_prefixed_character_encoding(kind, &ir, strings)?;
    validate_end_of_parent(kind, &ir)?;
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
    }
    if matches!(ir.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        && ir.length.is_none()
        && ir.length_sibling.is_none()
        && ir.length_pattern.is_none()
        && !ir.length_expr_unparsed
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property length is not defined".into(),
        }
        .into());
    }
    if matches!(ir.length_kind, LengthKind::Explicit | LengthKind::Fixed) {
        if let Some(len) = ir.length {
            validate_float_double_bit_length(kind, len, ir.length_units)?;
            if ir.representation == Representation::Binary {
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
                validate_zoned_text_number_pattern(
                    kind,
                    pat,
                    ir.text_number_check_policy,
                    ir.decimal_signed,
                )?;
            }
            if pat.starts_with(';') {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: The positive part of the dfdl:textNumberPattern is required. The dfdl:textNumberPattern cannot begin with ';'.".into(),
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
    Ok(ir)
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

fn validate_delimiter_at_compile(prop: &str, raw: &str) -> Result<()> {
    if raw.trim_start().starts_with('{') {
        return Ok(());
    }
    if let Err(msg) = crate::schema::validate_delimiter_property_value(raw) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!("Schema Definition Error. {msg}"),
        }
        .into());
    }
    if let Err(msg) = crate::schema::validate_delimiter_es_restriction(prop, raw) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!("Schema Definition Error. {msg}"),
        }
        .into());
    }
    Ok(())
}

fn validate_delimiter_props(props: &DfdlProps) -> Result<()> {
    for (prop, s) in [
        ("initiator", props.initiator.as_deref()),
        ("separator", props.separator.as_deref()),
        ("terminator", props.terminator.as_deref()),
    ] {
        if let Some(v) = s {
            if !v.is_empty() {
                validate_delimiter_at_compile(prop, v)?;
            }
        }
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
    if crate::schema::is_zero_length_delimiter(initiator) {
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

fn validate_binary_delimited(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.representation == Representation::Binary
        && props.length_kind == LengthKind::Delimited
    {
        if matches!(kind, ValueKind::String | ValueKind::HexBinary) {
            return Ok(());
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
            message: "Schema Definition Error. lengthKind='delimited' only supported for packed binary formats.".into(),
        }
        .into());
    }
    Ok(())
}

fn validate_implicit_text_length(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.length_kind != LengthKind::Implicit {
        return Ok(());
    }
    if props.representation != Representation::Text {
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
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error. type {} representation text lengthKind implicit is not allowed",
            value_kind_type_name(kind)
        ),
    }
    .into())
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

fn branch_name(particle: &Particle) -> String {
    match particle {
        Particle::Element(e) => e.name.clone(),
        Particle::Sequence(_) => "sequence".to_string(),
        Particle::Choice(_) => "choice".to_string(),
    }
}

fn branch_initiator(particle: &Particle, strings: &mut StringPool) -> Option<StringId> {
    let raw = match particle {
        Particle::Element(e) => e.props.initiator.as_deref(),
        Particle::Sequence(s) => s.props.initiator.as_deref(),
        Particle::Choice(c) => c.props.initiator.as_deref(),
    };
    raw.map(|s| strings.intern(s))
}

fn value_kind_from_simple(base: &SimpleBase) -> ValueKind {
    match base {
        SimpleBase::Builtin(b) => value_kind_from_builtin(*b),
        SimpleBase::Restriction { base, .. } => value_kind_from_builtin(*base),
    }
}

fn restriction_min_length(base: &SimpleBase) -> Option<u64> {
    match base {
        SimpleBase::Restriction { min_length, .. } => *min_length,
        _ => None,
    }
}

fn apply_restriction_facets(props: &mut IrProps, base: &SimpleBase) {
    if let Some(min) = restriction_min_length(base) {
        props.min_length = Some(min);
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
    merged: &IrProps,
    group_props: &DfdlProps,
    defaults: &IrProps,
) -> IrProps {
    let mut inherited = merged.clone();
    if group_props.length_kind.is_none() {
        inherited.length_kind = defaults.length_kind;
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
    }
    if let Some(v) = props.bit_order {
        base.bit_order = v;
    }
    if let Some(v) = props.length_kind {
        base.length_kind = v;
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
        base.text_number_pad_character = props
            .text_number_pad_character
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.text_string_pad_character.is_some() {
        base.text_string_pad_character = props
            .text_string_pad_character
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.binary_number_rep {
        base.binary_number_rep = v;
    }
    if props.binary_packed_sign_codes.is_some() {
        base.binary_packed_sign_codes =
            strings.intern(props.binary_packed_sign_codes.as_deref().unwrap_or("C D F C"));
    }
    if let Some(v) = props.binary_number_check_policy {
        base.binary_number_check_policy = v;
    }
    if let Some(v) = props.binary_calendar_rep {
        base.binary_calendar_rep = v;
    }
    if let Some(v) = props.binary_float_rep {
        base.binary_float_rep = v;
    }
    if props.binary_decimal_virtual_point.is_some() {
        base.binary_decimal_virtual_point = props.binary_decimal_virtual_point.unwrap_or(0);
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
        if !s.is_empty() {
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
    if props.default_value.is_some() {
        base.default_value = props
            .default_value
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.sequence_kind {
        base.sequence_kind = v;
    }
    if let Some(v) = props.input_value_calc {
        base.input_value_calc = Some(v);
    }
    if props.input_value_calc_sibling.is_some() {
        base.input_value_calc_sibling = props
            .input_value_calc_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.output_value_calc {
        base.output_value_calc = Some(v);
    }
    if props.output_value_calc_sibling.is_some() {
        base.output_value_calc_sibling = props
            .output_value_calc_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
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
    if props.alignment.is_some() {
        base.alignment = props.alignment.unwrap_or(0);
    }
    if let Some(v) = props.alignment_units {
        base.alignment_units = v;
    }
    if let Some(ref bytes) = props.fill_byte {
        base.fill_byte = bytes.first().copied().unwrap_or(0);
    }
    if let Some(v) = props.prefix_includes_prefix_length {
        base.prefix_includes_prefix_length = v;
    }
    Ok(base)
}

fn merge_ir_props(base: &IrProps, overlay: &IrProps) -> IrProps {
    let mut out = base.clone();
    out.representation = overlay.representation;
    out.byte_order = overlay.byte_order;
    out.bit_order = overlay.bit_order;
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
    } else {
        out.length_kind = overlay.length_kind;
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
    out.length_units = overlay.length_units;
    out.encoding = overlay.encoding;
    out.encoding_error_policy = overlay.encoding_error_policy;
    out.nillable = overlay.nillable;
    out.nil_kind = overlay.nil_kind;
    out.nil_value = overlay.nil_value;
    out.separator_suppression_policy = overlay.separator_suppression_policy;
    out.ignore_case = overlay.ignore_case;
    out.text_trim_kind = overlay.text_trim_kind;
    out.text_pad_kind = overlay.text_pad_kind;
    out.text_number_pad_character = overlay.text_number_pad_character;
    out.text_string_pad_character = overlay.text_string_pad_character;
    out.binary_number_rep = overlay.binary_number_rep;
    out.binary_packed_sign_codes = overlay.binary_packed_sign_codes;
    out.binary_number_check_policy = overlay.binary_number_check_policy;
    out.binary_calendar_rep = overlay.binary_calendar_rep;
    out.binary_float_rep = overlay.binary_float_rep;
    out.binary_decimal_virtual_point = overlay.binary_decimal_virtual_point;
    out.decimal_signed = overlay.decimal_signed;
    out.calendar_pattern = overlay.calendar_pattern;
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
    if overlay.default_value.is_some() {
        out.default_value = overlay.default_value;
    }
    out.sequence_kind = overlay.sequence_kind;
    out.occurs_min = overlay.occurs_min;
    out.occurs_max = overlay.occurs_max;
    if overlay.alignment != 0 {
        out.alignment = overlay.alignment;
    }
    out.alignment_units = overlay.alignment_units;
    if overlay.fill_byte != 0 {
        out.fill_byte = overlay.fill_byte;
    }
    out.input_value_calc = overlay.input_value_calc;
    out.input_value_calc_sibling = overlay.input_value_calc_sibling;
    out.output_value_calc = overlay.output_value_calc;
    out.output_value_calc_sibling = overlay.output_value_calc_sibling;
    out.text_string_justification = overlay.text_string_justification;
    out.text_number_justification = overlay.text_number_justification;
    out.text_standard_base = overlay.text_standard_base;
    out.truncate_specified_length_string = overlay.truncate_specified_length_string;
    if overlay.min_length.is_some() {
        out.min_length = overlay.min_length;
    }
    if overlay.prefix_length.is_some() {
        out.prefix_length = overlay.prefix_length.clone();
    }
    out.prefix_includes_prefix_length = overlay.prefix_includes_prefix_length;
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

    IrBuilder::new(schema, tunables)?.build(&root_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::parse_schema;

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
}
