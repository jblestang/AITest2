use super::super::{IrProps, StringId, StringPool, ValueKind};
use super::props::dfdl_props_for_element_ref;
use crate::error::{Result, SchemaError};
use crate::length_validate::validate_float_double_bit_length_schema;
use crate::schema::{
    BinaryNumberRep, DfdlProps, ElementDecl, LengthKind, LengthUnits, OccursCountKind, Particle,
    SchemaDocument, TypeName,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn validate_prefixed_character_encoding(
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

pub(crate) fn validate_float_double_bit_length(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
) -> Result<()> {
    validate_float_double_bit_length_schema(kind, length, units).map_err(Into::into)
}

pub(crate) fn text_number_pattern_requires_grouping_separator(pattern: &str) -> bool {
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

pub(crate) fn text_number_pattern_bare(pattern: &str) -> String {
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

pub(crate) fn text_number_pattern_has_grouping_and_exponent(pattern: &str) -> bool {
    text_number_pattern_requires_grouping_separator(pattern)
        && text_number_pattern_requires_exponent(pattern)
}

pub(crate) fn validate_text_number_pattern_unquoted_special(pattern: &str) -> Result<()> {
    if pattern.starts_with(';') {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: textNumberPattern positive part is mandatory".into(),
        }
        .into());
    }
    if text_number_pattern_has_grouping_and_exponent(pattern) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: Invalid textNumberPattern `{pattern}`. Cannot have grouping separator in scientific notation."
            ),
        }
        .into());
    }
    for sub in pattern.split(';') {
        let bare = text_number_pattern_bare(sub);
        let chars: Vec<char> = bare.chars().collect();
        let mut pad_spec_count = 0;
        let mut p_idx = 0;
        while p_idx < chars.len() {
            if chars[p_idx] == '*' {
                pad_spec_count += 1;
                p_idx += 2;
            } else {
                p_idx += 1;
            }
        }
        if pad_spec_count > 1 {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: Invalid textNumberPattern `{pattern}` - multiple pad specifiers"
                ),
            }
            .into());
        }
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

pub(crate) fn text_standard_distinct_entries(
    ir: &IrProps,
    strings: &StringPool,
) -> Result<Vec<(&'static str, alloc::string::String)>> {
    let mut entries = Vec::new();
    if ir.text_standard_decimal_separator_defined
        && ir.text_standard_decimal_separator_sibling.is_none()
    {
        let raw = strings
            .get(ir.text_standard_decimal_separator)
            .map_err(|e| SchemaError::InvalidProperty {
                message: e.to_string(),
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

pub(crate) fn count_pad_specifiers(bare: &str) -> Result<usize> {
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

pub(crate) fn validate_text_number_pad_specifiers(subpattern: &str) -> Result<()> {
    let bare = text_number_pattern_bare(subpattern);
    let count = count_pad_specifiers(&bare)?;
    if count > 1 {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: Invalid textNumberPattern: Malformed pattern \"{bare}\""
            ),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn validate_zoned_text_number_pattern(
    pattern: &str,
    rep: Option<crate::schema::TextNumberRep>,
) -> Result<()> {
    if rep != Some(crate::schema::TextNumberRep::Zoned) {
        return Ok(());
    }
    if pattern.is_empty() {
        return Ok(());
    }
    let bare = text_number_pattern_bare(pattern);
    let mut digit_count = 0usize;
    let mut v_seen = false;
    let mut p_seen = false;
    for c in bare.chars() {
        match c {
            '0' | '#' => digit_count += 1,
            'V' | 'v' => {
                if v_seen {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: Invalid textNumberPattern for zoned number `{pattern}`"
                        ),
                    }
                    .into());
                }
                v_seen = true;
            }
            'P' | 'p' => p_seen = true,
            '+' | '-' => {}
            _ => {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: Invalid textNumberPattern for zoned number `{pattern}`"
                    ),
                }
                .into());
            }
        }
    }
    if digit_count == 0 && (v_seen || p_seen) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: Invalid textNumberPattern for zoned number `{pattern}`"
            ),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn validate_text_standard_sibling_order(
    ir: &IrProps,
    prior_names: &[String],
    strings: &StringPool,
) -> Result<()> {
    let check = |sib: Option<StringId>, label: &str| -> Result<()> {
        let Some(id) = sib else {
            return Ok(());
        };
        let target = strings.get(id).unwrap_or("");
        if !prior_names.iter().any(|n| n == target) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: {label} refers to `{target}` which does not exist as a prior sibling element."
                ),
            }
            .into());
        }
        Ok(())
    };
    check(
        ir.text_standard_decimal_separator_sibling,
        "textStandardDecimalSeparator",
    )?;
    check(
        ir.text_standard_grouping_separator_sibling,
        "textStandardGroupingSeparator",
    )?;
    check(
        ir.text_standard_exponent_rep_sibling,
        "textStandardExponentRep",
    )?;
    check(ir.output_new_line_sibling, "outputNewLine")?;
    Ok(())
}

