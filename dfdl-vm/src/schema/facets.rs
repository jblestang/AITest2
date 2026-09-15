use super::ast::{
    BuiltinType, LengthKind, Representation, RestrictionBase, SchemaDocument, SimpleBase, TypeDef,
};
use crate::error::SchemaError;
use crate::ir::{IrProps, ValueKind};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Default, Clone)]
pub struct EffectiveFacets {
    pub length: Option<u64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub min_inclusive: Option<i64>,
    pub max_inclusive: Option<i64>,
    pub min_exclusive: Option<i64>,
    pub max_exclusive: Option<i64>,
    pub min_inclusive_lexical: Option<String>,
    pub max_inclusive_lexical: Option<String>,
    pub min_exclusive_lexical: Option<String>,
    pub max_exclusive_lexical: Option<String>,
    pub pattern_levels: Vec<Vec<String>>,
    pub enumeration: Option<Vec<String>>,
    pub total_digits: Option<u64>,
    pub fraction_digits: Option<u64>,
    pub invalid_min_length: Option<String>,
    pub invalid_max_length: Option<String>,
    pub invalid_length: Option<String>,
    pub invalid_total_digits: Option<String>,
    pub invalid_fraction_digits: Option<String>,
}

impl SchemaDocument {
    /// Collect XSD facets along a simple-type restriction chain.
    pub fn effective_facets(&self, base: &SimpleBase) -> EffectiveFacets {
        let mut out = EffectiveFacets::default();
        self.collect_facets(base, &mut out);
        out
    }

    fn collect_facets(&self, base: &SimpleBase, out: &mut EffectiveFacets) {
        match base {
            SimpleBase::Builtin(_) | SimpleBase::Union { .. } => {}
            SimpleBase::Restriction {
                base: parent,
                length,
                min_length,
                max_length,
                min_inclusive,
                max_inclusive,
                min_exclusive,
                max_exclusive,
                min_inclusive_lexical,
                max_inclusive_lexical,
                min_exclusive_lexical,
                max_exclusive_lexical,
                patterns,
                enumerations,
                total_digits,
                fraction_digits,
                invalid_min_length,
                invalid_max_length,
                invalid_length,
                invalid_total_digits,
                invalid_fraction_digits,
                ..
            } => {
                match parent {
                    RestrictionBase::Named(name) => {
                        if let Some(TypeDef::Simple { base: inner, .. }) = self.types.get(name) {
                            self.collect_facets(inner, out);
                        }
                    }
                    RestrictionBase::Builtin(_) => {}
                }
                if invalid_length.is_some() {
                    out.invalid_length = invalid_length.clone();
                }
                if invalid_min_length.is_some() {
                    out.invalid_min_length = invalid_min_length.clone();
                }
                if invalid_max_length.is_some() {
                    out.invalid_max_length = invalid_max_length.clone();
                }
                if invalid_total_digits.is_some() {
                    out.invalid_total_digits = invalid_total_digits.clone();
                }
                if invalid_fraction_digits.is_some() {
                    out.invalid_fraction_digits = invalid_fraction_digits.clone();
                }
                out.length = merge_length_facet(out.length, *length);
                out.min_length = merge_min_length(out.min_length, *min_length);
                out.max_length = merge_max_length(out.max_length, *max_length);
                out.min_inclusive = merge_inclusive_max(out.min_inclusive, *min_inclusive);
                out.max_inclusive = merge_inclusive_min(out.max_inclusive, *max_inclusive);
                out.min_exclusive = merge_exclusive_max(out.min_exclusive, *min_exclusive);
                out.max_exclusive = merge_exclusive_min(out.max_exclusive, *max_exclusive);
                out.min_inclusive_lexical =
                    merge_lexical_inclusive_max(out.min_inclusive_lexical.clone(), min_inclusive_lexical.clone());
                out.max_inclusive_lexical =
                    merge_lexical_inclusive_min(out.max_inclusive_lexical.clone(), max_inclusive_lexical.clone());
                out.min_exclusive_lexical =
                    merge_lexical_inclusive_max(out.min_exclusive_lexical.clone(), min_exclusive_lexical.clone());
                out.max_exclusive_lexical =
                    merge_lexical_inclusive_min(out.max_exclusive_lexical.clone(), max_exclusive_lexical.clone());
                out.total_digits = merge_min_u64(out.total_digits, *total_digits);
                out.fraction_digits = merge_min_u64(out.fraction_digits, *fraction_digits);
                if !patterns.is_empty() {
                    out.pattern_levels.push(patterns.clone());
                }
                if !enumerations.is_empty() {
                    out.enumeration = Some(enumerations.clone());
                }
            }
        }
    }
}

