use crate::error::VmError;
use crate::ir::{IrProps, StringPool, ValueKind};
use crate::schema::{match_length_pattern, LengthUnits};
use crate::value::DfdlValue;
use alloc::string::ToString;
pub fn facet_validation_error(
    props: &IrProps,
    strings: &StringPool,
    _detail: alloc::string::String,
) -> VmError {
    if props.facet_check_constraints {
        if let Some(id) = props.facet_assert_message {
            if let Ok(msg) = strings.get(id) {
                let message = if props.facet_assert_daffodil_prefix {
                    alloc::format!("Assertion failed: {msg}")
                } else {
                    msg.to_string()
                };
                return VmError::InvalidValue { message };
            }
        }
        return VmError::InvalidValue {
            message: "Assertion failed: Assertion failed for dfdl:checkConstraints(.)".into(),
        };
    }
    VmError::InvalidValue {
        message: _detail,
    }
}

pub fn needs_facet_validation(props: &IrProps) -> bool {
    if props.facet_check_constraints {
        return true;
    }
    !props.facet_pattern_groups.is_empty()
        || props.value_min_inclusive.is_some()
        || props.value_max_inclusive.is_some()
        || props.value_min_exclusive.is_some()
        || props.value_max_exclusive.is_some()
        || props.total_digits.is_some()
        || props.fraction_digits.is_some()
}

pub fn validate_decoded_facets(
    value: &DfdlValue,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), VmError> {
    if kind == ValueKind::String || kind == ValueKind::HexBinary {
        let text = match value {
            DfdlValue::String(s) => s.text.as_str(),
            DfdlValue::HexBinary(_) => return Ok(()),
            _ => return Ok(()),
        };
        validate_string_facets(text, props, strings)?;
    } else if matches!(kind, ValueKind::Float | ValueKind::Double) {
        validate_float_facets(value, kind, props, strings)?;
    } else if let Some(n) = numeric_value_i64(value) {
        validate_numeric_facets(n, props, strings)?;
    }
    if props.total_digits.is_some() || props.fraction_digits.is_some() {
        if let Some(canon) = decimal_lexical_for_digit_facets(value, kind) {
            validate_digit_facets(&canon, props, strings)?;
        }
    }
    Ok(())
}

fn validate_string_facets(text: &str, props: &IrProps, strings: &StringPool) -> Result<(), VmError> {
    let char_count = text.chars().count();
    if let Some(min) = props.min_length {
        if char_count < min as usize {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: minLength ({min})"),
            ));
        }
    }
    if let Some(max) = props.max_length {
        if char_count > max as usize {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: maxLength ({max})"),
            ));
        }
    }
    for group_id in &props.facet_pattern_groups {
        let pat = strings.get(*group_id)?;
        if !pattern_group_matches(text, pat) {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: facet pattern ({pat})"),
            ));
        }
    }
    Ok(())
}

fn pattern_group_matches(text: &str, or_pattern: &str) -> bool {
    let bytes = text.as_bytes();
    for sub in or_pattern.split('|') {
        if sub.is_empty() {
            continue;
        }
        if let Some(len) = match_length_pattern(bytes, sub) {
            if len == bytes.len() {
                return true;
            }
        }
    }
    false
}

fn validate_digit_facets(
    lexical: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), VmError> {
    if let Some(max) = props.total_digits {
        let count = xsd_total_digits(lexical);
        if count > max as usize {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("number of total digits has been limited to {max}"),
            ));
        }
    }
    if let Some(max) = props.fraction_digits {
        let count = xsd_fraction_digits(lexical);
        if count > max as usize {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("number of fraction digits has been limited to {max}"),
            ));
        }
    }
    Ok(())
}

fn xsd_total_digits(lexical: &str) -> usize {
    let mut s = lexical.trim();
    if let Some(rest) = s.strip_prefix('-') {
        s = rest;
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest;
    }
    let digits: alloc::string::String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        0
    } else {
        trimmed.len()
    }
}

