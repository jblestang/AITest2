use super::qname::*;
use crate::error::{ParseError, Result};
use crate::expression::{
    parse_expression, BinaryOpKind, Expr, FuncKind, PathOrigin, SimpleType, StepTest, UnaryOpKind,
};
use crate::schema::ast::*;
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

pub(crate) fn props_from_attrs(attrs: &BTreeMap<String, String>) -> Result<DfdlProps> {
    props_from_attrs_with_variables(attrs, None, None, None)
}

pub(crate) fn props_from_attrs_with_variables(
    attrs: &BTreeMap<String, String>,
    variables: Option<&BTreeMap<String, String>>,
    schema_label: Option<&str>,
    mut schema_diagnostics: Option<&mut alloc::vec::Vec<String>>,
) -> Result<DfdlProps> {
    let mut props = DfdlProps::default();
    for (key, value) in attrs {
        let key = local_tag(key);
        match key {
            "representation" => {
                props.representation = Some(match value.as_str() {
                    "binary" => Representation::Binary,
                    "text" => Representation::Text,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown representation `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "byteOrder" => {
                let trimmed = value.trim();
                if let Some((test, t, f)) = parse_byte_order_if_expr(value) {
                    props.byte_order_conditional_test = Some(test);
                    props.byte_order_if_true = Some(t);
                    props.byte_order_if_false = Some(f);
                } else if trimmed.starts_with('{') && trimmed.ends_with('}') {
                    props.byte_order_conditional_test = Some(trimmed.to_string());
                } else {
                    props.byte_order = Some(match value.as_str() {
                        "bigEndian" => ByteOrder::BigEndian,
                        "littleEndian" => ByteOrder::LittleEndian,
                        other => {
                            return Err(ParseError::InvalidXml {
                                message: alloc::format!("unknown byteOrder `{other}`"),
                            }
                            .into())
                        }
                    });
                }
            }
            "bitOrder" => {
                props.bit_order = Some(match value.as_str() {
                    "mostSignificantBitFirst" => BitOrder::MostSignificantBitFirst,
                    "leastSignificantBitFirst" => BitOrder::LeastSignificantBitFirst,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown bitOrder `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "lengthKind" => {
                props.length_kind = Some(match value.as_str() {
                    "implicit" => LengthKind::Implicit,
                    "explicit" => LengthKind::Explicit,
                    "fixed" => LengthKind::Fixed,
                    "delimited" => LengthKind::Delimited,
                    "prefixed" => LengthKind::Prefixed,
                    "pattern" => LengthKind::Pattern,
                    "endOfParent" => LengthKind::EndOfParent,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown lengthKind `{other}`"),
                        }
                        .into())
                    }
                });
                props.length_kind_defined = true;
            }
            "lengthPattern" => {
                crate::schema::entities::validate_length_pattern(&value).map_err(|e| ParseError::InvalidXml { message: e })?;
                props.length_pattern = Some(value.clone());
            }
            "length" => {
                if value.trim().starts_with('{') {
                    props.length_expr_unparsed = true;
                }
                if let Ok(v) = value.parse::<u64>() {
                    props.length = Some(v);
                } else if let Some(v) = parse_constant_length_expr(value) {
                    props.length = Some(v);
                } else if let Some((sibling, cast_long, adjust)) = parse_sibling_length_expr(value)
                {
                    props.length_sibling = Some(sibling);
                    props.length_sibling_cast_long = cast_long;
                    props.length_sibling_adjust = adjust;
                } else if variables
                    .and_then(|vars| parse_variable_length_expr(value, vars))
                    .is_some()
                {
                    props.length =
                        variables.and_then(|vars| parse_variable_length_expr(value, vars));
                } else if let Some(cap) = parse_self_string_length_max_expr(value) {
                    props.length_self_string_max_cap = Some(cap);
                } else if value.contains("dfdl:valueLength(.")
                    || value.contains("dfdl:valueLength( .")
                {
                    props.length_self_value_length = true;
                } else if value.trim().starts_with('{') {
                    // Defer unsupported expressions; do not fail the whole property set.
                } else {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!("invalid length `{value}`"),
                    }
                    .into());
                }
            }
            "lengthUnits" => {
                props.length_units = Some(match value.as_str() {
                    "bytes" => LengthUnits::Bytes,
                    "bits" => LengthUnits::Bits,
                    "characters" => LengthUnits::Characters,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown lengthUnits `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "encoding" => {
                if value.trim().is_empty() {
                    return Err(crate::error::SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Property encoding value cannot be an empty string".into(),
                    }
                    .into());
                }
                props.encoding = Some(value.clone());
            }
            "encodingErrorPolicy" => {
                props.encoding_error_policy_defined = true;
                props.encoding_error_policy = Some(match value.as_str() {
                    "error" => EncodingErrorPolicy::Error,
                    "replace" => EncodingErrorPolicy::Replace,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown encodingErrorPolicy `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "nilKind" => {
                props.nil_kind = Some(match value.as_str() {
                    "literalValue" => NilKind::LiteralValue,
                    "literalCharacter" => NilKind::LiteralCharacter,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown nilKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "nilValue" => {
                // Keep the raw list; entities are expanded per-alternative at runtime.
                props.nil_value = Some(value.clone());
            }
            "separatorSuppressionPolicy" => {
                props.separator_suppression_policy = Some(match value.as_str() {
                    "anyEmpty" => SeparatorSuppressionPolicy::AnyEmpty,
                    "trailingEmpty" => SeparatorSuppressionPolicy::TrailingEmpty,
                    "trailingEmptyStrict" => SeparatorSuppressionPolicy::TrailingEmptyStrict,
                    "never" => SeparatorSuppressionPolicy::Never,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown separatorSuppressionPolicy `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "emptyElementParsePolicy" => {
                props.empty_element_parse_policy = Some(match value.as_str() {
                    "treatAsEmpty" => EmptyElementParsePolicy::TreatAsEmpty,
                    "treatAsAbsent" | "treatAsMissing" => EmptyElementParsePolicy::TreatAsAbsent,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown emptyElementParsePolicy `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "occursCountKind" => {
                props.occurs_count_kind = Some(match value.as_str() {
                    "parsed" => OccursCountKind::Parsed,
                    "implicit" => OccursCountKind::Implicit,
                    "fixed" => OccursCountKind::Fixed,
                    "expression" => OccursCountKind::Expression,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown occursCountKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "escapeSchemeRef" => {
                props.escape_scheme_ref = Some(value.to_string());
            }
            "hiddenGroupRef" => {
                props.hidden_group_ref = Some(value.to_string());
            }
            "ignoreCase" => {
                props.ignore_case = Some(match value.as_str() {
                    "yes" => true,
                    "no" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown ignoreCase `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "truncateSpecifiedLengthString" => {
                props.truncate_specified_length_string = Some(match value.as_str() {
                    "yes" => true,
                    "no" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!(
                                "unknown truncateSpecifiedLengthString `{other}`"
                            ),
                        }
                        .into())
                    }
                });
            }
            "textTrimKind" => {
                props.text_trim_kind = Some(match value.as_str() {
                    "none" => TextTrimKind::None,
                    "trim" => TextTrimKind::Trim,
                    "left" => TextTrimKind::Left,
                    "right" => TextTrimKind::Right,
                    "padChar" => TextTrimKind::PadChar,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textTrimKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textNumberPadCharacter" => {
                props.text_number_pad_character = Some(value.clone());
            }
            "textStandardBase" => {
                let base: u32 = value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("Schema Definition Error: textStandardBase invalid base `{value}`"),
                })?;
                if !matches!(base, 2 | 8 | 10 | 16) {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "Schema Definition Error: Property textStandardBase must be 2, 8, 10, or 16, got `{value}`"
                        ),
                    }
                    .into());
                }
                props.text_standard_base = Some(base);
            }
            "textStringPadCharacter" => {
                props.text_string_pad_character = Some(value.clone());
            }
            "textCalendarPadCharacter" => {
                props.text_calendar_pad_character = Some(value.clone());
            }
            "textCalendarJustification" => {
                props.text_calendar_justification = Some(match value.as_str() {
                    "left" => TextStringJustification::Left,
                    "right" => TextStringJustification::Right,
                    "center" => TextStringJustification::Center,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textCalendarJustification `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textPadKind" => {
                props.text_pad_kind = Some(match value.as_str() {
                    "none" => crate::schema::TextPadKind::None,
                    "padChar" => crate::schema::TextPadKind::PadChar,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textPadKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textStringJustification" => {
                props.text_string_justification = Some(match value.as_str() {
                    "left" => TextStringJustification::Left,
                    "right" => TextStringJustification::Right,
                    "center" => TextStringJustification::Center,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textStringJustification `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textNumberJustification" => {
                props.text_number_justification = Some(match value.as_str() {
                    "left" => TextNumberJustification::Left,
                    "right" => TextNumberJustification::Right,
                    "center" => TextNumberJustification::Center,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberJustification `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "outputValueCalc" => {
                let (ovc_value, scale) = output_value_calc_strip_multiply(value);
                if scale != 1 {
                    props.output_value_calc_scale = Some(scale);
                }
                let value = ovc_value.as_str();
                if parse_repeat_indicator_output_value_calc(value) {
                    props.output_value_calc = Some(OutputValueCalc::RepeatIndicatorFromParentCount);
                } else if let Some(expr) = parse_output_value_calc_fn_error(value) {
                    props.output_value_calc = Some(OutputValueCalc::FnError);
                    props.output_value_calc_literal = Some(expr);
                } else if let Some(inner) = parse_output_value_calc_xs_date_inner(value) {
                    if let Some(segments) =
                        parse_input_value_calc_concat(&alloc::format!("{{{inner}}}"))
                    {
                        props.output_value_calc = Some(OutputValueCalc::FnConcat);
                        props.output_value_calc_segments = Some(segments);
                    } else if let Some((steps, units, addend)) =
                        parse_output_value_calc_value_length_path(value)
                    {
                        props.output_value_calc =
                            Some(OutputValueCalc::ValueLengthInfosetPath(units, addend));
                        props.output_value_calc_path = Some(steps);
                    } else {
                        props.output_value_calc_conditional = true;
                        props.output_value_calc_literal =
                            Some(value.trim()[1..value.trim().len() - 1].trim().to_string());
                    }
                } else if let Some(segments) = parse_input_value_calc_concat(value) {
                    props.output_value_calc = Some(OutputValueCalc::FnConcat);
                    props.output_value_calc_segments = Some(segments);
                } else if value.contains("if (") {
                    props.output_value_calc_conditional = true;
                } else if let Some((steps, units, addend)) =
                    parse_output_value_calc_value_length_path(value)
                {
                    props.output_value_calc =
                        Some(OutputValueCalc::ValueLengthInfosetPath(units, addend));
                    props.output_value_calc_path = Some(steps);
                } else if let Some(steps) = parse_output_value_calc_fn_count(value) {
                    props.output_value_calc = Some(OutputValueCalc::FnCountPath);
                    props.output_value_calc_path = Some(steps);
                } else if let Some((steps, addend)) = parse_output_value_calc_infoset_path(value) {
                    props.output_value_calc = Some(OutputValueCalc::InfosetPathAddend);
                    props.output_value_calc_path = Some(steps);
                    props.output_value_calc_path_addend = Some(addend);
                } else if let Some((calc, steps, addend)) =
                    parse_output_value_calc_occurs_index(value)
                {
                    props.output_value_calc = Some(calc);
                    props.output_value_calc_path = Some(steps);
                    props.output_value_calc_path_addend = addend;
                } else if let Some(calc) = parse_output_value_calc(value) {
                    props.output_value_calc = Some(calc.0);
                    props.output_value_calc_sibling = calc.1;
                    props.output_value_calc_literal = calc.2;
                } else if looks_like_xpath_output_value_calc(value) {
                    // e.g. `{ xs:int(../ex:x) }` — computed on unparse, satisfies hidden-group rules.
                    props.output_value_calc_conditional = true;
                    props.output_value_calc_literal =
                        Some(value.trim()[1..value.trim().len() - 1].trim().to_string());
                }
            }
            "inputValueCalc" => {
                if let Some(expr) = parse_input_value_calc_expression(value)? {
                    props.input_value_calc_expression = Some(expr);
                }
                if let Some(segments) = parse_input_value_calc_concat(value) {
                    props.input_value_calc_segments = Some(segments);
                }
                if let Some(steps) = parse_input_value_calc_relative_path(value) {
                    props.input_value_calc_path = Some(steps);
                }
                if let Some((calc, lit)) =
                    variables.and_then(|vars| parse_variable_input_value_calc(value, vars))
                {
                    props.input_value_calc = Some(calc);
                    props.input_value_calc_literal = lit;
                } else if let Some(calc) = parse_input_value_calc(value) {
                    props.input_value_calc = Some(calc.0);
                    props.input_value_calc_sibling = calc.1;
                    props.input_value_calc_literal = calc.2;
                }
            }
            "parseUnparsePolicy" => {
                props.parse_unparse_policy = Some(parse_parse_unparse_policy(value)?);
            }
            "textBidi" => {
                props.text_bidi = Some(match value.trim() {
                    "yes" | "true" => true,
                    "no" | "false" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("invalid textBidi `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "floating" => {
                props.floating = Some(match value.trim() {
                    "yes" | "true" => true,
                    "no" | "false" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("invalid floating `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "binaryPackedSignCodes" => {
                props.binary_packed_sign_codes_defined = true;
                props.binary_packed_sign_codes = Some(value.clone());
            }
            "binaryNumberCheckPolicy" => {
                props.binary_number_check_policy = Some(match value.as_str() {
                    "strict" => crate::schema::BinaryNumberCheckPolicy::Strict,
                    "lax" => crate::schema::BinaryNumberCheckPolicy::Lax,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown binaryNumberCheckPolicy `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "binaryCalendarEpoch" => {
                props.binary_calendar_epoch = Some(value.clone());
            }
            "binaryNumberRep" | "binaryCalendarRep" => {
                let rep = match value.as_str() {
                    "binary" => BinaryNumberRep::Binary,
                    "bcd" => BinaryNumberRep::Bcd,
                    "packed" | "packedBCD" => BinaryNumberRep::PackedBcd,
                    "ibm4690Packed" | "ibm4690" => BinaryNumberRep::Ibm4690Packed,
                    "binarySeconds" => BinaryNumberRep::BinarySeconds,
                    "binaryMilliseconds" => BinaryNumberRep::BinaryMilliseconds,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown binary number/calendar rep `{other}`"),
                        }
                        .into())
                    }
                };
                if key == "binaryCalendarRep" {
                    props.binary_calendar_rep = Some(rep);
                } else if !matches!(
                    rep,
                    BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds
                ) {
                    props.binary_number_rep = Some(rep);
                } else {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!("unknown binary number/calendar rep `{value}`"),
                    }
                    .into());
                }
            }
            "binaryFloatRep" => {
                props.binary_float_rep = Some(match value.as_str() {
                    "ieee" => BinaryFloatRep::Ieee,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown binaryFloatRep `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "binaryDecimalVirtualPoint" => {
                let parsed: i32 = value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid binaryDecimalVirtualPoint `{value}`"),
                })?;
                if parsed < 0 {
                    props.binary_decimal_virtual_point_sde = Some(parsed);
                } else {
                    props.binary_decimal_virtual_point = Some(parsed as u32);
                }
            }
            "decimalSigned" => {
                props.decimal_signed = Some(matches!(value.as_str(), "yes" | "true" | "1"));
            }
            "calendarPattern" => props.calendar_pattern = Some(value.clone()),
            "calendarPatternKind" => {
                props.calendar_pattern_kind = Some(match value.as_str() {
                    "explicit" => crate::schema::CalendarPatternKind::Explicit,
                    "implicit" => crate::schema::CalendarPatternKind::Implicit,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown calendarPatternKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "calendarCheckPolicy" => {
                props.calendar_check_policy_lax = Some(matches!(value.as_str(), "lax"));
            }
            "calendarTimeZone" => {
                if let Some(diags) = schema_diagnostics.as_deref_mut() {
                    record_invalid_calendar_time_zone(value, schema_label, diags);
                }
                props.calendar_time_zone = Some(value.clone());
                props.calendar_time_zone_defined = true;
            }
            "calendarCenturyStart" => {
                let parsed: u32 = value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid calendarCenturyStart `{value}`"),
                })?;
                props.calendar_century_start = Some(parsed);
            }
            "calendarLanguage" => {
                if let Some(segments) = parse_input_value_calc_concat(value) {
                    props.calendar_language_segments = Some(segments);
                } else {
                    props.calendar_language = Some(value.clone());
                }
            }
            "calendarDaysInFirstWeek" => {
                let parsed: u32 = value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid calendarDaysInFirstWeek `{value}`"),
                })?;
                props.calendar_days_in_first_week = Some(parsed);
            }
            "calendarFirstDayOfWeek" => props.calendar_first_day_of_week = Some(value.clone()),
            "textNumberPattern" => props.text_number_pattern = Some(value.clone()),
            "textNumberCheckPolicy" => {
                props.text_number_check_policy = Some(match value.as_str() {
                    "strict" => crate::schema::BinaryNumberCheckPolicy::Strict,
                    "lax" => crate::schema::BinaryNumberCheckPolicy::Lax,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberCheckPolicy `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textNumberRep" => {
                props.text_number_rep = Some(match value.as_str() {
                    "standard" => crate::schema::TextNumberRep::Standard,
                    "zoned" => crate::schema::TextNumberRep::Zoned,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberRep `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textZonedSignStyle" => {
                props.text_zoned_sign_style = Some(match value.as_str() {
                    "asciiStandard" => crate::schema::TextZonedSignStyle::AsciiStandard,
                    "asciiTranslatedEBCDIC" => {
                        crate::schema::TextZonedSignStyle::AsciiTranslatedEBCDIC
                    }
                    "asciiCARealiaModified" => {
                        crate::schema::TextZonedSignStyle::AsciiCARealiaModified
                    }
                    "asciiTandemModified" => crate::schema::TextZonedSignStyle::AsciiTandemModified,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textZonedSignStyle `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textStandardDecimalSeparator" => {
                if let Some((sibling, _, _)) = parse_sibling_length_expr(value) {
                    props.text_standard_decimal_separator_sibling = Some(sibling);
                } else {
                    props.text_standard_decimal_separator = Some(value.clone());
                }
            }
            "textStandardGroupingSeparator" => {
                if let Some((sibling, _, _)) = parse_sibling_length_expr(value) {
                    props.text_standard_grouping_separator_sibling = Some(sibling);
                } else {
                    props.text_standard_grouping_separator = Some(value.clone());
                }
            }
            "textStandardExponentRep" => {
                if let Some((sibling, _, _)) = parse_sibling_length_expr(value) {
                    props.text_standard_exponent_rep_sibling = Some(sibling);
                } else {
                    props.text_standard_exponent_rep = Some(value.clone());
                }
            }
            "textStandardInfinityRep" => {
                props.text_standard_infinity_rep = Some(value.clone());
            }
            "textStandardNaNRep" => {
                props.text_standard_nan_rep = Some(value.clone());
            }
            "textStandardZeroRep" => {
                props.text_standard_zero_rep = Some(value.clone());
            }
            "textNumberRounding" => {
                props.text_number_rounding = Some(match value.as_str() {
                    "pattern" => crate::schema::TextNumberRounding::Pattern,
                    "explicit" => crate::schema::TextNumberRounding::Explicit,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberRounding `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textNumberRoundingIncrement" => {
                props.text_number_rounding_increment = Some(value.clone());
            }
            "textNumberRoundingMode" => {
                props.text_number_rounding_mode = Some(match value.as_str() {
                    "roundCeiling" => crate::schema::TextNumberRoundingMode::RoundCeiling,
                    "roundFloor" => crate::schema::TextNumberRoundingMode::RoundFloor,
                    "roundDown" => crate::schema::TextNumberRoundingMode::RoundDown,
                    "roundUp" => crate::schema::TextNumberRoundingMode::RoundUp,
                    "roundHalfEven" => crate::schema::TextNumberRoundingMode::RoundHalfEven,
                    "roundHalfDown" => crate::schema::TextNumberRoundingMode::RoundHalfDown,
                    "roundHalfUp" => crate::schema::TextNumberRoundingMode::RoundHalfUp,
                    "roundUnnecessary" => crate::schema::TextNumberRoundingMode::RoundUnnecessary,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberRoundingMode `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "initiator" => {
                if value.contains("%%") {
                    props.initiator_percent_escaped = true;
                }
                props.raw_initiator = Some(value.to_string());
                let lit = parse_delimiter_literal(value)?;
                props.initiator = Some(lit);
            }
            "terminator" => {
                props.raw_terminator = Some(value.to_string());
                let lit = parse_delimiter_literal(value)?;
                props.terminator = Some(lit);
            }
            "separator" => {
                props.raw_separator = Some(value.to_string());
                let lit = parse_delimiter_literal(value)?;
                props.separator = Some(lit);
            }
            "outputNewLine" => {
                if let Some(sib) = parse_output_new_line_encode_sibling(value) {
                    props.output_new_line_sibling = Some(sib);
                } else {
                    let lit = parse_delimiter_literal(value)?;
                    if lit.is_empty() {
                        return Err(ParseError::InvalidXml {
                            message: "For property dfdl:outputNewLine, the length of string must be exactly 1 character, except for CRLF case when it can be 2 characters.".into(),
                        }
                        .into());
                    }
                    props.output_new_line = Some(lit);
                }
            }
            "initiatedContent" => {
                props.initiated_content = Some(matches!(value.as_str(), "yes" | "true" | "1"));
            }
            "separatorPosition" => {
                props.separator_position = Some(match value.as_str() {
                    "infix" => SeparatorPosition::Infix,
                    "prefix" => SeparatorPosition::Prefix,
                    "postfix" => SeparatorPosition::Postfix,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown separatorPosition `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textBooleanTrueRep" => {
                props.text_boolean_true_rep_defined = true;
                props.text_boolean_true_rep = Some(value.clone());
            }
            "textBooleanFalseRep" => {
                props.text_boolean_false_rep_defined = true;
                props.text_boolean_false_rep = Some(value.clone());
            }
            "textBooleanPadCharacter" => {
                props.text_boolean_pad_character = Some(crate::schema::expand_entities_str(value));
            }
            "occursCount" => {
                let trimmed = value.trim();
                if trimmed.starts_with('{') && trimmed.ends_with('}') {
                    let inner = trimmed[1..trimmed.len() - 1].trim();
                    if let Ok(n) = inner.parse::<u64>() {
                        props.occurs_min = Some(n);
                        props.occurs_max = Some(n);
                        props.max_occurs_specified = true;
                        props.occurs_count_kind = Some(OccursCountKind::Expression);
                    } else if let Some(steps) = parse_fn_count_path(inner) {
                        props.occurs_count_fn_path = Some(steps);
                        props.occurs_count_kind = Some(OccursCountKind::Expression);
                    } else if !inner.starts_with("if")
                        && parse_input_value_calc_relative_path(&alloc::format!("{{{inner}}}")).is_some()
                    {
                        props.occurs_count_fn_path = parse_input_value_calc_relative_path(&alloc::format!("{{{inner}}}"));
                        props.occurs_count_kind = Some(OccursCountKind::Expression);
                    } else {
                        props.occurs_count_expr = Some(value.to_string());
                        props.occurs_count_kind = Some(OccursCountKind::Expression);
                    }
                }
            }
            "binaryBooleanTrueRep" => {
                props.binary_boolean_true_rep_defined = true;
                if value.is_empty() {
                    props.binary_boolean_true_rep = None;
                } else {
                    let n: u64 = value.parse().map_err(|_| ParseError::InvalidXml {
                        message: alloc::format!("invalid binaryBooleanTrueRep `{value}`"),
                    })?;
                    props.binary_boolean_true_rep = Some(n);
                }
            }
            "binaryBooleanFalseRep" => {
                props.binary_boolean_false_rep_defined = true;
                if value.is_empty() {
                    props.binary_boolean_false_rep = None;
                } else {
                    let n: u64 = value.parse().map_err(|_| ParseError::InvalidXml {
                        message: alloc::format!("invalid binaryBooleanFalseRep `{value}`"),
                    })?;
                    props.binary_boolean_false_rep = Some(n);
                }
            }
            "alignment" => {
                if value == "implicit" {
                    props.alignment_implicit = Some(true);
                } else {
                    props.alignment = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                        message: alloc::format!("invalid alignment `{value}`"),
                    })?);
                    props.alignment_implicit = Some(false);
                }
            }
            "alignmentKind" => {
                props.alignment_manual = Some(value == "manual");
            }
            "alignmentUnits" => {
                props.alignment_units = Some(match value.as_str() {
                    "bytes" => LengthUnits::Bytes,
                    "bits" => LengthUnits::Bits,
                    "characters" => LengthUnits::Characters,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown alignmentUnits `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "leadingSkip" => {
                props.leading_skip = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid leadingSkip `{value}`"),
                })?);
            }
            "trailingSkip" => {
                props.trailing_skip = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid trailingSkip `{value}`"),
                })?);
            }
            "sequenceKind" => {
                props.sequence_kind = Some(match value.as_str() {
                    "ordered" => SequenceKind::Ordered,
                    "unordered" => SequenceKind::Unordered,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown sequenceKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "choiceLengthKind" => {
                props.choice_length_kind = Some(match value.as_str() {
                    "implicit" => ChoiceLengthKind::Implicit,
                    "explicit" => ChoiceLengthKind::Explicit,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown choiceLengthKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "choiceLength" => {
                props.choice_length = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid choiceLength `{value}`"),
                })?);
            }
            "fillByte" => {
                props.fill_byte_raw = Some(value.to_string());
                props.fill_byte = Some(crate::schema::expand_entities(value));
            }
            "ref" => props.format_ref = Some(value.to_string()),
            "prefixLengthType" => {
                props.prefix_length_type = Some(TypeName::new(super::normalize_qname(value)));
            }
            "prefixIncludesPrefixLength" => {
                props.prefix_includes_prefix_length = Some(match value.as_str() {
                    "yes" => true,
                    "no" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown prefixIncludesPrefixLength `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "format" => {}
            "objectKind" => {
                props.object_kind = Some(match value.as_str() {
                    "bytes" => crate::schema::ObjectKind::Bytes,
                    "chars" => crate::schema::ObjectKind::Chars,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown objectKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "choiceDispatchKey" => apply_choice_dispatch_key_parse(&mut props, value),
            "choiceBranchKey" => props.choice_branch_key = Some(value.clone()),
            "defineEscapeScheme" => {
                return Err(crate::error::SchemaError::InvalidProperty {
                    message: "Schema Definition Error: defineEscapeScheme must not appear as a property on an element declaration".into(),
                }
                .into());
            }
            _ => {}
        }
    }
    Ok(props)
}

fn parse_delimiter_literal(raw: &str) -> Result<String> {
    Ok(crate::schema::parse_delimiter_literal_value(raw))
}

fn local_tag(tag: &str) -> &str {
    if let Some(idx) = tag.rfind('}') {
        return &tag[idx + 1..];
    }
    strip_prefix(tag)
}

fn strip_prefix(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

fn record_invalid_calendar_time_zone(
    value: &str,
    schema_label: Option<&str>,
    diagnostics: &mut alloc::vec::Vec<String>,
) {
    if crate::vm::calendar_binary::is_valid_dfdl_calendar_time_zone(value) {
        return;
    }
    let mut msg = alloc::format!(
        "Schema Definition Error: Value '{value}' is not valid with respect to its type, 'CalendarTimeZoneType'"
    );
    if let Some(label) = schema_label {
        msg.push('\n');
        msg.push_str(label);
    }
    diagnostics.push(msg);
}

fn is_self_node_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Path(p) => {
            p.origin == PathOrigin::Relative
                && p.steps.len() == 1
                && matches!(p.steps[0].test, StepTest::SelfNode)
        }
        Expr::Cast { expr, .. } => is_self_node_expr(expr),
        _ => false,
    }
}

fn occurs_index_addend_from_expr(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::FunctionCall { func: FuncKind::Other(ref name), .. }
            if name == "dfdl:occursIndex" || name == "occursIndex" => Some(0),
        Expr::BinaryOp { op: BinaryOpKind::Add, left, right } => {
            if let Expr::FunctionCall { func: FuncKind::Other(ref name), .. } = &**left {
                if name == "dfdl:occursIndex" || name == "occursIndex" {
                    if let Expr::Literal(ref val) = **right {
                        return literal_to_i64(val);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_assert_eq_occurs_index_addend(test: &str) -> Option<i64> {
    let expr = parse_expression(test).ok()?;
    if let Expr::BinaryOp { op: BinaryOpKind::Eq, left, right } = expr {
        if is_self_node_expr(&left) {
            return occurs_index_addend_from_expr(&right);
        } else if is_self_node_expr(&right) {
            return occurs_index_addend_from_expr(&left);
        }
    }
    None
}

pub(crate) fn apply_dfdl_assert_test(props: &mut DfdlProps, test: &str) {
    if test.contains("checkConstraints") {
        props.facet_check_constraints = true;
    }
    if let Some(addend) = parse_assert_eq_occurs_index_addend(test) {
        props.facet_check_constraints = true;
        props.assert_eq_occurs_index_addend = Some(addend);
        return;
    }
    if let Some(n) = parse_assert_int_eq_test(test) {
        props.assert_int_eq = Some(n);
        return;
    }
}

pub(crate) fn parse_assert_int_eq_test(test: &str) -> Option<i64> {
    let expr = parse_expression(test).ok()?;
    if let Expr::BinaryOp { op: BinaryOpKind::Eq, left, right } = expr {
        if is_self_node_expr(&left) {
            if let Expr::Literal(ref val) = *right {
                return literal_to_i64(val);
            }
        }
    }
    None
}

fn parse_xs_string_literal_arg(arg: &str) -> Option<String> {
    let arg = arg.trim();
    if arg.len() < 2 || !arg.starts_with('\'') || !arg.ends_with('\'') {
        return None;
    }
    Some(arg[1..arg.len() - 1].to_string())
}

fn split_top_level_commas(s: &str) -> alloc::vec::Vec<alloc::string::String> {
    let mut out = alloc::vec::Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim().to_string());
    out
}

fn split_top_level_ivc_op(s: &str, op: char) -> Option<alloc::vec::Vec<alloc::string::String>> {
    let mut parts = alloc::vec::Vec::new();
    let mut current = alloc::string::String::new();
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            c if c == op && depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    if parts.len() <= 1 {
        return None;
    }
    Some(parts)
}

fn is_valid_path_string(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty()
        || trimmed.contains('(')
        || trimmed.contains(')')
        || trimmed.contains('\'')
        || trimmed.contains('"')
        || trimmed.contains('+')
        || trimmed.contains('*')
    {
        return false;
    }
    for part in trimmed.split('/') {
        let p = part.trim();
        if p.is_empty() || p == "." || p == ".." {
            continue;
        }
        let head = p.find('[').map(|idx| &p[..idx]).unwrap_or(p).trim();
        if head.is_empty() {
            return false;
        }
        let nc = head.rsplit(':').next().unwrap_or(head);
        if !nc.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            return false;
        }
    }
    true
}



fn pest_ast_to_ivc_expr(expr: &Expr) -> Option<crate::schema::InputValueCalcExpression> {
    use crate::schema::InputValueCalcExpression as IvcExpr;
    match expr {
        Expr::Literal(ref val) => match val {
            DfdlValue::Int(n) => Some(IvcExpr::Literal(*n as i64)),
            DfdlValue::Long(n) => Some(IvcExpr::Literal(*n)),
            DfdlValue::Integer(s) => {
                if let Ok(n) = s.parse::<i64>() {
                    Some(IvcExpr::Literal(n))
                } else {
                    Some(IvcExpr::LiteralLexical(s.clone()))
                }
            }
            DfdlValue::Float(f) => Some(IvcExpr::LiteralLexical(f.to_string())),
            DfdlValue::Double(f) => Some(IvcExpr::LiteralLexical(f.to_string())),
            DfdlValue::String(s) => Some(IvcExpr::LiteralLexical(s.text.clone())),
            _ => None,
        },
        Expr::Variable { local, .. } => Some(IvcExpr::Variable(local.clone())),
        Expr::Path(path) => {
            let parent_root = matches!(path.origin, PathOrigin::Parent(_));
            let mut steps = alloc::vec::Vec::new();
            if let PathOrigin::Parent(up) = path.origin {
                for _ in 0..up {
                    steps.push((None, "..".into(), None, false));
                }
            }
            for step in &path.steps {
                let (prefix, local) = match &step.test {
                    StepTest::Name { prefix, local } => (prefix.clone(), local.clone()),
                    StepTest::Attribute(attr) => (None, alloc::format!("@{attr}")),
                    StepTest::Wildcard => (None, "*".into()),
                    StepTest::SelfNode => (None, ".".into()),
                };
                let index = step.predicate.as_ref().and_then(|p| match **p {
                    Expr::Literal(ref val) => literal_to_i64(val).filter(|&n| n > 0).map(|n| n as u32),
                    _ => None,
                });
                steps.push((prefix, local, index, false));
            }
            Some(IvcExpr::Path { parent_root, steps })
        }
        Expr::BinaryOp { op, left, right } => {
            let l = pest_ast_to_ivc_expr(left)?;
            let r = pest_ast_to_ivc_expr(right)?;
            match op {
                BinaryOpKind::Add => Some(IvcExpr::Add(alloc::vec![l, r])),
                BinaryOpKind::Sub => Some(IvcExpr::Sub(alloc::vec![l, r])),
                BinaryOpKind::Mul => Some(IvcExpr::Mul(alloc::vec![l, r])),
                BinaryOpKind::Div => {
                    Some(IvcExpr::Div(alloc::boxed::Box::new(l), alloc::boxed::Box::new(r)))
                }
                _ => None,
            }
        }
        Expr::UnaryOp { op: UnaryOpKind::Minus, expr } => {
            let inner = pest_ast_to_ivc_expr(expr)?;
            match inner {
                IvcExpr::Literal(n) => Some(IvcExpr::Literal(-n)),
                IvcExpr::LiteralLexical(s) => {
                    let mut neg = String::from("-");
                    neg.push_str(s.trim_start_matches('+'));
                    Some(IvcExpr::LiteralLexical(neg))
                }
                other => Some(IvcExpr::Sub(alloc::vec![IvcExpr::Literal(0), other])),
            }
        }
        Expr::Cast { target_type, expr } => {
            use crate::schema::IvcXsCast;
            let inner = pest_ast_to_ivc_expr(expr)?;
            if *target_type == SimpleType::String {
                return Some(IvcExpr::StringOf(alloc::boxed::Box::new(inner)));
            }
            let kind = match target_type {
                SimpleType::Byte => IvcXsCast::Byte,
                SimpleType::Short => IvcXsCast::Short,
                SimpleType::Int | SimpleType::Integer => IvcXsCast::Int,
                SimpleType::Long => IvcXsCast::Long,
                SimpleType::UnsignedByte => IvcXsCast::UnsignedByte,
                SimpleType::UnsignedShort => IvcXsCast::UnsignedShort,
                SimpleType::UnsignedInt => IvcXsCast::UnsignedInt,
                SimpleType::UnsignedLong => IvcXsCast::UnsignedLong,
                SimpleType::Float => IvcXsCast::Float,
                SimpleType::Double => IvcXsCast::Double,
                _ => return None,
            };
            Some(IvcExpr::Cast { kind, inner: alloc::boxed::Box::new(inner) })
        }
        Expr::FunctionCall { func: FuncKind::Other(ref name), args } if (name == "xs:string" || name == "xsd:string") && args.len() == 1 => {
            let inner = pest_ast_to_ivc_expr(&args[0])?;
            Some(IvcExpr::StringOf(alloc::boxed::Box::new(inner)))
        }
        Expr::FunctionCall { func: FuncKind::Other(ref name), args } if (name == "fn:ceiling" || name == "ceiling") && args.len() == 1 => {
            let inner = pest_ast_to_ivc_expr(&args[0])?;
            Some(IvcExpr::Ceiling(alloc::boxed::Box::new(inner)))
        }
        Expr::FunctionCall { func: FuncKind::DfdlValueLength | FuncKind::DfdlContentLength, args } if args.len() == 2 => {
            let raw_path = match &args[0] {
                Expr::Path(path) => {
                    let last = path.steps.last()?;
                    match &last.test {
                        StepTest::Name { local, .. } => local_name_from_qname(local).to_string(),
                        _ => return None,
                    }
                }
                _ => return None,
            };
            let units = match &args[1] {
                Expr::Literal(DfdlValue::String(u)) => length_units_from_calc_args(&u.text),
                _ => LengthUnits::Bytes,
            };
            Some(IvcExpr::ValueLength { sibling: raw_path, units })
        }
        _ => None,
    }
}

pub(crate) fn parse_input_value_calc_expression(
    value: &str,
) -> Result<Option<crate::schema::InputValueCalcExpression>> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Ok(None);
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    match parse_expression(inner) {
        Ok(expr) => Ok(pest_ast_to_ivc_expr(&expr)),
        Err(err) => Err(ParseError::InvalidXml {
            message: alloc::format!(
                "Schema Definition Error: Unable to parse expression. Variables cannot be used in path expressions: {err}"
            ),
        }
        .into()),
    }
}

fn ast_path_to_steps(
    path: &crate::expression::Path,
) -> alloc::vec::Vec<(
    Option<alloc::string::String>,
    alloc::string::String,
    Option<u32>,
    bool,
)> {
    let mut steps = alloc::vec::Vec::new();
    if let PathOrigin::Parent(up) = path.origin {
        for _ in 0..up {
            steps.push((None, "..".into(), None, false));
        }
    }
    for step in &path.steps {
        let (prefix, local) = match &step.test {
            StepTest::Name { prefix, local } => (prefix.clone(), local.clone()),
            StepTest::Attribute(attr) => (None, alloc::format!("@{attr}")),
            StepTest::Wildcard => (None, "*".into()),
            StepTest::SelfNode => (None, ".".into()),
        };
        let index = step.predicate.as_ref().and_then(|p| match **p {
            Expr::Literal(ref val) => {
                literal_to_i64(val).filter(|&n| n > 0).map(|n| n as u32)
            }
            _ => None,
        });
        steps.push((prefix, local, index, false));
    }
    steps
}

fn parse_concat_arg_segment(
    part: &str,
) -> Option<alloc::vec::Vec<crate::schema::InputValueCalcSegment>> {
    use crate::schema::InputValueCalcSegment;
    let part = part.trim();
    if part.is_empty() {
        return None;
    }
    if let Some(nested) = part.strip_prefix("fn:concat(") {
        if !part.ends_with(')') {
            return None;
        }
        let nested_args = &nested[..nested.len() - 1];
        return parse_concat_args_flat(nested_args);
    }
    if let Some(name) = part.strip_prefix("../") {
        if !name.contains('/') && !name.contains("..") {
            return Some(alloc::vec![InputValueCalcSegment::Sibling(
                local_name_from_qname(name).to_string(),
            )]);
        }
    }
    if part.starts_with('/') || part.starts_with("parent::") || part.starts_with("../") {
        if let Ok(Expr::Path(path)) = parse_expression(part) {
            let steps = ast_path_to_steps(&path);
            if !steps.is_empty() {
                return Some(alloc::vec![InputValueCalcSegment::InfosetPath(steps)]);
            }
        }
    }
    if part.len() >= 2
        && ((part.starts_with('\'') && part.ends_with('\''))
            || (part.starts_with('"') && part.ends_with('"')))
    {
        let inner = &part[1..part.len() - 1];
        let unescaped = inner.replace("''", "'").replace("\"\"", "\"");
        return Some(alloc::vec![InputValueCalcSegment::Literal(unescaped)]);
    }
    if let Some(rest) = part.strip_prefix("fn:substring(") {
        if !rest.ends_with(')') {
            return None;
        }
        let sub_args = split_top_level_commas(&rest[..rest.len() - 1]);
        if sub_args.len() != 3 {
            return None;
        }
        let sib = sub_args[0].strip_prefix("../").unwrap_or(&sub_args[0]);
        let start: usize = sub_args[1].parse().ok()?;
        let length: usize = sub_args[2].parse().ok()?;
        return Some(alloc::vec![InputValueCalcSegment::Substring {
            sibling: local_name_from_qname(sib).to_string(),
            start,
            length,
        }]);
    }
    if part.starts_with("dfdl:valueLength(") && part.ends_with(')') {
        let rest = part.strip_prefix("dfdl:valueLength(")?.strip_suffix(')')?;
        let arg_parts = split_top_level_commas(rest.trim());
        if arg_parts.len() != 2 {
            return None;
        }
        let raw_path = arg_parts[0].trim();
        if raw_path != ".." && raw_path != "." {
            let path = raw_path.strip_prefix("../").unwrap_or(raw_path);
            let units = length_units_from_calc_args(arg_parts[1].trim());
            return Some(alloc::vec![InputValueCalcSegment::ValueLength {
                sibling: local_name_from_qname(path).to_string(),
                units,
            }]);
        }
    }
    None
}

fn parse_concat_args_flat(
    args: &str,
) -> Option<alloc::vec::Vec<crate::schema::InputValueCalcSegment>> {
    let mut out = alloc::vec::Vec::new();
    for part in split_top_level_commas(args) {
        out.extend(parse_concat_arg_segment(&part)?);
    }
    Some(out)
}

pub(crate) fn parse_input_value_calc_concat(
    value: &str,
) -> Option<alloc::vec::Vec<crate::schema::InputValueCalcSegment>> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let rest = inner.strip_prefix("fn:concat(")?;
    if !rest.ends_with(')') {
        return None;
    }
    let args = &rest[..rest.len() - 1];
    parse_concat_args_flat(args)
}

pub(crate) fn parse_parse_unparse_policy(value: &str) -> Result<ParseUnparsePolicy> {
    use crate::schema::ast::ParseUnparsePolicy::*;
    Ok(match value.trim() {
        "both" => Both,
        "parseOnly" => ParseOnly,
        "unparseOnly" => UnparseOnly,
        other => {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("unknown parseUnparsePolicy `{other}`"),
            }
            .into())
        }
    })
}

fn literal_to_i64(val: &DfdlValue) -> Option<i64> {
    match val {
        DfdlValue::Int(n) => Some(*n as i64),
        DfdlValue::Long(n) => Some(*n),
        DfdlValue::Integer(s) => s.parse().ok(),
        DfdlValue::UnsignedInt(n) => Some(*n as i64),
        DfdlValue::UnsignedLong(n) => (*n).try_into().ok(),
        DfdlValue::Short(n) => Some(*n as i64),
        DfdlValue::Byte(n) => Some(*n as i64),
        _ => None,
    }
}

fn eval_i64_expr(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Literal(ref val) => literal_to_i64(val),
        Expr::UnaryOp { op: UnaryOpKind::Minus, expr } => eval_i64_expr(expr).map(|v| -v),
        Expr::UnaryOp { op: UnaryOpKind::Plus, expr } => eval_i64_expr(expr),
        _ => None,
    }
}

pub(crate) fn parse_fn_count_path(
    inner: &str,
) -> Option<
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
        bool,
    )>,
> {
    let expr = parse_expression(inner).ok()?;
    if let Expr::FunctionCall { func: FuncKind::FnCount, mut args } = expr {
        if args.len() == 1 {
            if let Expr::Path(path) = args.remove(0) {
                let mut steps = alloc::vec::Vec::new();
                if let PathOrigin::Parent(up) = path.origin {
                    for _ in 0..up {
                        steps.push((None, "..".into(), None, false));
                    }
                }
                for step in path.steps {
                    let (prefix, local) = match step.test {
                        StepTest::Name { prefix, local } => (prefix, local),
                        StepTest::Attribute(attr) => (None, alloc::format!("@{attr}")),
                        StepTest::Wildcard => (None, "*".into()),
                        StepTest::SelfNode => (None, ".".into()),
                    };
                    let index = step.predicate.and_then(|p| match *p {
                        Expr::Literal(ref val) => {
                            literal_to_i64(val).filter(|&n| n > 0).map(|n| n as u32)
                        }
                        _ => None,
                    });
                    steps.push((prefix, local, index, false));
                }
                return Some(steps);
            }
        }
    }
    None
}

type InfosetPathStepParsed = (
    Option<alloc::string::String>,
    alloc::string::String,
    Option<u32>,
    bool,
);

pub(crate) fn parse_infoset_path_step(step: &str) -> InfosetPathStepParsed {
    let (head, index, index_from_occurs) = if let Some(open) = step.find('[') {
        let bracket = step[open + 1..].strip_suffix(']').unwrap_or("");
        let bracket_trim = bracket.trim();
        if bracket_trim == "dfdl:occursIndex()" {
            (&step[..open], None, true)
        } else {
            let idx = bracket_trim.parse::<u32>().ok();
            (&step[..open], idx, false)
        }
    } else {
        (step, None, false)
    };
    let (prefix, local) = if let Some((p, l)) = head.split_once(':') {
        (Some(p.to_string()), l.to_string())
    } else {
        (None, head.to_string())
    };
    (prefix, local, index, index_from_occurs)
}

pub(crate) fn parse_output_value_calc_fn_count(
    value: &str,
) -> Option<alloc::vec::Vec<InfosetPathStepParsed>> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    parse_fn_count_path(inner)
}

pub(crate) fn parse_output_value_calc_occurs_index(
    value: &str,
) -> Option<(
    OutputValueCalc,
    alloc::vec::Vec<InfosetPathStepParsed>,
    Option<i64>,
)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let (multiply, path_part) = if let Some(rest) = inner.strip_prefix("dfdl:occursIndex() * ") {
        (true, rest.trim())
    } else {
        let rest = inner.strip_prefix("dfdl:occursIndex() + ")?;
        (false, rest.trim())
    };
    let (path_expr, addend) = match path_part.rsplit_once('+') {
        Some((left, right)) if right.trim().parse::<i64>().is_ok() && !left.contains('[') => {
            (left.trim(), Some(right.trim().parse::<i64>().ok()?))
        }
        _ => (path_part, Some(0)),
    };
    let mut rel = path_expr;
    while rel.starts_with("../") {
        rel = rel.strip_prefix("../").unwrap_or(rel);
    }
    if rel.is_empty() {
        return Some((
            OutputValueCalc::OccursIndexPath { multiply },
            alloc::vec::Vec::new(),
            addend,
        ));
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rel.split('/').filter(|s| !s.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some((OutputValueCalc::OccursIndexPath { multiply }, steps, addend))
}

fn parse_byte_order_literal(order: &str) -> Option<ByteOrder> {
    match order.trim().trim_matches('\'').trim_matches('"') {
        "bigEndian" => Some(ByteOrder::BigEndian),
        "littleEndian" => Some(ByteOrder::LittleEndian),
        _ => None,
    }
}

fn expr_to_string(expr: &Expr) -> String {
    match expr {
        Expr::Literal(v) => match v {
            DfdlValue::String(s) => alloc::format!("'{}'", s.text),
            DfdlValue::Int(n) => n.to_string(),
            DfdlValue::Long(n) => n.to_string(),
            DfdlValue::Double(f) => f.to_string(),
            DfdlValue::Boolean(b) => b.to_string(),
            _ => alloc::format!("{v:?}"),
        },
        Expr::Path(path) => {
            let mut s = String::new();
            if let PathOrigin::Parent(up) = path.origin {
                for _ in 0..up {
                    s.push_str("../");
                }
            } else if path.origin == PathOrigin::Root {
                s.push('/');
            }
            for (i, step) in path.steps.iter().enumerate() {
                if i > 0 && !s.ends_with('/') {
                    s.push('/');
                }
                match &step.test {
                    StepTest::Name { prefix: Some(p), local } => {
                        s.push_str(p);
                        s.push(':');
                        s.push_str(local);
                    }
                    StepTest::Name { prefix: None, local } => s.push_str(local),
                    StepTest::Attribute(attr) => {
                        s.push('@');
                        s.push_str(attr);
                    }
                    StepTest::Wildcard => s.push('*'),
                    StepTest::SelfNode => s.push('.'),
                }
                if let Some(ref pred) = step.predicate {
                    s.push('[');
                    s.push_str(&expr_to_string(pred));
                    s.push(']');
                }
            }
            s
        }
        Expr::BinaryOp { op, left, right } => {
            let op_str = match op {
                BinaryOpKind::Eq => "eq",
                BinaryOpKind::Ne => "ne",
                BinaryOpKind::Lt => "lt",
                BinaryOpKind::Le => "le",
                BinaryOpKind::Gt => "gt",
                BinaryOpKind::Ge => "ge",
                BinaryOpKind::Add => "+",
                BinaryOpKind::Sub => "-",
                BinaryOpKind::Mul => "*",
                BinaryOpKind::Div => "div",
                BinaryOpKind::Mod => "mod",
                BinaryOpKind::And => "and",
                BinaryOpKind::Or => "or",
            };
            alloc::format!("{} {} {}", expr_to_string(left), op_str, expr_to_string(right))
        }
        Expr::UnaryOp { op, expr } => match op {
            UnaryOpKind::Minus => alloc::format!("-{}", expr_to_string(expr)),
            UnaryOpKind::Plus => alloc::format!("+{}", expr_to_string(expr)),
            UnaryOpKind::Not => alloc::format!("not({})", expr_to_string(expr)),
        },
        Expr::FunctionCall { func, args } => {
            let arg_strs: alloc::vec::Vec<String> = args.iter().map(expr_to_string).collect();
            let func_name = match func {
                FuncKind::FnCount => "fn:count",
                FuncKind::FnConcat => "fn:concat",
                FuncKind::FnSubstring => "fn:substring",
                FuncKind::FnExists => "fn:exists",
                FuncKind::FnEmpty => "fn:empty",
                FuncKind::FnNot => "fn:not",
                FuncKind::FnNillable => "fn:nillable",
                FuncKind::DfdlOccursIndex => "dfdl:occursIndex",
                FuncKind::DfdlValueLength => "dfdl:valueLength",
                FuncKind::DfdlContentLength => "dfdl:contentLength",
                FuncKind::Other(o) => o.as_str(),
            };
            alloc::format!("{}({})", func_name, arg_strs.join(", "))
        }
        Expr::Variable { prefix: Some(p), local } => alloc::format!("${p}:{local}"),
        Expr::Variable { prefix: None, local } => alloc::format!("${local}"),
        Expr::IfThenElse { cond, then_branch, else_branch } => {
            alloc::format!("if ({}) then {} else {}", expr_to_string(cond), expr_to_string(then_branch), expr_to_string(else_branch))
        }
        Expr::Cast { target_type, expr } => {
            alloc::format!("xs:{:?}({})", target_type, expr_to_string(expr))
        }
    }
}

fn expr_to_byte_order(expr: &Expr) -> Option<ByteOrder> {
    match expr {
        Expr::Literal(DfdlValue::String(s)) => parse_byte_order_literal(&s.text),
        Expr::Literal(DfdlValue::Integer(s)) => parse_byte_order_literal(s),
        _ => None,
    }
}

pub(crate) fn parse_byte_order_if_expr(value: &str) -> Option<(String, ByteOrder, ByteOrder)> {
    let expr = parse_expression(value).ok()?;
    if let Expr::IfThenElse { cond, then_branch, else_branch } = expr {
        let t = expr_to_byte_order(&then_branch)?;
        let f = expr_to_byte_order(&else_branch)?;
        let cond_str = expr_to_string(&cond);
        if cond_str.is_empty() {
            return None;
        }
        return Some((cond_str, t, f));
    }
    None
}

pub(crate) fn parse_input_value_calc_relative_path(
    value: &str,
) -> Option<
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
        bool,
    )>,
> {
    let expr = parse_expression(value).ok()?;
    if let Expr::Path(path) = expr {
        if let PathOrigin::Parent(up) = path.origin {
            if up > 0 {
                let mut steps = alloc::vec::Vec::new();
                for _ in 0..up {
                    steps.push((None, "..".into(), None, false));
                }
                for step in path.steps {
                    let (prefix, local) = match step.test {
                        StepTest::Name { prefix, local } => (prefix, local),
                        StepTest::Attribute(attr) => (None, alloc::format!("@{attr}")),
                        StepTest::Wildcard => (None, "*".into()),
                        StepTest::SelfNode => (None, ".".into()),
                    };
                    let index = step.predicate.and_then(|p| match *p {
                        Expr::Literal(ref val) => {
                            literal_to_i64(val).filter(|&n| n > 0).map(|n| n as u32)
                        }
                        _ => None,
                    });
                    steps.push((prefix, local, index, false));
                }
                return Some(steps);
            }
        }
    }
    None
}

pub(crate) fn parse_output_value_calc_value_length_path(
    value: &str,
) -> Option<(
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
        bool,
    )>,
    LengthUnits,
    i64,
)> {
    let expr = parse_expression(value).ok()?;
    let (func_expr, addend) = match &expr {
        Expr::BinaryOp { op: BinaryOpKind::Add, left, right } => {
            let addend = eval_i64_expr(right)?;
            (left.as_ref(), addend)
        }
        _ => (&expr, 0),
    };
    if let Expr::FunctionCall { func, args } = func_expr {
        let is_val_len = match func {
            FuncKind::DfdlValueLength => true,
            FuncKind::Other(s) => s == "dfdl:valueLength",
            _ => false,
        };
        if is_val_len && args.len() == 2 {
            if let Expr::Path(path) = &args[0] {
                let steps: alloc::vec::Vec<_> = ast_path_to_steps(path)
                    .into_iter()
                    .filter(|s| s.1 != ".." && s.1 != ".")
                    .collect();
                if steps.is_empty() {
                    return None;
                }
                let units_str = expr_to_string(&args[1]);
                let units = length_units_from_calc_args(&units_str);
                return Some((steps, units, addend));
            }
        }
    }
    None
}

pub(crate) fn parse_output_value_calc_infoset_path(
    value: &str,
) -> Option<(
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
        bool,
    )>,
    i64,
)> {
    let expr = parse_expression(value).ok()?;
    let (path_expr, addend) = match &expr {
        Expr::BinaryOp { op: BinaryOpKind::Add, left, right } => {
            let addend = eval_i64_expr(right)?;
            (left.as_ref(), addend)
        }
        _ => (&expr, 0),
    };
    if let Expr::Path(path) = path_expr {
        if matches!(path.origin, PathOrigin::Parent(_)) {
            let steps: alloc::vec::Vec<_> = ast_path_to_steps(path)
                .into_iter()
                .filter(|s| s.1 != ".." && s.1 != ".")
                .collect();
            if !steps.is_empty() {
                return Some((steps, addend));
            }
        }
    }
    None
}

fn length_units_from_calc_args(args: &str) -> LengthUnits {
    let lower = args.to_ascii_lowercase();
    if lower.contains("'bits'") || lower.contains("\"bits\"") {
        LengthUnits::Bits
    } else if lower.contains("'characters'") || lower.contains("\"characters\"") {
        LengthUnits::Characters
    } else {
        LengthUnits::Bytes
    }
}

pub(crate) fn parse_input_value_calc(
    value: &str,
) -> Option<(InputValueCalc, Option<String>, Option<String>)> {
    let expr = parse_expression(value).ok()?;
    match &expr {
        Expr::Cast { target_type, expr: inner } => {
            if matches!(target_type, SimpleType::String) {
                if let Expr::Literal(DfdlValue::String(s)) = inner.as_ref() {
                    return Some((InputValueCalc::StringLiteral, None, Some(s.text.clone())));
                }
            }
        }
        Expr::FunctionCall { func, args } => {
            let name = match func {
                FuncKind::Other(s) => s.as_str(),
                FuncKind::DfdlContentLength => "dfdl:contentLength",
                FuncKind::DfdlValueLength => "dfdl:valueLength",
                _ => "",
            };
            if name == "xs:hexBinary" || name == "xsd:hexBinary" {
                if let Some(Expr::Path(path)) = args.first() {
                    if let Some(step) = path.steps.last() {
                        if let StepTest::Name { local, .. } = &step.test {
                            let local_str = local_name_from_qname(local).to_string();
                            return Some((InputValueCalc::HexBinaryFromSibling, Some(local_str), None));
                        }
                    }
                }
            }
            if args.len() == 1 {
                let units = length_units_from_calc_args(&expr_to_string(&args[0]));
                let target_str = expr_to_string(&args[0]);
                if name == "dfdl:contentLength" {
                    if target_str == ".." {
                        return Some((InputValueCalc::ContentLengthSelf(units), None, None));
                    } else {
                        let name = target_str.strip_prefix("../").unwrap_or(&target_str);
                        return Some((
                            InputValueCalc::ContentLengthSibling(units),
                            Some(local_name_from_qname(name).to_string()),
                            None,
                        ));
                    }
                } else if name == "dfdl:valueLength" {
                    if target_str == ".." {
                        return Some((InputValueCalc::ValueLengthSelf(units), None, None));
                    } else {
                        let name = target_str.strip_prefix("../").unwrap_or(&target_str);
                        return Some((
                            InputValueCalc::ValueLengthSibling(units),
                            Some(local_name_from_qname(name).to_string()),
                            None,
                        ));
                    }
                } else if name == "xs:boolean" || name == "xsd:boolean" {
                    let name = target_str.strip_prefix("../").unwrap_or(&target_str);
                    return Some((
                        InputValueCalc::BooleanFromSibling,
                        Some(local_name_from_qname(name).to_string()),
                        None,
                    ));
                }
            }
        }
        Expr::Literal(val) => {
            if let Some(n) = literal_to_i64(val) {
                return Some((InputValueCalc::Constant(n), None, None));
            }
            if let DfdlValue::String(s) = val {
                return Some((InputValueCalc::StringLiteral, None, Some(s.text.clone())));
            }
        }
        _ => {}
    }
    if let Some(v) = parse_constant_length_expr(value) {
        return Some((InputValueCalc::Constant(v as i64), None, None));
    }
    None
}

pub(crate) fn parse_variable_input_value_calc(
    value: &str,
    vars: &BTreeMap<String, String>,
) -> Option<(InputValueCalc, Option<String>)> {
    let expr = parse_expression(value).ok()?;
    if let Expr::Variable { ref local, .. } = expr {
        if vars.contains_key(local) {
            return Some((InputValueCalc::SchemaVariable, Some(local.clone())));
        }
    }
    None
}

pub(crate) fn parse_variable_length_expr(
    value: &str,
    vars: &BTreeMap<String, String>,
) -> Option<u64> {
    let expr = parse_expression(value).ok()?;
    if let Expr::Variable { ref local, .. } = expr {
        return vars.get(local)?.parse().ok();
    }
    None
}

pub(crate) fn parse_repeat_indicator_output_value_calc(value: &str) -> bool {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .map(str::trim)
        .unwrap_or(trimmed);
    let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("dfdl:occursIndex()ltfn:count(..)") && compact.contains("then1else0")
}

pub(crate) fn looks_like_xpath_output_value_calc(value: &str) -> bool {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return false;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    inner.contains("../")
        || inner.starts_with("fn:")
        || inner.contains("xs:int(")
        || inner.contains("xs:string(")
        || inner.contains("xs:long(")
        || inner.contains("xs:unsignedByte(")
}

pub(crate) fn parse_output_new_line_encode_sibling(value: &str) -> Option<String> {
    let expr = parse_expression(value).ok()?;
    if let Expr::FunctionCall { func, args } = &expr {
        let name_str = match func {
            FuncKind::Other(s) => s.as_str(),
            _ => "",
        };
        if name_str == "dfdl:encodeDFDLEntities" && args.len() == 1 {
            if let Expr::Path(path) = &args[0] {
                if matches!(path.origin, PathOrigin::Parent(_)) && !path.steps.is_empty() {
                    let last_step = path.steps.last()?;
                    if let StepTest::Name { local, .. } = &last_step.test {
                        return Some(local_name_from_qname(local).to_string());
                    }
                }
            }
        }
    }
    None
}

fn parse_decode_dfdl_entities_call(arg: &str) -> Option<String> {
    let arg = arg.trim();
    let prefix = "dfdl:decodeDFDLEntities(";
    if !arg.starts_with(prefix) || !arg.ends_with(')') {
        return None;
    }
    let inner = arg[prefix.len()..arg.len() - 1].trim();
    let lit = parse_xs_string_literal_arg(inner)?;
    Some(crate::schema::expand_entities_str(&lit))
}

fn has_fn_error_call(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { func, args } => {
            if let FuncKind::Other(ref s) = func {
                if s == "fn:error" {
                    return true;
                }
            }
            args.iter().any(has_fn_error_call)
        }
        Expr::UnaryOp { expr, .. } => has_fn_error_call(expr),
        Expr::BinaryOp { left, right, .. } => has_fn_error_call(left) || has_fn_error_call(right),
        Expr::IfThenElse { cond, then_branch, else_branch } => {
            has_fn_error_call(cond) || has_fn_error_call(then_branch) || has_fn_error_call(else_branch)
        }
        Expr::Cast { expr, .. } => has_fn_error_call(expr),
        _ => false,
    }
}

pub(crate) fn parse_output_value_calc_fn_error(value: &str) -> Option<String> {
    let expr = parse_expression(value).ok()?;
    if has_fn_error_call(&expr) {
        Some(value.trim().trim_start_matches('{').trim_end_matches('}').trim().to_string())
    } else {
        None
    }
}

pub(crate) fn parse_output_value_calc_xs_date_inner(value: &str) -> Option<alloc::string::String> {
    let expr = parse_expression(value).ok()?;
    if let Expr::FunctionCall { func, args } = &expr {
        let name_str = match func {
            FuncKind::Other(s) => s.as_str(),
            _ => "",
        };
        if name_str == "xs:date" && args.len() == 1 {
            return Some(expr_to_string(&args[0]));
        }
    }
    None
}

pub(crate) fn output_value_calc_strip_multiply(value: &str) -> (alloc::string::String, i64) {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return (value.to_string(), 1);
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(star) = inner.rfind('*') {
        let left = inner[..star].trim();
        let right = inner[star + 1..].trim();
        if let Ok(n) = right.parse::<i64>() {
            return (alloc::format!("{{{left}}}"), n);
        }
    }
    (value.to_string(), 1)
}

fn parse_ovc_from_ast(
    expr: &Expr,
) -> Option<(OutputValueCalc, Option<String>, Option<String>)> {
    match expr {
        Expr::Literal(DfdlValue::String(s)) => {
            Some((OutputValueCalc::Constant(0), None, Some(s.text.clone())))
        }
        Expr::Literal(DfdlValue::Int(n)) => Some((OutputValueCalc::Constant(*n as i64), None, None)),
        Expr::Literal(DfdlValue::Long(n)) => Some((OutputValueCalc::Constant(*n), None, None)),
        Expr::Literal(DfdlValue::Double(f)) => {
            Some((OutputValueCalc::Constant(0), None, Some(f.to_string())))
        }
        Expr::Literal(DfdlValue::Integer(s)) => {
            if let Ok(v) = s.parse::<i64>() {
                Some((OutputValueCalc::Constant(v), None, None))
            } else {
                Some((OutputValueCalc::Constant(0), None, Some(s.clone())))
            }
        }
        Expr::Cast { expr, .. } => parse_ovc_from_ast(expr),
        Expr::BinaryOp { op: BinaryOpKind::Add, left, right } => {
            if let Expr::Literal(ref val) = **right {
                let addend = literal_to_i64(val).unwrap_or(0);
                if let Some((calc, sib, lit)) = parse_ovc_from_ast(left) {
                    let updated_calc = match calc {
                        OutputValueCalc::ContentLengthSelf(units, _) => OutputValueCalc::ContentLengthSelf(units, addend),
                        OutputValueCalc::ValueLengthSelf(units, _) => OutputValueCalc::ValueLengthSelf(units, addend),
                        OutputValueCalc::ContentLengthSibling(units, _) => OutputValueCalc::ContentLengthSibling(units, addend),
                        OutputValueCalc::ValueLengthSibling(units, _) => OutputValueCalc::ValueLengthSibling(units, addend),
                        other => other,
                    };
                    return Some((updated_calc, sib, lit));
                }
            }
            None
        }
        Expr::FunctionCall { func, args } => {
            match func {
                FuncKind::DfdlContentLength | FuncKind::DfdlValueLength => {
                    if args.is_empty() {
                        return None;
                    }
                    let units = if args.len() >= 2 {
                        if let Expr::Literal(DfdlValue::String(ref s)) = args[1] {
                            length_units_from_calc_args(&s.text)
                        } else {
                            LengthUnits::Bytes
                        }
                    } else {
                        LengthUnits::Bytes
                    };
                    if let Expr::Path(ref path) = args[0] {
                        if path.origin == PathOrigin::Parent(1) && path.steps.is_empty() {
                            let calc = if *func == FuncKind::DfdlContentLength {
                                OutputValueCalc::ContentLengthSelf(units, 0)
                            } else {
                                OutputValueCalc::ValueLengthSelf(units, 0)
                            };
                            return Some((calc, None, None));
                        }
                        if let PathOrigin::Parent(up) = path.origin {
                            if up >= 1 && !path.steps.is_empty() {
                                if let StepTest::Name { ref local, .. } = path.steps[0].test {
                                    let calc = if *func == FuncKind::DfdlContentLength {
                                        OutputValueCalc::ContentLengthSibling(units, 0)
                                    } else {
                                        OutputValueCalc::ValueLengthSibling(units, 0)
                                    };
                                    return Some((calc, Some(local_name_from_qname(local).to_string()), None));
                                }
                            }
                        }
                    }
                    None
                }
                FuncKind::FnSubstring => {
                    if args.len() == 3 {
                        let sib_name = match &args[0] {
                            Expr::Path(p) if !p.steps.is_empty() => match &p.steps[0].test {
                                StepTest::Name { local, .. } => local_name_from_qname(local).to_string(),
                                _ => return None,
                            },
                            _ => return None,
                        };
                        let start = eval_constant_uint_expr(&args[1])? as usize;
                        let length = eval_constant_uint_expr(&args[2])? as usize;
                        return Some((OutputValueCalc::Substring { start, length }, Some(sib_name), None));
                    }
                    None
                }
                FuncKind::Other(ref name) => {
                    if (name == "fn:string-length" || name == "string-length") && args.len() == 1 {
                        if let Expr::Path(p) = &args[0] {
                            if !p.steps.is_empty() {
                                if let StepTest::Name { local, .. } = &p.steps[0].test {
                                    return Some((OutputValueCalc::StringLengthSibling, Some(local_name_from_qname(local).to_string()), None));
                                }
                            }
                        }
                    }
                    if name == "dfdl:decodeDFDLEntities" && args.len() == 1 {
                        if let Expr::Literal(DfdlValue::String(ref s)) = args[0] {
                            let decoded = crate::schema::expand_entities_str(&s.text);
                            return Some((OutputValueCalc::Constant(0), None, Some(decoded)));
                        }
                    }
                    if (name == "xs:hexBinary" || name == "dfdl:hexBinary") && args.len() == 1 {
                        if let Expr::Literal(DfdlValue::String(ref s)) = args[0] {
                            return Some((OutputValueCalc::HexBinaryFromLexical, None, Some(s.text.clone())));
                        }
                        if let Some(n) = eval_i64_expr(&args[0]) {
                            return Some((OutputValueCalc::HexBinaryFromInteger(n), None, None));
                        }
                        if let Expr::Path(ref p) = args[0] {
                            if p.origin == PathOrigin::Parent(1) && !p.steps.is_empty() {
                                if let StepTest::Name { ref local, .. } = p.steps[0].test {
                                    return Some((OutputValueCalc::HexBinaryFromByteSibling, Some(local_name_from_qname(local).to_string()), None));
                                }
                            }
                        }
                        if let Expr::Cast { target_type: SimpleType::Byte, expr: ref inner_expr } = args[0] {
                            if let Expr::Path(ref p) = **inner_expr {
                                if p.origin == PathOrigin::Parent(1) && !p.steps.is_empty() {
                                    if let StepTest::Name { ref local, .. } = p.steps[0].test {
                                        return Some((OutputValueCalc::HexBinaryFromByteSibling, Some(local_name_from_qname(local).to_string()), None));
                                    }
                                }
                            }
                        }
                        if let Expr::Cast { target_type: SimpleType::Short, expr: ref inner_expr } = args[0] {
                            if let Expr::Literal(ref val) = **inner_expr {
                                if let Some(n) = literal_to_i64(val) {
                                    return Some((OutputValueCalc::HexBinaryFromShort(n as i16), None, None));
                                }
                            }
                        }
                    }
                    None
                }
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn parse_output_value_calc(
    value: &str,
) -> Option<(OutputValueCalc, Option<String>, Option<String>)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let expr = parse_expression(inner).ok()?;
    parse_ovc_from_ast(&expr)
}

fn find_string_length_gt_limit(expr: &Expr) -> Option<u64> {
    match expr {
        Expr::BinaryOp { op, left, right } if matches!(op, BinaryOpKind::Gt | BinaryOpKind::Ge) => {
            if let Expr::FunctionCall { func: FuncKind::Other(ref name), args } = &**left {
                if (name == "fn:string-length" || name == "string-length")
                    && args.len() == 1
                    && matches!(&args[0], Expr::Path(p) if p.steps.iter().any(|s| matches!(s.test, StepTest::SelfNode)))
                {
                    if let Expr::Literal(ref val) = **right {
                        return literal_to_i64(val).and_then(|n| n.try_into().ok());
                    }
                }
            }
            find_string_length_gt_limit(left).or_else(|| find_string_length_gt_limit(right))
        }
        Expr::IfThenElse { cond, then_branch, else_branch } => {
            find_string_length_gt_limit(cond)
                .or_else(|| find_string_length_gt_limit(then_branch))
                .or_else(|| find_string_length_gt_limit(else_branch))
        }
        Expr::BinaryOp { left, right, .. } => {
            find_string_length_gt_limit(left).or_else(|| find_string_length_gt_limit(right))
        }
        _ => None,
    }
}

pub(crate) fn parse_self_string_length_max_expr(value: &str) -> Option<u64> {
    if let Ok(expr) = parse_expression(value) {
        return find_string_length_gt_limit(&expr);
    }
    None
}

pub(crate) fn apply_choice_dispatch_key_parse(props: &mut DfdlProps, value: &str) {
    props.choice_dispatch_key = Some(value.to_string());
    props.choice_dispatch_sibling = None;
    props.choice_dispatch_path = None;
    props.choice_dispatch_literal = None;
    props.choice_dispatch_sibling_int = None;
    if let Some(steps) = parse_input_value_calc_relative_path(value) {
        if steps.len() > 1 {
            props.choice_dispatch_path = Some(steps);
            return;
        }
    }
    let expr = match parse_expression(value) {
        Ok(e) => e,
        Err(_) => return,
    };
    match &expr {
        Expr::Path(path) => {
            inspect_choice_dispatch_path(props, path);
        }
        Expr::FunctionCall { func: FuncKind::Other(ref name), args } if (name == "xs:string" || name == "xsd:string") && args.len() == 1 => {
            inspect_choice_dispatch_inner_expr(props, &args[0]);
        }
        Expr::Cast { target_type: SimpleType::String, expr: inner } => {
            inspect_choice_dispatch_inner_expr(props, inner);
        }
        _ => {}
    }
}

fn inspect_choice_dispatch_path(props: &mut DfdlProps, p: &crate::expression::Path) {
    let elem_steps: alloc::vec::Vec<_> = p
        .steps
        .iter()
        .filter(|s| match &s.test {
            StepTest::SelfNode => false,
            StepTest::Name { local, .. } => local != "..",
            _ => true,
        })
        .collect();
    if elem_steps.len() > 1 {
        props.choice_dispatch_path = Some(ast_path_to_steps(p));
    } else if let Some(step) = elem_steps.last() {
        if let StepTest::Name { local, .. } = &step.test {
            props.choice_dispatch_sibling = Some(local_name_from_qname(local).to_string());
        }
    }
}

fn inspect_choice_dispatch_inner_expr(props: &mut DfdlProps, inner: &Expr) {
    match inner {
        Expr::FunctionCall { func: FuncKind::Other(ref name), args } if (name == "xs:int" || name == "xsd:int") && args.len() == 1 => {
            if let Expr::Path(p) = &args[0] {
                if let Some(step) = p.steps.last() {
                    if let StepTest::Name { local, .. } = &step.test {
                        props.choice_dispatch_sibling_int = Some(local_name_from_qname(local).to_string());
                    }
                }
            }
        }
        Expr::Cast { target_type: SimpleType::Int | SimpleType::Integer, expr: inner_expr } => {
            if let Expr::Path(p) = inner_expr.as_ref() {
                if let Some(step) = p.steps.last() {
                    if let StepTest::Name { local, .. } = &step.test {
                        props.choice_dispatch_sibling_int = Some(local_name_from_qname(local).to_string());
                    }
                }
            }
        }
        Expr::Path(p) => {
            inspect_choice_dispatch_path(props, p);
        }
        Expr::Literal(DfdlValue::String(s)) => {
            props.choice_dispatch_literal = Some(s.text.clone());
        }
        _ => {}
    }
}

fn parse_sibling_length_from_ast(expr: &Expr) -> Option<(String, bool, i64)> {
    match expr {
        Expr::Path(path) => {
            if let PathOrigin::Parent(1) = path.origin {
                if path.steps.len() == 1 {
                    if let StepTest::Name { local, .. } = &path.steps[0].test {
                        return Some((local_name_from_qname(local).to_string(), false, 0));
                    }
                }
            }
            None
        }
        Expr::Cast { target_type, expr } => {
            let is_long_or_int = matches!(target_type, SimpleType::Long | SimpleType::Int | SimpleType::Integer);
            let (name, _, adjust) = parse_sibling_length_from_ast(expr)?;
            Some((name, is_long_or_int, adjust))
        }
        Expr::BinaryOp { op, left, right } => {
            let (name, is_long, base_adjust) = parse_sibling_length_from_ast(left)?;
            if let Expr::Literal(ref val) = **right {
                let delta = literal_to_i64(val)?;
                let adjust = match op {
                    BinaryOpKind::Add => base_adjust.checked_add(delta)?,
                    BinaryOpKind::Sub => base_adjust.checked_sub(delta)?,
                    _ => return None,
                };
                return Some((name, is_long, adjust));
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn parse_sibling_length_expr(value: &str) -> Option<(String, bool, i64)> {
    let expr = parse_expression(value).ok()?;
    parse_sibling_length_from_ast(&expr)
}

fn eval_constant_uint_expr(expr: &Expr) -> Option<u64> {
    match expr {
        Expr::Literal(val) => literal_to_i64(val).filter(|&n| n >= 0).map(|n| n as u64),
        Expr::BinaryOp { op, left, right } => {
            let l = eval_constant_uint_expr(left)?;
            let r = eval_constant_uint_expr(right)?;
            match op {
                BinaryOpKind::Add => l.checked_add(r),
                BinaryOpKind::Sub => l.checked_sub(r),
                BinaryOpKind::Mul => l.checked_mul(r),
                BinaryOpKind::Div if r != 0 => Some(l / r),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Parses constant DFDL length expressions such as `{ 6 }`, `{1}`, or `{ 1 + 1 }`.
pub(crate) fn parse_constant_length_expr(value: &str) -> Option<u64> {
    let expr = parse_expression(value).ok()?;
    eval_constant_uint_expr(&expr)
}

pub(crate) fn merge_dfdl_props(mut base: DfdlProps, overlay: DfdlProps) -> DfdlProps {
    if overlay.representation.is_some() {
        base.representation = overlay.representation;
    }
    if overlay.byte_order.is_some() {
        base.byte_order = overlay.byte_order;
    }
    if overlay.byte_order_conditional_test.is_some() {
        base.byte_order_conditional_test = overlay.byte_order_conditional_test;
        base.byte_order_if_true = overlay.byte_order_if_true;
        base.byte_order_if_false = overlay.byte_order_if_false;
    }
    if overlay.bit_order.is_some() {
        base.bit_order = overlay.bit_order;
    }
    if overlay.length_kind.is_some() {
        base.length_kind = overlay.length_kind;
        base.length_kind_defined = overlay.length_kind_defined;
    }
    if overlay.length.is_some() {
        base.length = overlay.length;
    }
    if overlay.length_sibling.is_some() {
        base.length_sibling = overlay.length_sibling;
        base.length_sibling_cast_long = overlay.length_sibling_cast_long;
        base.length_sibling_adjust = overlay.length_sibling_adjust;
    }
    if overlay.length_expr_unparsed {
        base.length_expr_unparsed = true;
    }
    if overlay.length_self_string_max_cap.is_some() {
        base.length_self_string_max_cap = overlay.length_self_string_max_cap;
    }
    if overlay.length_self_value_length {
        base.length_self_value_length = true;
    }
    if overlay.length_units.is_some() {
        base.length_units = overlay.length_units;
    }
    if overlay.encoding.is_some() {
        base.encoding = overlay.encoding;
    }
    if overlay.encoding_error_policy.is_some() {
        base.encoding_error_policy = overlay.encoding_error_policy;
    }
    if overlay.encoding_error_policy_defined {
        base.encoding_error_policy_defined = true;
    }
    if overlay.text_trim_kind.is_some() {
        base.text_trim_kind = overlay.text_trim_kind;
    }
    if overlay.text_pad_kind.is_some() {
        base.text_pad_kind = overlay.text_pad_kind;
    }
    if overlay.format_context_finalized {
        base.format_context_finalized = true;
    }
    if overlay.truncate_specified_length_string.is_some() {
        base.truncate_specified_length_string = overlay.truncate_specified_length_string;
    }
    if overlay.text_number_pad_character.is_some() {
        base.text_number_pad_character = overlay.text_number_pad_character;
        base.text_number_pad_character_property_form =
            overlay.text_number_pad_character_property_form;
    }
    if overlay.text_string_pad_character.is_some() {
        base.text_string_pad_character = overlay.text_string_pad_character;
        base.text_string_pad_character_property_form =
            overlay.text_string_pad_character_property_form;
    }
    if overlay.text_calendar_pad_character.is_some() {
        base.text_calendar_pad_character = overlay.text_calendar_pad_character;
    }
    if overlay.text_calendar_justification.is_some() {
        base.text_calendar_justification = overlay.text_calendar_justification;
    }
    if overlay.binary_number_rep.is_some() {
        base.binary_number_rep = overlay.binary_number_rep;
    }
    if overlay.binary_packed_sign_codes.is_some() {
        base.binary_packed_sign_codes = overlay.binary_packed_sign_codes;
        base.binary_packed_sign_codes_defined = overlay.binary_packed_sign_codes_defined;
    }
    if overlay.binary_number_check_policy.is_some() {
        base.binary_number_check_policy = overlay.binary_number_check_policy;
    }
    if overlay.binary_calendar_rep.is_some() {
        base.binary_calendar_rep = overlay.binary_calendar_rep;
    }
    if overlay.binary_calendar_epoch.is_some() {
        base.binary_calendar_epoch = overlay.binary_calendar_epoch;
    }
    if overlay.binary_float_rep.is_some() {
        base.binary_float_rep = overlay.binary_float_rep;
    }
    if overlay.binary_decimal_virtual_point.is_some() {
        base.binary_decimal_virtual_point = overlay.binary_decimal_virtual_point;
    }
    if overlay.binary_decimal_virtual_point_sde.is_some() {
        base.binary_decimal_virtual_point_sde = overlay.binary_decimal_virtual_point_sde;
    }
    if overlay.decimal_signed.is_some() {
        base.decimal_signed = overlay.decimal_signed;
    }
    if overlay.calendar_pattern.is_some() {
        base.calendar_pattern = overlay.calendar_pattern;
    }
    if overlay.calendar_pattern_kind.is_some() {
        base.calendar_pattern_kind = overlay.calendar_pattern_kind;
    }
    if overlay.calendar_check_policy_lax.is_some() {
        base.calendar_check_policy_lax = overlay.calendar_check_policy_lax;
    }
    if overlay.calendar_century_start.is_some() {
        base.calendar_century_start = overlay.calendar_century_start;
    }
    if overlay.calendar_language.is_some() {
        base.calendar_language = overlay.calendar_language;
    }
    if overlay.calendar_language_segments.is_some() {
        base.calendar_language_segments = overlay.calendar_language_segments;
    }
    if overlay.calendar_days_in_first_week.is_some() {
        base.calendar_days_in_first_week = overlay.calendar_days_in_first_week;
    }
    if overlay.calendar_first_day_of_week.is_some() {
        base.calendar_first_day_of_week = overlay.calendar_first_day_of_week;
    }
    if overlay.calendar_time_zone.is_some() {
        base.calendar_time_zone = overlay.calendar_time_zone;
        base.calendar_time_zone_defined = overlay.calendar_time_zone_defined;
    }
    if overlay.text_number_pattern.is_some() {
        base.text_number_pattern = overlay.text_number_pattern;
    }
    if overlay.text_number_check_policy.is_some() {
        base.text_number_check_policy = overlay.text_number_check_policy;
    }
    if overlay.text_number_rep.is_some() {
        base.text_number_rep = overlay.text_number_rep;
    }
    if overlay.text_zoned_sign_style.is_some() {
        base.text_zoned_sign_style = overlay.text_zoned_sign_style;
    }
    if overlay.text_standard_decimal_separator.is_some() {
        base.text_standard_decimal_separator = overlay.text_standard_decimal_separator;
    }
    if overlay.text_standard_decimal_separator_sibling.is_some() {
        base.text_standard_decimal_separator_sibling =
            overlay.text_standard_decimal_separator_sibling;
    }
    if overlay.text_standard_grouping_separator.is_some() {
        base.text_standard_grouping_separator = overlay.text_standard_grouping_separator;
    }
    if overlay.text_standard_grouping_separator_sibling.is_some() {
        base.text_standard_grouping_separator_sibling =
            overlay.text_standard_grouping_separator_sibling;
    }
    if overlay.text_standard_exponent_rep.is_some() {
        base.text_standard_exponent_rep = overlay.text_standard_exponent_rep;
    }
    if overlay.text_standard_exponent_rep_sibling.is_some() {
        base.text_standard_exponent_rep_sibling = overlay.text_standard_exponent_rep_sibling;
    }
    if overlay.text_standard_infinity_rep.is_some() {
        base.text_standard_infinity_rep = overlay.text_standard_infinity_rep;
    }
    if overlay.text_standard_nan_rep.is_some() {
        base.text_standard_nan_rep = overlay.text_standard_nan_rep;
    }
    if overlay.text_standard_zero_rep.is_some() {
        base.text_standard_zero_rep = overlay.text_standard_zero_rep;
    }
    if overlay.text_number_rounding.is_some() {
        base.text_number_rounding = overlay.text_number_rounding;
    }
    if overlay.text_number_rounding_increment.is_some() {
        base.text_number_rounding_increment = overlay.text_number_rounding_increment;
    }
    if overlay.text_number_rounding_mode.is_some() {
        base.text_number_rounding_mode = overlay.text_number_rounding_mode;
    }
    if overlay.initiator.is_some() {
        base.initiator = overlay.initiator;
    }
    if overlay.raw_initiator.is_some() {
        base.raw_initiator = overlay.raw_initiator;
    }
    if overlay.initiator_percent_escaped {
        base.initiator_percent_escaped = true;
    }
    if overlay.terminator.is_some() {
        base.terminator = overlay.terminator;
    }
    if overlay.raw_terminator.is_some() {
        base.raw_terminator = overlay.raw_terminator;
    }
    if overlay.separator.is_some() {
        base.separator = overlay.separator;
    }
    if overlay.raw_separator.is_some() {
        base.raw_separator = overlay.raw_separator;
    }
    if overlay.output_new_line.is_some() {
        base.output_new_line = overlay.output_new_line;
    }
    if overlay.output_new_line_sibling.is_some() {
        base.output_new_line_sibling = overlay.output_new_line_sibling;
    }
    if overlay.occurs_min.is_some() {
        base.occurs_min = overlay.occurs_min;
    }
    if overlay.max_occurs_specified {
        base.occurs_max = overlay.occurs_max;
        base.max_occurs_specified = true;
    } else if overlay.occurs_max.is_some() {
        base.occurs_max = overlay.occurs_max;
    }
    if overlay.choice_dispatch_key.is_some() {
        base.choice_dispatch_key = overlay.choice_dispatch_key;
    }
    if overlay.choice_dispatch_sibling.is_some() {
        base.choice_dispatch_sibling = overlay.choice_dispatch_sibling;
    }
    if overlay.choice_dispatch_path.is_some() {
        base.choice_dispatch_path = overlay.choice_dispatch_path;
    }
    if overlay.choice_dispatch_literal.is_some() {
        base.choice_dispatch_literal = overlay.choice_dispatch_literal;
    }
    if overlay.choice_dispatch_sibling_int.is_some() {
        base.choice_dispatch_sibling_int = overlay.choice_dispatch_sibling_int;
    }
    if overlay.choice_branch_key.is_some() {
        base.choice_branch_key = overlay.choice_branch_key;
    }
    if overlay.length_pattern.is_some() {
        base.length_pattern = overlay.length_pattern;
    }
    if overlay.test_pattern.is_some() {
        base.test_pattern = overlay.test_pattern;
    }
    if overlay.separator_position.is_some() {
        base.separator_position = overlay.separator_position;
    }
    if overlay.text_boolean_true_rep_defined {
        base.text_boolean_true_rep = overlay.text_boolean_true_rep;
        base.text_boolean_true_rep_defined = true;
    }
    if overlay.text_boolean_false_rep_defined {
        base.text_boolean_false_rep = overlay.text_boolean_false_rep;
        base.text_boolean_false_rep_defined = true;
    }
    if overlay.text_boolean_pad_character.is_some() {
        base.text_boolean_pad_character = overlay.text_boolean_pad_character;
    }
    if overlay.binary_boolean_true_rep_defined {
        base.binary_boolean_true_rep = overlay.binary_boolean_true_rep;
        base.binary_boolean_true_rep_defined = true;
    }
    if overlay.binary_boolean_false_rep_defined {
        base.binary_boolean_false_rep = overlay.binary_boolean_false_rep;
        base.binary_boolean_false_rep_defined = true;
    }
    if overlay.default_value.is_some() {
        base.default_value = overlay.default_value;
    }
    if overlay.alignment.is_some() {
        base.alignment = overlay.alignment;
    }
    if overlay.alignment_implicit.is_some() {
        base.alignment_implicit = overlay.alignment_implicit;
    }
    if overlay.alignment_manual.is_some() {
        base.alignment_manual = overlay.alignment_manual;
    }
    if overlay.alignment_units.is_some() {
        base.alignment_units = overlay.alignment_units;
    }
    if overlay.leading_skip.is_some() {
        base.leading_skip = overlay.leading_skip;
    }
    if overlay.trailing_skip.is_some() {
        base.trailing_skip = overlay.trailing_skip;
    }
    if overlay.sequence_kind.is_some() {
        base.sequence_kind = overlay.sequence_kind;
    }
    if overlay.choice_length_kind.is_some() {
        base.choice_length_kind = overlay.choice_length_kind;
    }
    if overlay.choice_length.is_some() {
        base.choice_length = overlay.choice_length;
    }
    if overlay.fill_byte.is_some() {
        base.fill_byte = overlay.fill_byte;
    }
    if overlay.fill_byte_raw.is_some() {
        base.fill_byte_raw = overlay.fill_byte_raw;
    }
    if overlay.format_ref.is_some() {
        base.format_ref = overlay.format_ref;
    }
    if overlay.has_short_and_long_ref_overlap {
        base.has_short_and_long_ref_overlap = true;
    }
    if overlay.invalid_annotation_element {
        base.invalid_annotation_element = true;
    }
    if overlay.invalid_annotation_target.is_some() {
        base.invalid_annotation_target = overlay.invalid_annotation_target.clone();
    }
    if overlay.prefix_length_type.is_some() {
        base.prefix_length_type = overlay.prefix_length_type;
    }
    if overlay.prefix_includes_prefix_length.is_some() {
        base.prefix_includes_prefix_length = overlay.prefix_includes_prefix_length;
    }
    if overlay.input_value_calc.is_some() {
        base.input_value_calc = overlay.input_value_calc;
    }
    if overlay.input_value_calc_literal.is_some() {
        base.input_value_calc_literal = overlay.input_value_calc_literal;
    }
    if overlay.input_value_calc_sibling.is_some() {
        base.input_value_calc_sibling = overlay.input_value_calc_sibling;
    }
    if overlay.input_value_calc_segments.is_some() {
        base.input_value_calc_segments = overlay.input_value_calc_segments;
    }
    if overlay.output_value_calc.is_some() {
        base.output_value_calc = overlay.output_value_calc;
    }
    if overlay.output_value_calc_literal.is_some() {
        base.output_value_calc_literal = overlay.output_value_calc_literal;
    }
    if overlay.output_value_calc_sibling.is_some() {
        base.output_value_calc_sibling = overlay.output_value_calc_sibling;
    }
    if overlay.output_value_calc_conditional {
        base.output_value_calc_conditional = true;
    }
    if overlay.text_string_justification.is_some() {
        base.text_string_justification = overlay.text_string_justification;
    }
    if overlay.text_number_justification.is_some() {
        base.text_number_justification = overlay.text_number_justification;
    }
    if overlay.text_standard_base.is_some() {
        base.text_standard_base = overlay.text_standard_base;
    }
    if overlay.nillable.is_some() {
        base.nillable = overlay.nillable;
    }
    if overlay.nil_kind.is_some() {
        base.nil_kind = overlay.nil_kind;
    }
    if overlay.nil_value.is_some() {
        base.nil_value = overlay.nil_value;
    }
    if overlay.separator_suppression_policy.is_some() {
        base.separator_suppression_policy = overlay.separator_suppression_policy;
    }
    if overlay.empty_element_parse_policy.is_some() {
        base.empty_element_parse_policy = overlay.empty_element_parse_policy;
    }
    if overlay.occurs_count_kind.is_some() {
        base.occurs_count_kind = overlay.occurs_count_kind;
    }
    if overlay.occurs_count_expr.is_some() {
        base.occurs_count_expr = overlay.occurs_count_expr;
    }
    if overlay.occurs_count_fn_path.is_some() {
        base.occurs_count_fn_path = overlay.occurs_count_fn_path;
    }
    if overlay.hidden_group_ref.is_some() {
        base.hidden_group_ref = overlay.hidden_group_ref;
    }
    if overlay.hidden_group_ref_from_appinfo_sequence {
        base.hidden_group_ref_from_appinfo_sequence = true;
    }
    if overlay.ignore_case.is_some() {
        base.ignore_case = overlay.ignore_case;
    }
    if overlay.initiated_content.is_some() {
        base.initiated_content = overlay.initiated_content;
    }
    if overlay.has_statement_annotation {
        base.has_statement_annotation = true;
    }
    if overlay.has_multiple_discriminators {
        base.has_multiple_discriminators = true;
    }
    if overlay.has_discriminator_and_assert {
        base.has_discriminator_and_assert = true;
    }
    if overlay.assert_message.is_some() {
        base.assert_message = overlay.assert_message;
    }
    if overlay.assert_message_segments.is_some() {
        base.assert_message_segments = overlay.assert_message_segments;
    }
    if overlay.facet_check_constraints {
        base.facet_check_constraints = true;
    }
    if overlay.assert_int_eq.is_some() {
        base.assert_int_eq = overlay.assert_int_eq;
    }
    if overlay.assert_eq_occurs_index_addend.is_some() {
        base.assert_eq_occurs_index_addend = overlay.assert_eq_occurs_index_addend;
    }
    if overlay.is_discriminator {
        base.is_discriminator = true;
    }
    if overlay.discriminator_test.is_some() {
        base.discriminator_test = overlay.discriminator_test;
        base.assert_recoverable_error = overlay.assert_recoverable_error;
    }
    if overlay.assert_recoverable_error {
        base.assert_recoverable_error = true;
    }
    if overlay.discriminator_xpath_prefixes.is_some() {
        base.discriminator_xpath_prefixes = overlay.discriminator_xpath_prefixes;
    }
    if overlay.has_test_attr_and_body {
        base.has_test_attr_and_body = true;
    }
    if overlay.has_test_pat_attr_and_body {
        base.has_test_pat_attr_and_body = true;
    }
    if overlay.has_test_and_test_pattern {
        base.has_test_and_test_pattern = true;
    }
    if overlay.has_multiple_discriminators {
        base.has_multiple_discriminators = true;
    }
    if overlay.has_discriminator_and_assert {
        base.has_discriminator_and_assert = true;
    }
    if overlay.has_empty_test_pattern {
        base.has_empty_test_pattern = true;
    }
    if overlay.object_kind.is_some() {
        base.object_kind = overlay.object_kind;
    }
    if overlay.parse_unparse_policy.is_some() {
        base.parse_unparse_policy = overlay.parse_unparse_policy;
    }
    if overlay.input_value_calc_path.is_some() {
        base.input_value_calc_path = overlay.input_value_calc_path;
    }
    if overlay.output_value_calc_path.is_some() {
        base.output_value_calc_path = overlay.output_value_calc_path;
    }
    if overlay.output_value_calc_path_addend.is_some() {
        base.output_value_calc_path_addend = overlay.output_value_calc_path_addend;
    }
    if overlay.output_value_calc_scale.is_some() {
        base.output_value_calc_scale = overlay.output_value_calc_scale;
    }
    if overlay.output_value_calc_segments.is_some() {
        base.output_value_calc_segments = overlay.output_value_calc_segments;
    }
    if overlay.input_value_calc_expression.is_some() {
        base.input_value_calc_expression = overlay.input_value_calc_expression;
    }
    if overlay.text_bidi.is_some() {
        base.text_bidi = overlay.text_bidi;
    }
    if overlay.floating.is_some() {
        base.floating = overlay.floating;
    }
    if overlay.escape_scheme_ref.is_some() {
        base.escape_scheme_ref = overlay.escape_scheme_ref;
    }
    if overlay.suppress_schema_definition_warnings.is_some() {
        base.suppress_schema_definition_warnings = overlay.suppress_schema_definition_warnings;
    }
    if overlay.has_duplicate_variable_value_spec {
        base.has_duplicate_variable_value_spec = true;
    }
    if !overlay.set_variables.is_empty() {
        base.set_variables.extend(overlay.set_variables);
    }
    if !overlay.new_variable_instances.is_empty() {
        base.new_variable_instances
            .extend(overlay.new_variable_instances);
    }
    base
}