fn merge_length_facet(base: Option<u64>, local: Option<u64>) -> Option<u64> {
    match (base, local) {
        (Some(b), Some(l)) if b == l => Some(l),
        (None, l) => l,
        (Some(b), None) => Some(b),
        _ => local.or(base),
    }
}

fn merge_min_length(base: Option<u64>, local: Option<u64>) -> Option<u64> {
    match (base, local) {
        (Some(b), Some(l)) => Some(b.max(l)),
        (None, l) => l,
        (Some(b), None) => Some(b),
    }
}

fn merge_max_length(base: Option<u64>, local: Option<u64>) -> Option<u64> {
    match (base, local) {
        (Some(b), Some(l)) => Some(b.min(l)),
        (None, l) => l,
        (Some(b), None) => Some(b),
    }
}

fn merge_inclusive_max(base: Option<i64>, local: Option<i64>) -> Option<i64> {
    match (base, local) {
        (Some(b), Some(l)) => Some(b.max(l)),
        (None, l) => l,
        (Some(b), None) => Some(b),
    }
}

fn merge_inclusive_min(base: Option<i64>, local: Option<i64>) -> Option<i64> {
    match (base, local) {
        (Some(b), Some(l)) => Some(b.min(l)),
        (None, l) => l,
        (Some(b), None) => Some(b),
    }
}

fn merge_exclusive_max(base: Option<i64>, local: Option<i64>) -> Option<i64> {
    merge_inclusive_max(base, local)
}

fn merge_exclusive_min(base: Option<i64>, local: Option<i64>) -> Option<i64> {
    merge_inclusive_min(base, local)
}

fn merge_lexical_inclusive_max(base: Option<String>, local: Option<String>) -> Option<String> {
    match (base, local) {
        (Some(b), Some(l)) => Some(if lexical_datetime_ge(&l, &b) { l } else { b }),
        (None, l) => l,
        (Some(b), None) => Some(b),
    }
}

fn merge_lexical_inclusive_min(base: Option<String>, local: Option<String>) -> Option<String> {
    match (base, local) {
        (Some(b), Some(l)) => Some(if lexical_datetime_le(&l, &b) { l } else { b }),
        (None, l) => l,
        (Some(b), None) => Some(b),
    }
}

fn lexical_datetime_ge(a: &str, b: &str) -> bool {
    crate::vm::calendar_binary::xs_datetime_lexical_cmp(a, b)
        .map(|o| o != core::cmp::Ordering::Less)
        .unwrap_or(a >= b)
}

fn lexical_datetime_le(a: &str, b: &str) -> bool {
    crate::vm::calendar_binary::xs_datetime_lexical_cmp(a, b)
        .map(|o| o != core::cmp::Ordering::Greater)
        .unwrap_or(a <= b)
}

fn merge_min_u64(base: Option<u64>, local: Option<u64>) -> Option<u64> {
    merge_max_length(base, local)
}

fn facet_err_non_negative_integer(raw: &str) -> SchemaError {
    SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: Value '{raw}' is not facet-valid with respect to minInclusive '0' for type 'nonNegativeInteger'"
        ),
    }
}

fn facet_err_positive_integer(raw: &str) -> SchemaError {
    SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: Value '{raw}' is not facet-valid with respect to minInclusive '0' for type 'positiveInteger'"
        ),
    }
}