pub(crate) fn validate_text_standard_separator_semantics(
    ir: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    if ir.text_standard_decimal_separator_sibling.is_some()
        && ir.text_standard_grouping_separator_sibling.is_none()
        && ir.text_standard_grouping_separator_defined
    {
        if let Some(gid) = ir.text_standard_grouping_separator {
            let g = strings.get(gid).unwrap_or("");
            if g.trim().is_empty() {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error. Dynamic textStandardDecimalSeparator cannot be used with empty static textStandardGroupingSeparator".into(),
                }
                .into());
            }
        }
    }
    if let Some(pat_id) = ir.text_number_pattern {
        let pattern = strings.get(pat_id).unwrap_or("");
        if text_number_pattern_requires_decimal_separator(pattern) {
            if !ir.text_standard_decimal_separator_defined
                && ir.text_standard_decimal_separator_sibling.is_none()
                && ir.resolved_text_standard_decimal_separator.is_none()
            {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property textStandardDecimalSeparator is not defined".into(),
                }
                .into());
            }
            if ir.text_standard_decimal_separator_defined {
                let dec = strings.get(ir.text_standard_decimal_separator).unwrap_or("");
                if dec.is_empty() {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Property textStandardDecimalSeparator cannot be empty".into(),
                    }
                    .into());
                }
            }
        }
        if text_number_pattern_requires_grouping_separator(pattern) {
            if !ir.text_standard_grouping_separator_defined
                && ir.text_standard_grouping_separator_sibling.is_none()
                && ir.resolved_text_standard_grouping_separator.is_none()
            {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property textStandardGroupingSeparator is not defined".into(),
                }
                .into());
            }
            if ir.text_standard_grouping_separator_defined {
                let grp = ir
                    .text_standard_grouping_separator
                    .and_then(|id| strings.get(id).ok())
                    .unwrap_or("");
                if grp.is_empty() {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: textStandardGroupingSeparator must be exactly 1 character".into(),
                    }
                    .into());
                }
            }
        }
        if text_number_pattern_requires_exponent(pattern)
            && ir.text_standard_exponent_rep_defined
        {
            let exp = strings.get(ir.text_standard_exponent_rep).unwrap_or("");
            if exp.is_empty() {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: Property textStandardExponentRep cannot be empty".into(),
                }
                .into());
            }
        }
    }
    Ok(())
}

pub(crate) fn text_number_pattern_requires_decimal_separator(pattern: &str) -> bool {
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

pub(crate) fn text_number_pattern_requires_exponent(pattern: &str) -> bool {
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
        if !in_quote && (chars[i] == 'E' || chars[i] == 'e') {
            return true;
        }
        i += 1;
    }
    false
}

