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
        || !props.facet_enumeration.is_empty()
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
    } else if kind == ValueKind::Decimal {
        validate_decimal_range_facets(value, props, strings)?;
    } else if let Some(n) = numeric_value_i64(value) {
        validate_numeric_facets(n, props, strings)?;
        validate_enumeration_numeric(n, props, strings)?;
    }
    if props.total_digits.is_some() || props.fraction_digits.is_some() {
        if let Some(canon) = decimal_lexical_for_digit_facets(value, kind) {
            validate_digit_facets(&canon, props, strings)?;
        }
    }
    Ok(())
}

fn validate_decimal_range_facets(
    value: &DfdlValue,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), VmError> {
    let DfdlValue::Decimal(lex) = value else {
        return Ok(());
    };
    if let Some(min) = props.value_min_inclusive {
        if decimal_lexical_cmp(lex, &min.to_string()) == core::cmp::Ordering::Less {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: minInclusive ({min})"),
            ));
        }
    }
    if let Some(max) = props.value_max_inclusive {
        if decimal_lexical_cmp(lex, &max.to_string()) == core::cmp::Ordering::Greater {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: maxInclusive ({max})"),
            ));
        }
    }
    if let Some(min) = props.value_min_exclusive {
        if decimal_lexical_cmp(lex, &min.to_string()) != core::cmp::Ordering::Greater {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: minExclusive ({min})"),
            ));
        }
    }
    if let Some(max) = props.value_max_exclusive {
        if decimal_lexical_cmp(lex, &max.to_string()) != core::cmp::Ordering::Less {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: maxExclusive ({max})"),
            ));
        }
    }
    Ok(())
}

fn decimal_lexical_cmp(a: &str, b: &str) -> core::cmp::Ordering {
    let (asign, adigits) = decimal_lexical_parts(a);
    let (bsign, bdigits) = decimal_lexical_parts(b);
    match (asign, bsign) {
        (false, true) => core::cmp::Ordering::Greater,
        (true, false) => core::cmp::Ordering::Less,
        (true, true) => decimal_magnitude_cmp(&adigits, &bdigits).reverse(),
        (false, false) => decimal_magnitude_cmp(&adigits, &bdigits),
    }
}

fn decimal_lexical_parts(s: &str) -> (bool, alloc::string::String) {
    let s = s.trim();
    let (neg, rest) = if let Some(r) = s.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = s.strip_prefix('+') {
        (false, r)
    } else {
        (false, s)
    };
    (neg, normalize_decimal_digits(rest))
}

fn normalize_decimal_digits(s: &str) -> alloc::string::String {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        int.into()
    } else {
        alloc::format!("{int}.{frac}")
    }
}

fn decimal_magnitude_cmp(a: &str, b: &str) -> core::cmp::Ordering {
    let (ai, af) = a.split_once('.').unwrap_or((a, ""));
    let (bi, bf) = b.split_once('.').unwrap_or((b, ""));
    match ai.len().cmp(&bi.len()) {
        core::cmp::Ordering::Equal => {}
        other => return other,
    }
    match ai.cmp(bi) {
        core::cmp::Ordering::Equal => {}
        other => return other,
    }
    let af = af.trim_end_matches('0');
    let bf = bf.trim_end_matches('0');
    let max = af.len().max(bf.len());
    let af = format!("{af:0<width$}", width = max);
    let bf = format!("{bf:0<width$}", width = max);
    af.cmp(&bf)
}

fn validate_enumeration_numeric(
    n: i64,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), VmError> {
    if props.facet_enumeration.is_empty() {
        return Ok(());
    }
    for id in &props.facet_enumeration {
        let allowed = strings.get(*id)?;
        if allowed.parse::<i64>() == Ok(n) {
            return Ok(());
        }
    }
    Err(facet_validation_error(
        props,
        strings,
        "failed facet checks due to: enumeration".into(),
    ))
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
    if !props.facet_enumeration.is_empty() {
        let mut allowed = false;
        for id in &props.facet_enumeration {
            let allowed_val = strings.get(*id)?;
            if text == allowed_val {
                allowed = true;
                break;
            }
        }
        if !allowed {
            return Err(facet_validation_error(
                props,
                strings,
                alloc::format!("failed facet checks due to: enumeration"),
            ));
        }
    }
    Ok(())
}

fn pattern_group_matches(text: &str, or_pattern: &str) -> bool {
    let bytes = text.as_bytes();
    // Match the full restriction-level pattern (OR branches are already `|` in the
    // regex). Do not split on `|` — that breaks character classes like `\|`.
    match_length_pattern(bytes, or_pattern).is_some_and(|len| len == bytes.len())
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