fn facet_err_not_integer(raw: &str) -> SchemaError {
    SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: '{raw}' is not a valid value for 'integer'"
        ),
    }
}

fn validate_enumeration_unique(enumerations: &[String]) -> Result<(), SchemaError> {
    for (idx, value) in enumerations.iter().enumerate() {
        if enumerations[..idx].iter().any(|v| v == value) {
            return Err(SchemaError::InvalidProperty {
                message: "Enumerations must be unique".into(),
            });
        }
    }
    Ok(())
}

fn validate_single_restriction(
    schema: &SchemaDocument,
    parent: &RestrictionBase,
    min_inclusive: &Option<i64>,
    min_exclusive: &Option<i64>,
    max_inclusive: &Option<i64>,
    max_exclusive: &Option<i64>,
    enumerations: &[String],
    total_digits: &Option<u64>,
    fraction_digits: &Option<u64>,
) -> Result<(), SchemaError> {
    if min_inclusive.is_some() && min_exclusive.is_some() {
        return Err(SchemaError::InvalidProperty {
            message: "MinInclusive and MinExclusive cannot be specified for the same simple type"
                .into(),
        });
    }
    if max_inclusive.is_some() && max_exclusive.is_some() {
        return Err(SchemaError::InvalidProperty {
            message: "MaxInclusive and MaxExclusive cannot be specified for the same simple type"
                .into(),
        });
    }
    if let Some(0) = total_digits {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: Value '0' is not facet-valid with respect to minInclusive '0' for type 'positiveInteger' (totalDigits)"
            ),
        });
    }
    if let (Some(frac), Some(total)) = (fraction_digits, total_digits) {
        if frac > total {
            return Err(SchemaError::InvalidProperty {
                message: "FractionDigits facet must not exceed TotalDigits".into(),
            });
        }
    }
    validate_enumeration_unique(enumerations)?;
    if !enumerations.is_empty() {
        if let Some(parent_enums) = parent_restriction_enumerations(schema, parent) {
            for value in enumerations {
                if !parent_enums.iter().any(|p| p == value) {
                    return Err(SchemaError::InvalidProperty {
                        message: "Local enumerations must be a subset of base enumerations".into(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Validate XSD facets on each restriction level in a simple-type derivation chain.
pub fn validate_restriction_chain(
    schema: &SchemaDocument,
    base: &SimpleBase,
) -> Result<(), SchemaError> {
    match base {
        SimpleBase::Restriction {
            base: parent,
            min_inclusive,
            min_exclusive,
            max_inclusive,
            max_exclusive,
            enumerations,
            total_digits,
            fraction_digits,
            ..
        } => {
            validate_single_restriction(
                schema,
                parent,
                min_inclusive,
                min_exclusive,
                max_inclusive,
                max_exclusive,
                enumerations,
                total_digits,
                fraction_digits,
            )?;
            match parent {
                RestrictionBase::Named(name) => {
                    if let Some(TypeDef::Simple { base: inner, .. }) = schema.types.get(name) {
                        validate_restriction_chain(schema, inner)?;
                    }
                }
                RestrictionBase::Builtin(_) => {}
            }
            Ok(())
        }
        SimpleBase::Union { members } => {
            for member in members {
                match member {
                    super::ast::UnionMember::Inline(inner) => {
                        validate_restriction_chain(schema, inner)?;
                    }
                    super::ast::UnionMember::Named(name) => {
                        if let Some(TypeDef::Simple { base: inner, .. }) = schema.types.get(name) {
                            validate_restriction_chain(schema, inner)?;
                        }
                    }
                }
            }
            Ok(())
        }
        SimpleBase::Builtin(_) => Ok(()),
    }
}

pub fn validate_facet_literals(eff: &EffectiveFacets) -> Result<(), SchemaError> {
    if eff.total_digits == Some(0) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: Value '0' is not facet-valid with respect to minInclusive '0' for type 'positiveInteger' (totalDigits)"
            ),
        });
    }
    for v in [
        eff.invalid_length.as_deref(),
        eff.invalid_min_length.as_deref(),
        eff.invalid_max_length.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if is_valid_non_negative_integer_literal(v) {
            continue;
        }
        if is_valid_integer_literal(v) && !is_valid_non_negative_integer_literal(v) {
            return Err(facet_err_non_negative_integer(v));
        }
        return Err(facet_err_not_integer(v));
    }
    if let Some(v) = &eff.invalid_total_digits {
        if is_valid_positive_integer_literal(v) {
            return Ok(());
        }
        if is_valid_integer_literal(v) && !is_valid_positive_integer_literal(v) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: Value '{v}' is not facet-valid with respect to minInclusive '0' for type 'positiveInteger' (totalDigits)"
                ),
            });
        }
        return Err(facet_err_not_integer(v));
    }
    if let Some(v) = &eff.invalid_fraction_digits {
        if is_valid_non_negative_integer_literal(v) {
            return Ok(());
        }
        if is_valid_integer_literal(v) && !is_valid_non_negative_integer_literal(v) {
            return Err(facet_err_non_negative_integer(v));
        }
        return Err(facet_err_not_integer(v));
    }
    Ok(())
}