pub(crate) fn validate_input_value_calc_compile(
    kind: ValueKind,
    ir: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    use crate::ir::IrInputValueCalcExpression;
    use crate::schema::InputValueCalc;
    let type_label = if kind == ValueKind::Long && ir.unsigned_integer {
        "UnsignedLong"
    } else if kind == ValueKind::Integer && ir.non_negative_integer {
        "NonNegativeInteger"
    } else {
        match kind {
            ValueKind::Byte => "Byte",
            ValueKind::Short => "Short",
            ValueKind::Int => "Int",
            ValueKind::Long => "Long",
            ValueKind::UnsignedByte => "UnsignedByte",
            ValueKind::UnsignedShort => "UnsignedShort",
            ValueKind::UnsignedInt => "UnsignedInt",
            ValueKind::Float => "Float",
            ValueKind::Double => "Double",
            _ => value_kind_type_name(kind),
        }
    };
    let sde_out_of_range = |value: &str| -> SchemaError {
        SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: {value} is not in the allowed range for type {type_label}"
            ),
        }
    };
    let check_i64 = |v: i64| -> Result<()> {
        if ir.unsigned_integer && v < 0 {
            return Err(sde_out_of_range(&v.to_string()).into());
        }
        let out_of_range = || Err::<(), SchemaError>(sde_out_of_range(&v.to_string()));
        match kind {
            ValueKind::Byte => {
                i8::try_from(v).map(|_| ()).or_else(|_| out_of_range())?;
            }
            ValueKind::Short => {
                i16::try_from(v).map(|_| ()).or_else(|_| out_of_range())?;
            }
            ValueKind::Int => {
                i32::try_from(v).map(|_| ()).or_else(|_| out_of_range())?;
            }
            ValueKind::UnsignedByte => {
                u8::try_from(v).map(|_| ()).or_else(|_| out_of_range())?;
            }
            ValueKind::UnsignedShort => {
                u16::try_from(v).map(|_| ()).or_else(|_| out_of_range())?;
            }
            ValueKind::UnsignedInt => {
                u32::try_from(v).map(|_| ()).or_else(|_| out_of_range())?;
            }
            _ => {}
        }
        Ok(())
    };
    let check_lexical = |text: &str| -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        if matches!(kind, ValueKind::String | ValueKind::HexBinary) {
            return Ok(());
        }
        if let Ok(v) = text.parse::<i64>() {
            return check_i64(v);
        }
        if kind == ValueKind::Long && ir.unsigned_integer {
            if text.parse::<u64>().is_err() {
                return Err(sde_out_of_range(text).into());
            }
            return Ok(());
        }
        if text.parse::<i128>().is_err() {
            return Err(sde_out_of_range(text).into());
        }
        Ok(())
    };
    if let Some(InputValueCalc::Constant(v)) = ir.input_value_calc {
        check_i64(v)?;
    }
    if ir.input_value_calc == Some(InputValueCalc::ConstantLexical) {
        if let Some(id) = ir.input_value_calc_literal {
            let text = strings.get(id).map_err(|e| SchemaError::InvalidProperty {
                message: e.to_string(),
            })?;
            check_lexical(text)?;
        }
    }
    if let Some(expr) = &ir.input_value_calc_expression {
        match expr {
            IrInputValueCalcExpression::Literal(v) => check_i64(*v)?,
            IrInputValueCalcExpression::LiteralLexical(id) => {
                let text = strings.get(*id).map_err(|e| SchemaError::InvalidProperty {
                    message: e.to_string(),
                })?;
                check_lexical(text)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn value_kind_type_name(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Complex => "Complex",
        ValueKind::String => "String",
        ValueKind::Boolean => "Boolean",
        ValueKind::Byte => "Byte",
        ValueKind::Short => "Short",
        ValueKind::Int => "Int",
        ValueKind::Long => "Long",
        ValueKind::Integer => "Integer",
        ValueKind::UnsignedByte => "UnsignedByte",
        ValueKind::UnsignedShort => "UnsignedShort",
        ValueKind::UnsignedInt => "UnsignedInt",
        ValueKind::Float => "Float",
        ValueKind::Double => "Double",
        ValueKind::Decimal => "Decimal",
        ValueKind::DateTime => "DateTime",
        ValueKind::Time => "Time",
        ValueKind::HexBinary => "HexBinary",
    }
}

pub(crate) fn ivc_path_prefix_is_known(schema: &SchemaDocument, prefix: &str) -> bool {
    if schema.namespace_prefixes.contains_key(prefix) {
        return true;
    }
    if prefix == "tns" && schema.target_namespace.is_some() {
        return true;
    }
    matches!(prefix, "xs" | "xsd")
}

pub(crate) fn validate_schema_ivc_expression_prefixes(
    expr: &crate::schema::InputValueCalcExpression,
    schema: &SchemaDocument,
) -> Result<()> {
    use crate::schema::InputValueCalcExpression;
    match expr {
        InputValueCalcExpression::Path {
            parent_root: false, ..
        } => {}
        InputValueCalcExpression::Path {
            parent_root: true,
            steps,
            ..
        } => {
            for (prefix, _, _, _) in steps {
                if let Some(p) = prefix {
                    if !ivc_path_prefix_is_known(schema, p) {
                        return Err(SchemaError::InvalidProperty {
                            message: alloc::format!(
                                "Schema Definition Error: The prefix `{p}` has no corresponding namespace declaration in the schema."
                            ),
                        }
                        .into());
                    }
                }
            }
        }
        InputValueCalcExpression::Cast { inner, .. } => {
            validate_schema_ivc_expression_prefixes(inner, schema)?;
        }
        InputValueCalcExpression::Div(left, right) => {
            validate_schema_ivc_expression_prefixes(left, schema)?;
            validate_schema_ivc_expression_prefixes(right, schema)?;
        }
        InputValueCalcExpression::Add(items)
        | InputValueCalcExpression::Sub(items)
        | InputValueCalcExpression::Mul(items) => {
            for item in items {
                validate_schema_ivc_expression_prefixes(item, schema)?;
            }
        }
        InputValueCalcExpression::StringOf(inner) => {
            validate_schema_ivc_expression_prefixes(inner, schema)?;
        }
        InputValueCalcExpression::Ceiling(inner) => {
            validate_schema_ivc_expression_prefixes(inner, schema)?;
        }
        InputValueCalcExpression::Variable(_)
        | InputValueCalcExpression::Literal(_)
        | InputValueCalcExpression::LiteralLexical(_)
        | InputValueCalcExpression::ValueLength { .. } => {}
    }
    Ok(())
}

pub(crate) fn dfdl_props_has_input_value_calc(props: &DfdlProps) -> bool {
    props.input_value_calc.is_some()
        || props.input_value_calc_literal.is_some()
        || props.input_value_calc_sibling.is_some()
        || props.input_value_calc_segments.is_some()
        || props.input_value_calc_path.is_some()
        || props.input_value_calc_expression.is_some()
}

pub(crate) fn validate_packed_number_rep_props(ir: &IrProps, kind: ValueKind) -> Result<()> {
    if kind == ValueKind::Complex || ir.input_value_calc_expression.is_some() {
        return Ok(());
    }
    let rep = ir.binary_number_rep;
    if matches!(
        rep,
        BinaryNumberRep::PackedBcd | BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed
    ) {
        if ir.length_kind == LengthKind::Implicit {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: lengthKind='implicit' is not allowed with packed binary formats".into(),
            }
            .into());
        }
        if ir.alignment_units == LengthUnits::Bits && !ir.alignment.is_multiple_of(4) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: The given alignment ({}) must be a multiple of 4 when using packed binary formats",
                    ir.alignment
                ),
            }
            .into());
        }
        if rep == BinaryNumberRep::Bcd
            && matches!(
                kind,
                ValueKind::Byte | ValueKind::Short | ValueKind::Int | ValueKind::Long
            )
        {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: not an allowed type for bcd".into(),
            }
            .into());
        }
    }
    Ok(())
}

