use super::super::{IrNode, IrProps, StringPool, ValueKind};
use super::props::{
    apply_type_name_ir_flags, element_props_for_complex_content, finalize_element_props,
    overlay_dfdl_to_ir,
};
use super::validate::validate_binary_calendar_compile;
use super::IrBuilder;
use crate::error::{Result, SchemaError};
use crate::schema::{
    validate_length_facets_for_type, BinaryNumberRep, BuiltinType, DfdlProps, GlobalElement, LengthKind,
    Representation, SchemaDocument, SimpleBase, TypeDef, TypeName,
};
use alloc::string::ToString;

impl<'a> IrBuilder<'a> {
    pub(crate) fn build_root_element_node(
        &mut self,
        root_name: &str,
        root_element: &GlobalElement,
    ) -> Result<u32> {
        let root = if let Some(builtin) =
            builtin_for_element_type_name(self.schema, &root_element.type_name)
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
                self.schema,
                &mut props,
                &SimpleBase::Builtin(builtin),
                &mut self.strings,
                &root_element.props,
            );
            validate_binary_calendar_compile(kind, &props, &self.strings)?;
            validate_implicit_text_length(kind, &props, Some(root_name), Some(&self.strings))?;
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

            if let TypeDef::Simple { base, .. } = type_def {
                let defaults = self.defaults.clone();
                let kind = value_kind_from_simple(self.schema, base);
                let type_props = self
                    .schema
                    .effective_simple_type_props(&root_element.type_name)
                    .unwrap_or_default();
                validate_dfdl_prop_overlap(&root_element.props, &type_props)?;
                let mut merged =
                    self.merge_props_full(&defaults, &type_props, &root_element.props)?;
                apply_type_name_ir_flags(&root_element.type_name, &mut merged);
                validate_length_facets_for_type(self.schema, base, kind, &merged, None)?;
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
                    self.schema,
                    &mut ir_props,
                    base,
                    &mut self.strings,
                    &root_element.props,
                );
                validate_binary_calendar_compile(kind, &ir_props, &self.strings)?;
                validate_implicit_text_length(
                    kind,
                    &ir_props,
                    Some(root_name),
                    Some(&self.strings),
                )?;
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
                let mut ir_props =
                    self.merge_props_full(&defaults, &DfdlProps::default(), &root_element.props)?;
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

    pub(crate) fn compile_type(
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

        let type_def =
            self.schema
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
                let kind = value_kind_from_simple(self.schema, base);
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
                validate_length_kind_defined(kind, &ir_props, self.schema, Some(type_name))?;
                apply_restriction_facets(
                    self.schema,
                    &mut ir_props,
                    base,
                    &mut self.strings,
                    element_props,
                );
                validate_binary_calendar_compile(kind, &ir_props, &self.strings)?;
                validate_length_facets_for_type(
                    self.schema,
                    base,
                    kind,
                    &ir_props,
                    facet_diagnostic,
                )?;
                validate_implicit_text_length(kind, &ir_props, None, Some(&self.strings))?;
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
}

pub(crate) fn builtin_for_element_type_name(
    schema: &SchemaDocument,
    type_name: &TypeName,
) -> Option<BuiltinType> {
    if schema.types.contains_key(type_name) {
        return None;
    }
    BuiltinType::from_xsd(type_name.as_str())
}

pub(crate) fn value_kind_from_builtin(builtin: BuiltinType) -> ValueKind {
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

pub(crate) fn value_kind_from_simple(schema: &SchemaDocument, base: &SimpleBase) -> ValueKind {
    schema
        .builtin_for_simple_base(base)
        .map(value_kind_from_builtin)
        .unwrap_or(ValueKind::String)
}

pub(crate) fn validate_dfdl_prop_overlap(
    element: &DfdlProps,
    type_props: &DfdlProps,
) -> Result<()> {
    macro_rules! check_overlap {
        ($field:ident, $name:expr) => {
            if element.$field.is_some()
                && type_props.$field.is_some()
                && element.$field != type_props.$field
            {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error. Property overlap {}", $name),
                }
                .into());
            }
        };
    }
    check_overlap!(byte_order, "byteOrder");
    check_overlap!(bit_order, "bitOrder");
    check_overlap!(encoding, "encoding");
    check_overlap!(length_kind, "lengthKind");
    check_overlap!(length_units, "lengthUnits");
    check_overlap!(text_number_pad_character, "textNumberPadCharacter");
    check_overlap!(text_string_pad_character, "textStringPadCharacter");
    check_overlap!(text_number_rep, "textNumberRep");
    check_overlap!(binary_number_rep, "binaryNumberRep");
    check_overlap!(representation, "representation");
    check_overlap!(calendar_pattern_kind, "calendarPatternKind");
    Ok(())
}