fn is_valid_integer_literal(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() {
        return false;
    }
    let (sign, rest) = match t.strip_prefix('-') {
        Some(r) => (-1i64, r),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let _ = sign;
    true
}

fn is_valid_non_negative_integer_literal(raw: &str) -> bool {
    let t = raw.trim();
    if t.starts_with('-') {
        return false;
    }
    let rest = t.strip_prefix('+').unwrap_or(t);
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

fn is_valid_positive_integer_literal(raw: &str) -> bool {
    is_valid_non_negative_integer_literal(raw) && raw.trim().trim_start_matches('+') != "0"
}

fn validate_int_range_facet(name: &str, value: i64) -> Result<(), SchemaError> {
    if value < i32::MIN as i64 || value > i32::MAX as i64 {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "{name} facet value ({value}) was found to be outside of Int range."
            ),
        });
    }
    Ok(())
}

fn validate_short_range_facet(name: &str, value: i64) -> Result<(), SchemaError> {
    if value < i16::MIN as i64 || value > i16::MAX as i64 {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "{name} facet value ({value}) was found to be outside of Short range."
            ),
        });
    }
    Ok(())
}

fn validate_facet_range_order(eff: &EffectiveFacets) -> Result<(), SchemaError> {
    if let (Some(min), Some(max)) = (eff.min_exclusive, eff.max_inclusive) {
        if min > max {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "MinExclusive({min}) must be less than or equal to MaxInclusive({max})"
                ),
            });
        }
    }
    if let (Some(min), Some(max)) = (eff.min_inclusive, eff.max_inclusive) {
        if min > max {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "MinInclusive({min}) must be less than or equal to MaxInclusive({max})"
                ),
            });
        }
    }
    if let (Some(min), Some(max)) = (eff.min_exclusive, eff.max_exclusive) {
        if min >= max {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "MinExclusive({min}) must be less than or equal to MaxExclusive({max})"
                ),
            });
        }
    }
    if let (Some(min), Some(max)) = (eff.min_inclusive, eff.max_exclusive) {
        if min >= max {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "MinInclusive({min}) must be less than or equal to MaxExclusive({max})"
                ),
            });
        }
    }
    Ok(())
}

fn validate_fraction_total_digits(eff: &EffectiveFacets) -> Result<(), SchemaError> {
    if let (Some(frac), Some(total)) = (eff.fraction_digits, eff.total_digits) {
        if frac > total {
            return Err(SchemaError::InvalidProperty {
                message: "FractionDigits facet must not exceed TotalDigits".into(),
            });
        }
    }
    Ok(())
}

fn parent_restriction_enumerations(
    schema: &SchemaDocument,
    parent: &RestrictionBase,
) -> Option<Vec<String>> {
    match parent {
        RestrictionBase::Builtin(_) => None,
        RestrictionBase::Named(name) => {
            let type_def = schema.types.get(name)?;
            let TypeDef::Simple { base, .. } = type_def else {
                return None;
            };
            match base {
                SimpleBase::Restriction {
                    enumerations,
                    base: inner,
                    ..
                } => {
                    if !enumerations.is_empty() {
                        Some(enumerations.clone())
                    } else {
                        parent_restriction_enumerations(schema, inner)
                    }
                }
                _ => None,
            }
        }
    }
}