pub(crate) fn validate_format_has_no_input_value_calc(schema: &SchemaDocument) -> Result<()> {
    if dfdl_props_has_input_value_calc(&schema.format_defaults.props) {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property dfdl:inputValueCalc is not allowed on dfdl:format".into(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn validate_ovc_on_element_decl(
    element: &ElementDecl,
    schema: &SchemaDocument,
) -> Result<()> {
    let props = dfdl_props_for_element_ref(schema, element);
    if !props.output_value_calc.is_some() && !props.output_value_calc_conditional {
        return Ok(());
    }
    let min = element.props.occurs_min.unwrap_or(1);
    if min == 0 {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: dfdl:outputValueCalc cannot be defined on optional elements.".into(),
        }
        .into());
    }
    if props.max_occurs_specified
        && (props.occurs_max.is_none() || props.occurs_max.is_some_and(|m| m > 1))
    {
        return Err(SchemaError::InvalidProperty {
            message:
                "Schema Definition Error: dfdl:outputValueCalc cannot be defined on array elements."
                    .into(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn validate_ivc_on_element_decl(
    element: &ElementDecl,
    schema: &SchemaDocument,
) -> Result<()> {
    let props = dfdl_props_for_element_ref(schema, element);
    if !dfdl_props_has_input_value_calc(&props) {
        return Ok(());
    }
    if props.output_value_calc.is_some() || props.output_value_calc_conditional {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Cannot have both dfdl:inputValueCalc and dfdl:outputValueCalc on the same element".into(),
        }
        .into());
    }
    let min = element.props.occurs_min.unwrap_or(1);
    let max = element.props.occurs_max;
    if min == 0 || max.map(|m| m > 1).unwrap_or(false) {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: dfdl:inputValueCalc property is not allowed on optional or array elements".into(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn binary_value_kind_requires_byte_order(kind: ValueKind) -> bool {
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

pub(crate) fn validate_bit_order_byte_order(kind: ValueKind, props: &IrProps) -> Result<()> {
    use crate::schema::{BitOrder, ByteOrder, Representation};
    if props.representation != Representation::Binary {
        return Ok(());
    }
    if !binary_value_kind_requires_byte_order(kind) {
        return Ok(());
    }
    if !props.byte_order_defined {
        let is_sub_byte_or_single_byte =
            props.length_units == LengthUnits::Bits && props.length.is_some_and(|l| l <= 8);
        if !is_sub_byte_or_single_byte || props.bit_order_defined {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: Property byteOrder is not defined.".into(),
            }
            .into());
        }
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


pub(crate) fn calendar_first_day_of_week_iso(raw: &str) -> u32 {
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

pub(crate) fn validate_binary_calendar_compile(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    crate::vm::calendar_binary::validate_calendar_schema(kind, props, strings).map_err(Into::into)
}

pub(crate) fn validate_delimiter_at_compile(
    prop: &str,
    raw: &str,
    props: &DfdlProps,
) -> Result<()> {
    let raw_attr = match prop {
        "initiator" => props.raw_initiator.as_deref().unwrap_or(raw),
        "terminator" => props.raw_terminator.as_deref().unwrap_or(raw),
        "separator" => props.raw_separator.as_deref().unwrap_or(raw),
        _ => raw,
    };
    if let Err(msg) = crate::schema::validate_delimiter_schema_attribute(raw_attr) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!("Schema Definition Error. {msg}"),
        }
        .into());
    }
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
    if let Err(msg) = crate::schema::validate_delimiter_es_restriction(prop, raw) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!("Schema Definition Error. {msg}"),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn validate_delimiter_props(props: &DfdlProps, escape_ctx: &DfdlProps) -> Result<()> {
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

pub(crate) fn validate_text_boolean_reps(
    props: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    if let Some(true_id) = props.text_boolean_true_rep {
        let true_raw = strings.get(true_id).unwrap_or("");
        if true_raw.contains("%#r") {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: DFDL Byte Entity `{true_raw}` is not allowed in dfdl:textBooleanTrueRep"
                ),
            }
            .into());
        }
    }
    if let Some(false_id) = props.text_boolean_false_rep {
        let false_raw = strings.get(false_id).unwrap_or("");
        if false_raw.contains("%#r") {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: DFDL Byte Entity `{false_raw}` is not allowed in dfdl:textBooleanFalseRep"
                ),
            }
            .into());
        }
    }
    if props.length_kind == LengthKind::Explicit {
        validate_text_boolean_same_length(props, strings)?;
    }
    Ok(())
}