fn xsd_fraction_digits(lexical: &str) -> usize {
    let mut s = lexical.trim();
    if let Some(rest) = s.strip_prefix('-') {
        s = rest;
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest;
    }
    let Some((_, frac)) = s.split_once('.') else {
        return 0;
    };
    frac.chars().filter(|c| c.is_ascii_digit()).count()
}

fn decimal_lexical_for_digit_facets(value: &DfdlValue, kind: ValueKind) -> Option<alloc::string::String> {
    match (kind, value) {
        (_, DfdlValue::Decimal(s)) => Some(s.clone()),
        (ValueKind::Integer, DfdlValue::Integer(s)) => Some(s.clone()),
        _ => numeric_value_i64(value).map(|n| n.to_string()),
    }
}

fn validate_float_facets(
    value: &DfdlValue,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), VmError> {
    let f = match (kind, value) {
        (ValueKind::Float, DfdlValue::Float(v)) => *v as f64,
        (ValueKind::Double, DfdlValue::Double(v)) => *v,
        _ => return Ok(()),
    };
    if f.is_nan() {
        if props.value_min_inclusive.is_some() {
            return Err(facet_validation_error(
                props,
                strings,
                "failed facet checks due to: minInclusive".into(),
            ));
        }
        if props.value_max_inclusive.is_some() {
            return Err(facet_validation_error(
                props,
                strings,
                "failed facet checks due to: maxInclusive".into(),
            ));
        }
        if props.value_min_exclusive.is_some() {
            return Err(facet_validation_error(
                props,
                strings,
                "failed facet checks due to: minExclusive".into(),
            ));
        }
        if props.value_max_exclusive.is_some() {
            return Err(facet_validation_error(
                props,
                strings,
                "failed facet checks due to: maxExclusive".into(),
            ));
        }
        return Ok(());
    }
    if let Some(min) = props.value_min_inclusive {
        if f < min as f64 {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: minInclusive ({min})"),
            ));
        }
    }
    if let Some(max) = props.value_max_inclusive {
        if f > max as f64 {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: maxInclusive ({max})"),
            ));
        }
    }
    if let Some(min) = props.value_min_exclusive {
        if f <= min as f64 {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: minExclusive ({min})"),
            ));
        }
    }
    if let Some(max) = props.value_max_exclusive {
        if f >= max as f64 {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: maxExclusive ({max})"),
            ));
        }
    }
    Ok(())
}

fn validate_numeric_facets(
    n: i64,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), VmError> {
    if let Some(min) = props.value_min_inclusive {
        if n < min {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: minInclusive ({min})"),
            ));
        }
    }
    if let Some(max) = props.value_max_inclusive {
        if n > max {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: maxInclusive ({max})"),
            ));
        }
    }
    if let Some(min) = props.value_min_exclusive {
        if n <= min {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: minExclusive ({min})"),
            ));
        }
    }
    if let Some(max) = props.value_max_exclusive {
        if n >= max {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: maxExclusive ({max})"),
            ));
        }
    }
    Ok(())
}

fn numeric_value_i64(value: &DfdlValue) -> Option<i64> {
    match value {
        DfdlValue::Byte(v) => Some(*v as i64),
        DfdlValue::Short(v) => Some(*v as i64),
        DfdlValue::Int(v) => Some(*v as i64),
        DfdlValue::Long(v) => Some(*v as i64),
        DfdlValue::UnsignedByte(v) => Some(*v as i64),
        DfdlValue::UnsignedShort(v) => Some(*v as i64),
        DfdlValue::UnsignedInt(v) => Some(*v as i64),
        DfdlValue::UnsignedLong(v) => i64::try_from(*v).ok(),
        _ => None,
    }
}

pub fn implicit_facet_byte_length(props: &IrProps) -> Option<usize> {
    let len = props.implicit_facet_length.or(props.facet_length)?;
    match props.length_units {
        LengthUnits::Bytes => Some(len as usize),
        LengthUnits::Characters => Some(len as usize),
        LengthUnits::Bits => None,
    }
}