pub fn validate_value_space_facets(
    schema: &SchemaDocument,
    base: &SimpleBase,
) -> Result<(), SchemaError> {
    validate_restriction_chain(schema, base)?;
    let eff = schema.effective_facets(base);
    validate_facet_range_order(&eff)?;
    validate_fraction_total_digits(&eff)?;

    let Some(builtin) = schema.builtin_for_simple_base(base) else {
        return Ok(());
    };
    match builtin {
        BuiltinType::Int => {
            if let Some(v) = eff.min_inclusive {
                validate_int_range_facet("minInclusive", v)?;
            }
            if let Some(v) = eff.max_inclusive {
                validate_int_range_facet("maxInclusive", v)?;
            }
            if let Some(v) = eff.min_exclusive {
                validate_int_range_facet("minExclusive", v)?;
            }
            if let Some(v) = eff.max_exclusive {
                validate_int_range_facet("maxExclusive", v)?;
            }
        }
        BuiltinType::Short => {
            if let Some(v) = eff.min_inclusive {
                validate_short_range_facet("minInclusive", v)?;
            }
            if let Some(v) = eff.max_inclusive {
                validate_short_range_facet("maxInclusive", v)?;
            }
            if let Some(v) = eff.min_exclusive {
                validate_short_range_facet("minExclusive", v)?;
            }
            if let Some(v) = eff.max_exclusive {
                validate_short_range_facet("maxExclusive", v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn validate_length_facets_for_type(
    schema: &SchemaDocument,
    base: &SimpleBase,
    kind: ValueKind,
    props: &IrProps,
) -> Result<(), SchemaError> {
    if let SimpleBase::Restriction {
        base: crate::schema::RestrictionBase::Named(base_name),
        length: Some(local_len),
        ..
    } = base
    {
        if let Some(type_def) = schema.resolve_type(base_name) {
            if let crate::schema::TypeDef::Simple { base: parent_base, .. } = type_def {
                let parent_eff = schema.effective_facets(parent_base);
                if let Some(base_len) = parent_eff.length {
                    if base_len != *local_len {
                        return Err(SchemaError::InvalidProperty {
                            message: alloc::format!(
                                "Schema Definition Error length-valid-restriction: \
                                 value of length must be = the value of that of the base type"
                            ),
                        });
                    }
                }
            }
        }
    }

    let eff = schema.effective_facets(base);
    validate_facet_literals(&eff)?;
    validate_value_space_facets(schema, base)?;

    let builtin = schema.builtin_for_simple_base(base);
    let prim_name = builtin
        .map(|b| builtin_type_name(b))
        .unwrap_or("unknown");

    let has_length = eff.length.is_some();
    let has_min = eff.min_length.is_some();
    let has_max = eff.max_length.is_some();

    if has_length && (has_min || has_max) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error due to length-minLength-maxLength: \
                 It is an error for both length and minLength to be members of facets. \
                 It is not valid to not have a minLength facet if the current restriction has the minLength facet \
                 and the current restriction or base has the length facet"
            ),
        });
    }

    let length_ok = matches!(kind, ValueKind::String | ValueKind::HexBinary);
    if !length_ok && (has_length || has_min || has_max) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "The length facet or minLength/maxLength facets are not allowed on types derived from type {prim_name}.\nThey are allowed only on types derived from string and hexBinary."
            ),
        });
    }

    if props.length_kind == LengthKind::Implicit
        && props.representation == Representation::Text
        && length_ok
    {
        if has_length {
            return Ok(());
        }
        match (has_min, has_max) {
            (true, true) => {
                let min = eff.min_length.unwrap_or(0);
                let max = eff.max_length.unwrap_or(0);
                if min != max {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: The minLength and maxLength must be equal for type {prim_name} with lengthKind='implicit'. Values were minLength of {min}, maxLength of {max}."
                        ),
                    });
                }
            }
            (false, false) => {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "The length facet or minLength/maxLength facets must be defined for type {prim_name} with lengthKind='implicit'"
                    ),
                });
            }
            _ => {
                return Err(SchemaError::InvalidProperty {
                    message: "When lengthKind='implicit', both minLength and maxLength facets must be specified.".into(),
                });
            }
        }
    } else if has_min && has_max {
        let min = eff.min_length.unwrap_or(0);
        let max = eff.max_length.unwrap_or(0);
        if min > max {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "The minLength facet value must be less than or equal to the maxLength facet value. Values were minLength of {min}, maxLength of {max}."
                ),
            });
        }
    }

    Ok(())
}

