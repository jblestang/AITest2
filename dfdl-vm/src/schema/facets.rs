use super::ast::{
    BuiltinType, LengthKind, Representation, SchemaDocument, SimpleBase, TypeDef,
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
    pub pattern_levels: Vec<Vec<String>>,
    pub total_digits: Option<u64>,
    pub fraction_digits: Option<u64>,
    pub invalid_min_length: Option<String>,
    pub invalid_max_length: Option<String>,
    pub invalid_length: Option<String>,
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
            SimpleBase::Builtin(_) => {}
            SimpleBase::Restriction {
                base: parent,
                length,
                min_length,
                max_length,
                min_inclusive,
                max_inclusive,
                min_exclusive,
                max_exclusive,
                patterns,
                total_digits,
                fraction_digits,
                invalid_min_length,
                invalid_max_length,
                invalid_length,
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
                out.length = merge_length_facet(out.length, *length);
                out.min_length = merge_min_length(out.min_length, *min_length);
                out.max_length = merge_max_length(out.max_length, *max_length);
                out.min_inclusive = merge_inclusive_max(out.min_inclusive, *min_inclusive);
                out.max_inclusive = merge_inclusive_min(out.max_inclusive, *max_inclusive);
                out.min_exclusive = merge_exclusive_max(out.min_exclusive, *min_exclusive);
                out.max_exclusive = merge_exclusive_min(out.max_exclusive, *max_exclusive);
                out.total_digits = merge_min_u64(out.total_digits, *total_digits);
                out.fraction_digits = merge_min_u64(out.fraction_digits, *fraction_digits);
                if !patterns.is_empty() {
                    out.pattern_levels.push(patterns.clone());
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

fn merge_min_u64(base: Option<u64>, local: Option<u64>) -> Option<u64> {
    merge_max_length(base, local)
}

use super::ast::RestrictionBase;

pub fn validate_facet_literals(eff: &EffectiveFacets) -> Result<(), SchemaError> {
    if let Some(v) = &eff.invalid_length {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: minLength/maxLength facet value `{v}` is not a valid non-negative integer."
            ),
        });
    }
    if let Some(v) = &eff.invalid_min_length {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: minLength facet value `{v}` is not a valid non-negative integer."
            ),
        });
    }
    if let Some(v) = &eff.invalid_max_length {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: maxLength facet value `{v}` is not a valid non-negative integer."
            ),
        });
    }
    Ok(())
}

pub fn validate_length_facets_for_type(
    schema: &SchemaDocument,
    base: &SimpleBase,
    kind: ValueKind,
    props: &IrProps,
) -> Result<(), SchemaError> {
    let eff = schema.effective_facets(base);
    validate_facet_literals(&eff)?;

    let builtin = schema.builtin_for_simple_base(base);
    let prim_name = builtin
        .map(|b| builtin_type_name(b))
        .unwrap_or("unknown");

    let has_length = eff.length.is_some();
    let has_min = eff.min_length.is_some();
    let has_max = eff.max_length.is_some();

    if has_length && (has_min || has_max) {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Facet length cannot be defined with minLength and maxLength facets".into(),
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
                            "The minLength and maxLength must be equal for type {prim_name} with lengthKind='implicit'. Values were minLength of {min}, maxLength of {max}."
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
}
