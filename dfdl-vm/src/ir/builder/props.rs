use super::super::{IrProps, StringId, StringPool, ValueKind};
use crate::error::{Result, SchemaError};
use crate::length_validate::{validate_fill_byte_schema, DaffodilTunables};
use crate::schema::{
    expand_entities_str, get_global_element, validate_text_standard_exponent_rep_literal,
    validate_text_standard_separator_literal, validate_text_standard_special_value_literal,
    validate_text_standard_zero_rep_literal, ByteOrder, DfdlProps, ElementDecl, LengthKind,
    LengthUnits, NilKind, ObjectKind, OccursCountKind, Representation, SchemaDocument, TextPadKind,
    TextTrimKind, TypeName,
};
use alloc::string::ToString;

pub(crate) fn apply_format_default_delimiters(ir: &mut IrProps, defaults: &IrProps) {
    if ir.initiator.is_none() {
        ir.initiator = defaults.initiator;
    }
    if ir.terminator.is_none() {
        ir.terminator = defaults.terminator;
    }
}

pub(crate) fn effective_element_type_name<'a>(
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

pub(crate) fn dfdl_props_for_element_ref(
    schema: &SchemaDocument,
    element: &ElementDecl,
) -> DfdlProps {
    let mut props = element.props.clone();
    if let Some(ref er) = element.element_ref {
        if let Some(g) = get_global_element(schema, er) {
            let imported = g.props.initiator.is_some() || g.format_context.initiator.is_some();
            props = crate::schema::merge_dfdl_props(g.props.clone(), props);
            if imported && props.initiator.is_none() {
                props.initiator = g.format_context.initiator.clone().filter(|s| !s.is_empty());
            }
        }
    }
    props
}