fn builtin_type_name(b: BuiltinType) -> &'static str {
    match b {
        BuiltinType::Boolean => "xs:boolean",
        BuiltinType::Byte => "xs:byte",
        BuiltinType::Short => "xs:short",
        BuiltinType::Int => "xs:int",
        BuiltinType::Integer => "xs:integer",
        BuiltinType::Long => "xs:long",
        BuiltinType::UnsignedByte => "xs:unsignedByte",
        BuiltinType::UnsignedShort => "xs:unsignedShort",
        BuiltinType::UnsignedInt => "xs:unsignedInt",
        BuiltinType::Float => "xs:float",
        BuiltinType::Double => "xs:double",
        BuiltinType::Decimal => "xs:decimal",
        BuiltinType::DateTime => "xs:dateTime",
        BuiltinType::Time => "xs:time",
        BuiltinType::String => "xs:string",
        BuiltinType::HexBinary => "xs:hexBinary",
        BuiltinType::NonNegativeInteger => "xs:nonNegativeInteger",
    }
}

pub fn apply_effective_facets_to_ir(
    eff: &EffectiveFacets,
    props: &mut IrProps,
    strings: &mut crate::ir::StringPool,
) {
    if let Some(v) = eff.length {
        props.facet_length = Some(v);
        props.min_length = Some(v);
        props.max_length = Some(v);
        if props.length_kind == LengthKind::Implicit {
            props.implicit_facet_length = Some(v);
        }
    } else {
        if let Some(v) = eff.min_length {
            props.min_length = Some(v);
        }
        if let Some(v) = eff.max_length {
            props.max_length = Some(v);
        }
        if props.length_kind == LengthKind::Implicit {
            if let (Some(min), Some(max)) = (eff.min_length, eff.max_length) {
                if min == max {
                    props.implicit_facet_length = Some(min);
                }
            }
        }
    }
    props.value_min_inclusive = eff.min_inclusive;
    props.value_max_inclusive = eff.max_inclusive;
    props.value_min_exclusive = eff.min_exclusive;
    props.value_max_exclusive = eff.max_exclusive;
    if let Some(v) = &eff.min_inclusive_lexical {
        props.value_min_inclusive_lexical = Some(strings.intern(v.clone()));
    }
    if let Some(v) = &eff.max_inclusive_lexical {
        props.value_max_inclusive_lexical = Some(strings.intern(v.clone()));
    }
    if let Some(v) = &eff.min_exclusive_lexical {
        props.value_min_exclusive_lexical = Some(strings.intern(v.clone()));
    }
    if let Some(v) = &eff.max_exclusive_lexical {
        props.value_max_exclusive_lexical = Some(strings.intern(v.clone()));
    }
    props.total_digits = eff.total_digits;
    props.fraction_digits = eff.fraction_digits;
    props.facet_pattern_groups.clear();
    for level in &eff.pattern_levels {
        if level.is_empty() {
            continue;
        }
        let combined = level.join("|");
        props.facet_pattern_groups.push(strings.intern(combined));
    }
    if let Some(values) = &eff.enumeration {
        props.facet_enumeration = values
            .iter()
            .map(|v| strings.intern(v.clone()))
            .collect();
    }
}