pub(crate) fn validate_text_boolean_same_length(
    props: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    let (Some(true_id), Some(false_id)) =
        (props.text_boolean_true_rep, props.text_boolean_false_rep)
    else {
        return Ok(());
    };
    let true_raw = strings
        .get(true_id)
        .map_err(|e| SchemaError::InvalidProperty {
            message: e.to_string(),
        })?;
    let false_raw = strings
        .get(false_id)
        .map_err(|e| SchemaError::InvalidProperty {
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

pub(crate) fn validate_text_string_pad_props(props: &DfdlProps) -> Result<()> {
    if let Some(raw) = props.text_string_pad_character.as_deref() {
        let length_units_bytes = matches!(props.length_units, Some(LengthUnits::Bytes));
        if let Err(msg) = crate::schema::validate_text_string_pad_character_merged(
            raw,
            length_units_bytes,
            props.text_string_pad_character_property_form,
        ) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
            .into());
        }
    }
    Ok(())
}

pub(crate) fn validate_initiated_content_particle(
    sequence_props: &DfdlProps,
    particle: &Particle,
) -> Result<()> {
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
                message:
                    "Schema Definition Error. initiatedContent yes requires initiator zero length"
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

pub(crate) fn validate_prefix_length_type(
    type_name: &TypeName,
    props: &DfdlProps,
    prefix_props: &IrProps,
    kind: ValueKind,
    strings: &StringPool,
) -> Result<()> {
    if props.length_kind.is_some()
        && !matches!(
            props.length_kind,
            Some(LengthKind::Explicit | LengthKind::Implicit | LengthKind::Prefixed)
        )
    {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. dfdl:prefixLengthType ex:{} specifies invalid dfdl:lengthKind",
                type_name.as_str()
            ),
        }
        .into());
    }
    if props.length_units == Some(LengthUnits::Bits)
        && prefix_props.length_units == LengthUnits::Bytes
    {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. ex:{} dfdl:prefixIncludesPrefixLength=\"yes\" dfdl:prefixLengthType dfdl:lengthUnits",
                type_name.as_str()
            ),
        }
        .into());
    }
    validate_prefixed_character_encoding(kind, prefix_props, strings)?;
    Ok(())
}

pub(crate) fn validate_fixed_occurs_count(props: &IrProps) -> Result<()> {
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
    };
    Ok(())
}

pub(crate) fn validate_model_group_occurs(group: &str, props: &DfdlProps) -> Result<()> {
    if props.occurs_min.is_some() || props.occurs_max.is_some() {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. subset: minOccurs/maxOccurs on {group} model group is not supported"
            ),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn validate_text_number_pad_character_overlap(
    props: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    use crate::schema::{Representation, TextPadKind, TextTrimKind};
    if props.representation != Representation::Text {
        return Ok(());
    }
    if props.text_pad_kind != TextPadKind::PadChar && props.text_trim_kind != TextTrimKind::PadChar
    {
        return Ok(());
    }
    let Some(pad_id) = props.text_number_pad_character else {
        return Ok(());
    };
    let pad = strings.get(pad_id).unwrap_or("");
    if pad == "0" && props.text_number_pattern.is_some() {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error. textNumberPadCharacter zero overlap".into(),
        }
        .into());
    }
    Ok(())
}