pub(crate) fn merge_dfdl_props(
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

fn calendar_first_day_of_week_iso(raw: &str) -> u32 {
    match raw.trim().to_ascii_lowercase().as_str() {
        "monday" => 1,
        "tuesday" => 2,
        "wednesday" => 3,
        "thursday" => 4,
        "friday" => 5,
        "saturday" => 6,
        "sunday" => 7,
        _ => 1,
    }
}

pub(crate) fn overlay_dfdl_to_ir(
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
    if let Some(ref test) = props.byte_order_conditional_test {
        base.byte_order_conditional_test = Some(strings.intern(test.clone()));
        base.byte_order_if_true = props.byte_order_if_true.unwrap_or(ByteOrder::BigEndian);
        base.byte_order_if_false = props.byte_order_if_false.unwrap_or(ByteOrder::LittleEndian);
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
    if props.length_sibling_adjust != 0 {
        base.length_sibling_adjust = props.length_sibling_adjust;
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
        base.nil_value = props.nil_value.as_ref().map(|s| strings.intern(s.clone()));
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
    if let Some(raw) = props.text_number_pad_character.as_deref() {
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
        base.binary_packed_sign_codes = strings.intern(
            props
                .binary_packed_sign_codes
                .as_deref()
                .unwrap_or("C D F C"),
        );
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
        base.custom_text_number_pattern = true;
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
        let raw = props
            .text_number_rounding_increment
            .as_deref()
            .unwrap_or("0");
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
        let raw = props
            .text_standard_grouping_separator
            .as_deref()
            .unwrap_or(",");
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
        let raw = props.text_standard_infinity_rep.as_deref().unwrap_or("Inf");
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
    if let Some(ref s) = props.output_new_line_sibling {
        base.output_new_line_sibling = Some(strings.intern(s.clone()));
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
        base.set_variables
            .push((strings.intern(name.clone()), strings.intern(val.clone())));
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
    if props.output_value_calc_scale.is_some() {
        base.output_value_calc_scale = props.output_value_calc_scale;
    }
    if let Some(segments) = &props.output_value_calc_segments {
        base.output_value_calc_segments = Some(intern_input_value_calc_segments(segments, strings));
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
            let encoding = strings.get(base.encoding).unwrap_or("ISO-8859-1");
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

pub(crate) fn merge_ir_props(base: &IrProps, overlay: &IrProps) -> IrProps {
    let mut out = base.clone();
    out.representation = overlay.representation;
    if overlay.byte_order_defined {
        out.byte_order = overlay.byte_order;
        out.byte_order_defined = true;
    }
    if overlay.byte_order_conditional_test.is_some() {
        out.byte_order_conditional_test = overlay.byte_order_conditional_test;
        out.byte_order_if_true = overlay.byte_order_if_true;
        out.byte_order_if_false = overlay.byte_order_if_false;
    }
    if overlay.bit_order_defined {
        out.bit_order = overlay.bit_order;
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
    if overlay.length_sibling_adjust != 0 {
        out.length_sibling_adjust = overlay.length_sibling_adjust;
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
    out.text_number_pad_character_property_form = overlay.text_number_pad_character_property_form;
    out.text_string_pad_character = overlay.text_string_pad_character;
    out.text_string_pad_character_property_form = overlay.text_string_pad_character_property_form;
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
        out.text_standard_decimal_separator_sibling =
            overlay.text_standard_decimal_separator_sibling;
    }
    out.text_standard_grouping_separator = overlay.text_standard_grouping_separator;
    if overlay.text_standard_grouping_separator_defined {
        out.text_standard_grouping_separator_defined = true;
    }
    if overlay.text_standard_grouping_separator_sibling.is_some() {
        out.text_standard_grouping_separator_sibling =
            overlay.text_standard_grouping_separator_sibling;
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
    if overlay.output_new_line_sibling.is_some() {
        out.output_new_line_sibling = overlay.output_new_line_sibling;
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
            if out.alignment != 0 && !out.alignment_implicit && overlay.alignment != out.alignment {
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
    if overlay.output_value_calc_scale.is_some() {
        out.output_value_calc_scale = overlay.output_value_calc_scale;
    }
    if overlay.output_value_calc_segments.is_some() {
        out.output_value_calc_segments = overlay.output_value_calc_segments.clone();
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

pub(crate) fn resolve_escape_scheme(
    schema: &SchemaDocument,
    type_props: &DfdlProps,
    element_props: &DfdlProps,
    ir: &mut IrProps,
) {
    let ref_name = element_props
        .escape_scheme_ref
        .as_ref()
        .or(type_props.escape_scheme_ref.as_ref())
        .or(schema.format_defaults.props.escape_scheme_ref.as_ref());
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

pub(crate) fn apply_unsigned_long_flag(type_name: &TypeName, props: &mut IrProps) {
    let local = type_name
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or(type_name.as_str());
    if matches!(local, "unsignedLong") {
        props.unsigned_integer = true;
    }
}

pub(crate) fn apply_integer_type_flags(type_name: &TypeName, props: &mut IrProps) {
    let local = type_name
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or(type_name.as_str());
    if matches!(local, "nonNegativeInteger") {
        props.non_negative_integer = true;
    }
}

pub(crate) fn apply_calendar_type_flags(type_name: &TypeName, props: &mut IrProps) {
    let local = type_name
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or(type_name.as_str());
    if matches!(local, "date") {
        props.calendar_date_only = true;
    }
}

pub(crate) fn apply_type_name_ir_flags(type_name: &TypeName, props: &mut IrProps) {
    apply_unsigned_long_flag(type_name, props);
    apply_integer_type_flags(type_name, props);
    apply_calendar_type_flags(type_name, props);
}

pub(crate) fn intern_input_path_steps(
    steps: &[(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
        bool,
    )],
    strings: &mut StringPool,
) -> alloc::vec::Vec<crate::ir::IrInputPathStep> {
    steps
        .iter()
        .map(
            |(prefix, local, index, index_from_occurs)| crate::ir::IrInputPathStep {
                prefix: prefix.as_ref().map(|p| strings.intern(p.clone())),
                local: strings.intern(local.clone()),
                index: *index,
                index_from_occurs: *index_from_occurs,
            },
        )
        .collect()
}

pub fn intern_input_value_calc_expression(
    expr: &crate::schema::InputValueCalcExpression,
    strings: &mut StringPool,
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
        InputValueCalcExpression::Sub(items) => IrInputValueCalcExpression::Sub(
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
        InputValueCalcExpression::Path { parent_root, steps } => IrInputValueCalcExpression::Path {
            parent_root: *parent_root,
            steps: intern_input_path_steps(steps, strings),
        },
        InputValueCalcExpression::StringOf(inner) => IrInputValueCalcExpression::StringOf(
            alloc::boxed::Box::new(intern_input_value_calc_expression(inner, strings)),
        ),
        InputValueCalcExpression::Literal(v) => IrInputValueCalcExpression::Literal(*v),
        InputValueCalcExpression::LiteralLexical(text) => {
            IrInputValueCalcExpression::LiteralLexical(strings.intern(text.clone()))
        }
        InputValueCalcExpression::Cast { kind, inner } => IrInputValueCalcExpression::Cast {
            kind: match kind {
                crate::schema::IvcXsCast::Byte => crate::ir::IrIvcXsCast::Byte,
                crate::schema::IvcXsCast::Short => crate::ir::IrIvcXsCast::Short,
                crate::schema::IvcXsCast::Int => crate::ir::IrIvcXsCast::Int,
                crate::schema::IvcXsCast::Long => crate::ir::IrIvcXsCast::Long,
                crate::schema::IvcXsCast::UnsignedByte => crate::ir::IrIvcXsCast::UnsignedByte,
                crate::schema::IvcXsCast::UnsignedShort => crate::ir::IrIvcXsCast::UnsignedShort,
                crate::schema::IvcXsCast::UnsignedInt => crate::ir::IrIvcXsCast::UnsignedInt,
                crate::schema::IvcXsCast::UnsignedLong => crate::ir::IrIvcXsCast::UnsignedLong,
                crate::schema::IvcXsCast::Float => crate::ir::IrIvcXsCast::Float,
                crate::schema::IvcXsCast::Double => crate::ir::IrIvcXsCast::Double,
                crate::schema::IvcXsCast::String => crate::ir::IrIvcXsCast::String,
                crate::schema::IvcXsCast::HexBinary => crate::ir::IrIvcXsCast::HexBinary,
            },
            inner: alloc::boxed::Box::new(intern_input_value_calc_expression(inner, strings)),
        },
        InputValueCalcExpression::Div(left, right) => IrInputValueCalcExpression::Div(
            alloc::boxed::Box::new(intern_input_value_calc_expression(left, strings)),
            alloc::boxed::Box::new(intern_input_value_calc_expression(right, strings)),
        ),
        InputValueCalcExpression::Ceiling(inner) => IrInputValueCalcExpression::Ceiling(
            alloc::boxed::Box::new(intern_input_value_calc_expression(inner, strings)),
        ),
        InputValueCalcExpression::Variable(name) => {
            IrInputValueCalcExpression::Variable(strings.intern(name.clone()))
        }
        InputValueCalcExpression::ValueLength { sibling, units } => {
            IrInputValueCalcExpression::ValueLength {
                sibling: strings.intern(sibling.clone()),
                units: *units,
            }
        }
    }
}

pub(crate) fn intern_input_value_calc_segments(
    segments: &[crate::schema::InputValueCalcSegment],
    strings: &mut StringPool,
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
            crate::schema::InputValueCalcSegment::ValueLength { sibling, units } => {
                crate::ir::IrInputValueCalcSegment::ValueLength {
                    sibling: strings.intern(sibling.clone()),
                    units: *units,
                }
            }
        })
        .collect()
}

pub(crate) fn particle_inherited_for_children(inherited: &IrProps) -> IrProps {
    let mut out = inherited.clone();
    out.initiator = None;
    out.terminator = None;
    out.separator = None;
    out
}

pub(crate) fn element_props_for_simple_type_compile(element: &DfdlProps) -> DfdlProps {
    let mut res = element.clone();
    res.separator = None;
    res.separator_position = None;
    res.separator_suppression_policy = None;
    res.sequence_kind = None;
    res.choice_length_kind = None;
    res.choice_length = None;
    res
}

pub(crate) fn finalize_element_props(
    kind: ValueKind,
    mut ir: IrProps,
    strings: &mut StringPool,
    tunables: DaffodilTunables,
    schema: Option<&SchemaDocument>,
    element_name: Option<&str>,
) -> Result<IrProps> {
    if kind == ValueKind::Complex
        && (ir.output_value_calc.is_some() || ir.output_value_calc_conditional)
    {
        return Err(SchemaError::InvalidProperty {
            message:
                "Schema Definition Error. dfdl:outputValueCalc cannot be defined on complexType elements."
                    .into(),
        }
        .into());
    }
    if ir.length_kind == LengthKind::EndOfParent {
        let type_kind_str = if kind == ValueKind::Complex { "complex type" } else { "simple type" };
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: lengthKind='endOfParent' is not implemented for {type_kind_str}"
            ),
        }
        .into());
    }
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
            message:
                "Schema Definition Error: objectKind='bytes' must have dfdl:lengthKind='explicit'"
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
            let raw = strings
                .get(nil_id)
                .map_err(|e| SchemaError::InvalidProperty {
                    message: e.to_string(),
                })?;
            if let Err(msg) =
                crate::schema::validate_nil_value_compile(raw, ir.nil_kind, ir.representation)
            {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error: {msg}"),
                }
                .into());
            }
        }
    }
    if let Some(ref scheme) = ir.escape_scheme {
        if let Some(raw) = scheme.escape_block_start_raw.as_deref() {
            if let Err(msg) = crate::schema::validate_escape_block_property(raw) {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error. {msg}"),
                }
                .into());
            }
        }
        if let Some(raw) = scheme.escape_block_end_raw.as_deref() {
            if let Err(msg) = crate::schema::validate_escape_block_property(raw) {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error. {msg}"),
                }
                .into());
            }
        }
        if let Some(raw) = scheme.escape_character_raw.as_deref() {
            if let Err(msg) = crate::schema::validate_escape_character_property(raw) {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error. {msg}"),
                }
                .into());
            }
        }
        if let Some(raw) = scheme.escape_escape_character_raw.as_deref() {
            if let Err(msg) = crate::schema::validate_escape_character_property(raw) {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error. {msg}"),
                }
                .into());
            }
        }
        let p_esc = scheme
            .escape_character
            .as_deref()
            .or(scheme.escape_escape_character.as_deref());
        let p_sep = ir.separator.and_then(|id| strings.get(id).ok());
        if let (Some(esc), Some(sep)) = (p_esc, p_sep) {
            if !esc.is_empty() && esc == sep {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: escapeCharacter and separator cannot be identical (`{esc}`)"
                    ),
                }
                .into());
            }
        }
    }
    if ir.length_kind == LengthKind::Explicit && ir.length.is_none() && !ir.length_expr_unparsed {
        if let Some(s) = schema {
            if let Some(name) = element_name {
                if let Some(el) = get_global_element(s, name) {
                    if el.props.length.is_some() {
                        ir.length = el.props.length;
                    }
                }
            }
        }
    }
    let has_ivc = ir.input_value_calc.is_some()
        || ir.input_value_calc_sibling.is_some()
        || ir.input_value_calc_segments.is_some()
        || ir.input_value_calc_path.is_some()
        || ir.input_value_calc_expression.is_some();
    if ir.length_kind == LengthKind::Explicit
        && ir.length.is_none()
        && !ir.length_expr_unparsed
        && ir.length_sibling.is_none()
        && !has_ivc
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property length is not defined".into(),
        }
        .into());
    }
    if ir.length_kind == LengthKind::Implicit
        && ir.representation == Representation::Text
        && ir.length.is_some()
        && kind != ValueKind::String
        && kind != ValueKind::HexBinary
        && kind != ValueKind::Complex
    {
        let kind_str = match kind {
            ValueKind::Int => "int",
            ValueKind::Long => "long",
            ValueKind::Short => "short",
            ValueKind::Byte => "byte",
            ValueKind::Integer => "integer",
            ValueKind::Decimal => "decimal",
            ValueKind::Boolean => "boolean",
            ValueKind::Float => "float",
            ValueKind::Double => "double",
            ValueKind::Time => "time",
            ValueKind::DateTime => "dateTime",
            _ => "simple",
        };
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: type {kind_str} with representation='text' cannot specify dfdl:length when lengthKind='implicit'"
            ),
        }
        .into());
    }
    if ir.length_kind == LengthKind::Implicit && ir.length.is_none() {
        if let Some(len) = ir
            .facet_length
            .or(ir.implicit_facet_length)
            .or(ir.min_length)
        {
            ir.length = Some(len);
        }
    }
    if (kind == ValueKind::Float || kind == ValueKind::Double)
        && ir.representation == Representation::Binary
        && ir.length_expr_unparsed
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: floating point binary numbers may not have runtime-specified lengths".into(),
        }
        .into());
    }
    if (ir.length_kind == LengthKind::Explicit || ir.length_kind == LengthKind::Fixed)
        && ir.representation == Representation::Binary
    {
        if let Some(len) = ir.length {
            crate::length_validate::validate_data_length_schema(
                kind,
                len,
                ir.length_units,
                ir.binary_number_rep,
            )?;
            crate::length_validate::validate_signed_one_bit_length_schema(
                kind,
                len,
                ir.length_units,
                &tunables,
            )?;
        }
    }
    if ir.representation == Representation::Text && ir.text_trim_kind == TextTrimKind::PadChar
        || ir.text_pad_kind == TextPadKind::PadChar
    {
        let pad_id = match kind {
            ValueKind::String => ir.text_string_pad_character,
            ValueKind::Boolean => ir.text_boolean_pad_character,
            _ => ir.text_number_pad_character,
        };
        let pad = pad_id.and_then(|id| strings.get(id).ok()).unwrap_or(" ");
        let expanded = crate::schema::expand_entities_str(pad);
        if expanded.chars().count() != 1 {
            return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: pad character must be single character, got `{expanded}`"
                    ),
                }
                .into());
        }
    }
    if schema.is_some() {
        super::validate::validate_delimiter_props(&DfdlProps::default(), &DfdlProps::default())?;
    }
    Ok(ir)
}

pub(crate) fn element_props_for_complex_content(element_props: &DfdlProps) -> DfdlProps {
    DfdlProps {
        separator: element_props.separator.clone(),
        separator_position: element_props.separator_position,
        separator_suppression_policy: element_props.separator_suppression_policy,
        sequence_kind: element_props.sequence_kind,
        choice_length_kind: element_props.choice_length_kind,
        choice_length: element_props.choice_length,
        choice_dispatch_sibling: element_props.choice_dispatch_sibling.clone(),
        choice_dispatch_path: element_props.choice_dispatch_path.clone(),
        choice_dispatch_literal: element_props.choice_dispatch_literal.clone(),
        choice_dispatch_sibling_int: element_props.choice_dispatch_sibling_int.clone(),
        ..DfdlProps::default()
    }
}