pub(crate) fn apply_restriction_facets(
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
        props.facet_assert_message_segments = Some(super::props::intern_input_value_calc_segments(
            segs, strings,
        ));
    }
    if let Some(n) = element_props.assert_int_eq {
        props.assert_int_eq = Some(n);
    }
    if let Some(n) = element_props.assert_eq_occurs_index_addend {
        props.assert_eq_occurs_index_addend = Some(n);
    }
    if let SimpleBase::Restriction {
        base, enumerations, ..
    } = base
    {
        if !enumerations.is_empty() {
            props.facet_assert_daffodil_prefix = true;
        }
        match base {
            crate::schema::RestrictionBase::Named(_) => {
                props.facet_assert_daffodil_prefix = true;
            }
            crate::schema::RestrictionBase::Builtin(b)
                if *b != crate::schema::BuiltinType::String =>
            {
                props.facet_assert_daffodil_prefix = true;
            }
            _ => {}
        }
    }
}

pub(crate) fn validate_implicit_text_length(
    kind: ValueKind,
    props: &IrProps,
    _element_name: Option<&str>,
    strings: Option<&StringPool>,
) -> Result<()> {
    if crate::ir::ir_props_has_input_value_calc(props)
        || crate::ir::ir_props_has_output_value_calc(props)
    {
        return Ok(());
    }
    if props.length_kind == LengthKind::Explicit
        && props.length.is_none()
        && props.length_sibling.is_none()
        && !props.length_expr_unparsed
        && !props.length_self_value_length
        && props.length_pattern.is_none()
        && props.prefix_length.is_none()
        && props.facet_length.is_none()
        && props.implicit_facet_length.is_none()
        && props.min_length.is_none()
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: dfdl:lengthKind is 'explicit' but no dfdl:length or length facet is specified.".into(),
        }
        .into());
    }
    crate::length_validate::validate_binary_decimal_virtual_point_schema(kind, props)?;
    if props.representation == Representation::Text
        && props.length_kind == LengthKind::Implicit
        && kind != ValueKind::Complex
        && props.length.is_none()
        && props.facet_length.is_none()
        && props.implicit_facet_length.is_none()
        && props.min_length.is_none()
        && props.max_length.is_none()
    {
        let type_name = if props.non_negative_integer {
            "nonNegativeInteger"
        } else {
            match kind {
                ValueKind::Int => "Int",
                ValueKind::Integer => "Integer",
                _ => &alloc::format!("{kind:?}"),
            }
        };
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!("Schema Definition Error: Type '{type_name}' with dfdl:representation='text' and dfdl:lengthKind='implicit' requires an explicit length facet or dfdl:length property."),
        }
        .into());
    }
    if props.representation == Representation::Binary
        && props.length_kind == LengthKind::Implicit
        && (kind == ValueKind::Integer || kind == ValueKind::Decimal || props.non_negative_integer)
        && props.length.is_none()
        && props.facet_length.is_none()
        && props.implicit_facet_length.is_none()
        && props.min_length.is_none()
    {
        let type_name = if props.non_negative_integer {
            "nonNegativeInteger"
        } else {
            match kind {
                ValueKind::Integer => "integer",
                ValueKind::Decimal => "decimal",
                _ => &alloc::format!("{kind:?}"),
            }
        };
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: Length of binary data '{type_name}' cannot be determined implicitly."
            ),
        }
        .into());
    }
    if props.representation == Representation::Binary
        && props.length_kind == LengthKind::Delimited
        && kind != ValueKind::HexBinary
        && kind != ValueKind::String
        && kind != ValueKind::Complex
        && !matches!(
            props.binary_number_rep,
            BinaryNumberRep::Bcd | BinaryNumberRep::PackedBcd | BinaryNumberRep::Ibm4690Packed
        )
        && !matches!(
            props.binary_calendar_rep,
            BinaryNumberRep::Bcd | BinaryNumberRep::PackedBcd | BinaryNumberRep::Ibm4690Packed
        )
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: lengthKind='delimited' only supported for packed binary formats.".into(),
        }
        .into());
    }
    if kind == ValueKind::HexBinary && props.length_kind == LengthKind::Delimited {
        if let Some(strs) = strings {
            if let Ok(enc) = strs.get(props.encoding) {
                if crate::vm::encoding::normalize_encoding_name(enc) != Some("iso-8859-1") {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: xs:hexBinary with dfdl:lengthKind=\"delimited\" must have dfdl:encoding=\"ISO-8859-1\", got '{enc}'"
                        ),
                    }
                    .into());
                }
            }
        }
    }
    super::validate::validate_bit_order_byte_order(kind, props)?;
    if props.representation == Representation::Binary && props.length_kind == LengthKind::Explicit {
        if (kind == ValueKind::Float || kind == ValueKind::Double) && props.length_expr_unparsed {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: floating point binary numbers may not have runtime-specified lengths".into(),
            }
            .into());
        }
        if let Some(len) = props.length {
            let bits = if props.length_units == crate::schema::LengthUnits::Bytes {
                len.saturating_mul(8)
            } else {
                len
            };
            if kind == ValueKind::Float && bits != 32 {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: binary xs:float must be 32 bits. Length in bits was {bits}"
                    ),
                }
                .into());
            }
            if kind == ValueKind::Double && bits != 64 {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: binary xs:double must be 64 bits. Length in bits was {bits}"
                    ),
                }
                .into());
            }
            if matches!(kind, ValueKind::Time | ValueKind::DateTime) {
                let xsd = match kind {
                    ValueKind::Time => "time",
                    ValueKind::DateTime => "dateTime",
                    _ => "dateTime",
                };
                if props.binary_calendar_rep == BinaryNumberRep::BinarySeconds && bits != 32 {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: binary xs:{xsd} must be 32 bits when binaryCalendarRep='binarySeconds'"
                        ),
                    }
                    .into());
                }
                if props.binary_calendar_rep == BinaryNumberRep::BinaryMilliseconds && bits != 64 {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: binary xs:{xsd} must be 64 bits when binaryCalendarRep='binaryMilliseconds'"
                        ),
                    }
                    .into());
                }
                if matches!(
                    props.binary_calendar_rep,
                    BinaryNumberRep::Bcd | BinaryNumberRep::PackedBcd | BinaryNumberRep::Ibm4690Packed
                ) && bits % 4 != 0
                {
                    let rep_str = match props.binary_calendar_rep {
                        BinaryNumberRep::Bcd => "bcd",
                        BinaryNumberRep::PackedBcd => "packed",
                        BinaryNumberRep::Ibm4690Packed => "ibm4690Packed",
                        _ => "bcd",
                    };
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: The given length ({bits} bits) must be a multiple of 4 when using binaryCalendarRep='{rep_str}'"
                        ),
                    }
                    .into());
                }
            }
        }
    }
    if kind == ValueKind::Boolean {
        if props.representation == Representation::Binary {
            if !props.binary_boolean_true_rep_defined {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property binaryBooleanTrueRep is not defined.".into(),
                }
                .into());
            }
            if !props.binary_boolean_false_rep_defined {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property binaryBooleanFalseRep is not defined.".into(),
                }
                .into());
            }
        } else if props.representation == Representation::Text {
            if !props.text_boolean_true_rep_defined {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property textBooleanTrueRep is not defined.".into(),
                }
                .into());
            }
            if !props.text_boolean_false_rep_defined {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property textBooleanFalseRep is not defined.".into(),
                }
                .into());
            }
            if let Some(strs) = strings {
                super::validate::validate_text_boolean_reps(props, strs)?;
            }
        }
    }
    Ok(())
}

fn validate_length_kind_defined(
    _kind: ValueKind,
    props: &IrProps,
    _schema: &SchemaDocument,
    _type_name: Option<&TypeName>,
) -> Result<()> {
    if crate::ir::ir_props_has_input_value_calc(props)
        || crate::ir::ir_props_has_output_value_calc(props)
    {
        return Ok(());
    }
    if !props.length_kind_defined {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property lengthKind is not defined.".into(),
        }
        .into());
    }
    Ok(())
}
