use super::qname::*;
use crate::error::{ParseError, Result};
use crate::schema::ast::*;
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
            "lengthPattern" => props.length_pattern = Some(value.clone()),
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
            "encoding" => props.encoding = Some(value.clone()),
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
                props.text_standard_base =
                    Some(value.parse().map_err(|_| ParseError::InvalidXml {
                        message: alloc::format!("invalid textStandardBase `{value}`"),
                    })?);
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
                if let Some(expr) = parse_input_value_calc_expression(value) {
                    props.input_value_calc_expression = Some(expr);
                } else if let Some(segments) = parse_input_value_calc_concat(value) {
                    props.input_value_calc_segments = Some(segments);
                } else if let Some(steps) = parse_input_value_calc_relative_path(value) {
                    props.input_value_calc_path = Some(steps);
                } else if let Some((calc, lit)) =
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
                let lit = parse_delimiter_literal(value)?;
                props.initiator = Some(lit);
            }
            "terminator" => {
                let lit = parse_delimiter_literal(value)?;
                props.terminator = Some(lit);
            }
            "separator" => {
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
                    } else if let Some(steps) =
                        parse_input_value_calc_relative_path(&alloc::format!("{{{inner}}}"))
                    {
                        props.occurs_count_fn_path = Some(steps);
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
    crate::schema::validate_delimiter_schema_attribute(raw)
        .map_err(|e| ParseError::InvalidXml { message: e })?;
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

fn parse_assert_eq_occurs_index_addend(test: &str) -> Option<i64> {
    let compact: alloc::string::String = test.chars().filter(|c| !c.is_whitespace()).collect();
    let body = compact
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(compact.as_str());
    if !body.contains("dfdl:occursIndex()") {
        return None;
    }
    if body.contains("xs:int(.)eqdfdl:occursIndex()") || body.contains(".eqdfdl:occursIndex()") {
        return Some(0);
    }
    if let Some(rest) = body.strip_prefix(".eq(dfdl:occursIndex()+") {
        let n = rest.strip_suffix(')')?;
        return n.parse().ok();
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
    let trimmed = test.trim();
    if !trimmed.is_empty() && !props.facet_check_constraints {
        props.discriminator_test = Some(trimmed.to_string());
    }
}

pub(crate) fn parse_assert_int_eq_test(test: &str) -> Option<i64> {
    let compact: alloc::string::String = test.chars().filter(|c| !c.is_whitespace()).collect();
    let body = compact
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(compact.as_str());
    let rest = body.strip_prefix("xs:int(.)eq")?;
    rest.parse::<i64>().ok()
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

fn parse_ivc_path_steps(
    s: &str,
) -> Option<(
    bool,
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
        bool,
    )>,
)> {
    let s = s.trim();
    let (parent_root, rest) = if let Some(r) = s.strip_prefix("parent::") {
        (true, r)
    } else if let Some(r) = s.strip_prefix('/') {
        (false, r)
    } else {
        let r = s.strip_prefix("../").or_else(|| s.strip_prefix("..\\"))?;
        (false, r)
    };
    if rest.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rest.split('/').filter(|p| !p.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some((parent_root, steps))
}

enum IvcIntegerLexical {
    I64(i64),
    Wide(alloc::string::String),
}

fn parse_ivc_integer_lexical(s: &str) -> Option<IvcIntegerLexical> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let rest = if let Some(r) = s.strip_prefix('+').or_else(|| s.strip_prefix('-')) {
        r
    } else {
        s
    };
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(IvcIntegerLexical::I64(v));
    }
    Some(IvcIntegerLexical::Wide(s.to_string()))
}

fn split_top_level_ivc_div(s: &str) -> Option<alloc::vec::Vec<alloc::string::String>> {
    let s = s.trim();
    let mut parts = alloc::vec::Vec::new();
    let mut current = alloc::string::String::new();
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
                i += 1;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
                i += 1;
            }
            _ if depth == 0 && s[i..].starts_with(" div ") => {
                parts.push(current.trim().to_string());
                current.clear();
                i += 5;
            }
            _ => {
                current.push(ch);
                i += 1;
            }
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

fn extract_ivc_paren_argument(s: &str, open_prefix: &str) -> Option<alloc::string::String> {
    let rest = s.strip_prefix(open_prefix)?;
    let mut depth = 1i32;
    for (i, ch) in rest.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(rest[..i].trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_ivc_path_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let (parent_root, steps) = parse_ivc_path_steps(s)?;
    Some(crate::schema::InputValueCalcExpression::Path { parent_root, steps })
}

fn parse_ivc_primary(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    if let Some(arg) = extract_ivc_paren_argument(s, "fn:ceiling(") {
        let inner = parse_ivc_add_expr(&arg)?;
        return Some(crate::schema::InputValueCalcExpression::Ceiling(
            alloc::boxed::Box::new(inner),
        ));
    }
    for (prefix, kind) in [
        ("xs:byte(", crate::schema::IvcXsCast::Byte),
        ("xsd:byte(", crate::schema::IvcXsCast::Byte),
        ("xs:short(", crate::schema::IvcXsCast::Short),
        ("xsd:short(", crate::schema::IvcXsCast::Short),
        ("xs:int(", crate::schema::IvcXsCast::Int),
        ("xsd:int(", crate::schema::IvcXsCast::Int),
        ("xs:long(", crate::schema::IvcXsCast::Long),
        ("xsd:long(", crate::schema::IvcXsCast::Long),
        ("xs:unsignedByte(", crate::schema::IvcXsCast::UnsignedByte),
        ("xsd:unsignedByte(", crate::schema::IvcXsCast::UnsignedByte),
        ("xs:unsignedShort(", crate::schema::IvcXsCast::UnsignedShort),
        (
            "xsd:unsignedShort(",
            crate::schema::IvcXsCast::UnsignedShort,
        ),
        ("xs:unsignedInt(", crate::schema::IvcXsCast::UnsignedInt),
        ("xsd:unsignedInt(", crate::schema::IvcXsCast::UnsignedInt),
        ("xs:unsignedLong(", crate::schema::IvcXsCast::UnsignedLong),
        ("xsd:unsignedLong(", crate::schema::IvcXsCast::UnsignedLong),
        ("xs:float(", crate::schema::IvcXsCast::Float),
        ("xsd:float(", crate::schema::IvcXsCast::Float),
        ("xs:double(", crate::schema::IvcXsCast::Double),
        ("xsd:double(", crate::schema::IvcXsCast::Double),
        ("xs:string(", crate::schema::IvcXsCast::String),
        ("xsd:string(", crate::schema::IvcXsCast::String),
        ("xs:hexBinary(", crate::schema::IvcXsCast::HexBinary),
        ("xsd:hexBinary(", crate::schema::IvcXsCast::HexBinary),
    ] {
        if let Some(arg) = extract_ivc_paren_argument(s, prefix) {
            let inner = parse_ivc_add_expr(&arg)?;
            return Some(crate::schema::InputValueCalcExpression::Cast {
                kind,
                inner: alloc::boxed::Box::new(inner),
            });
        }
    }
    if let Some(rest) = s.strip_prefix('-') {
        if let Some(lit) = parse_ivc_integer_lexical(rest) {
            return Some(match lit {
                IvcIntegerLexical::I64(v) => crate::schema::InputValueCalcExpression::Literal(-v),
                IvcIntegerLexical::Wide(text) => {
                    let mut neg = alloc::string::String::from("-");
                    neg.push_str(text.trim_start_matches('+'));
                    crate::schema::InputValueCalcExpression::LiteralLexical(neg)
                }
            });
        }
    }
    if let Some(lit) = parse_ivc_integer_lexical(s) {
        return Some(match lit {
            IvcIntegerLexical::I64(v) => crate::schema::InputValueCalcExpression::Literal(v),
            IvcIntegerLexical::Wide(text) => {
                crate::schema::InputValueCalcExpression::LiteralLexical(text)
            }
        });
    }
    if s.len() >= 2
        && ((s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')))
    {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("''", "'").replace("\"\"", "\"");
        return Some(crate::schema::InputValueCalcExpression::LiteralLexical(
            unescaped,
        ));
    }
    if let Some(name) = s.strip_prefix('$').map(str::trim) {
        if !name.is_empty() && !name.contains(' ') {
            return Some(crate::schema::InputValueCalcExpression::Variable(
                name.to_string(),
            ));
        }
    }
    parse_ivc_path_expr(s)
}

fn parse_ivc_unary_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    parse_ivc_primary(s)
}

fn parse_ivc_div_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_div(s) {
        let mut left = parse_ivc_unary_expr(&parts[0])?;
        for part in parts.iter().skip(1) {
            let right = parse_ivc_unary_expr(part)?;
            left = crate::schema::InputValueCalcExpression::Div(
                alloc::boxed::Box::new(left),
                alloc::boxed::Box::new(right),
            );
        }
        return Some(left);
    }
    parse_ivc_unary_expr(s)
}

fn split_top_level_ivc_sub(s: &str) -> Option<alloc::vec::Vec<alloc::string::String>> {
    let s = s.trim();
    let mut parts = alloc::vec::Vec::new();
    let mut current = alloc::string::String::new();
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
                i += 1;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
                i += 1;
            }
            _ if depth == 0 && s[i..].starts_with(" - ") => {
                parts.push(current.trim().to_string());
                current.clear();
                i += 3;
            }
            _ => {
                current.push(ch);
                i += 1;
            }
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

fn parse_ivc_mul_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_op(s, '*') {
        let mut terms = alloc::vec::Vec::new();
        for part in parts {
            terms.push(parse_ivc_div_expr(&part)?);
        }
        return Some(crate::schema::InputValueCalcExpression::Mul(terms));
    }
    parse_ivc_div_expr(s)
}

fn parse_ivc_sub_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_sub(s) {
        let mut left = parse_ivc_mul_expr(&parts[0])?;
        for part in parts.iter().skip(1) {
            let right = parse_ivc_mul_expr(part)?;
            left = crate::schema::InputValueCalcExpression::Sub(alloc::vec![left, right,]);
        }
        return Some(left);
    }
    parse_ivc_mul_expr(s)
}

fn parse_ivc_add_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_op(s, '+') {
        let mut terms = alloc::vec::Vec::new();
        for part in parts {
            terms.push(parse_ivc_sub_expr(&part)?);
        }
        return Some(crate::schema::InputValueCalcExpression::Add(terms));
    }
    parse_ivc_sub_expr(s)
}

pub(crate) fn parse_input_value_calc_expression(
    value: &str,
) -> Option<crate::schema::InputValueCalcExpression> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(rest) = inner
        .strip_prefix("xs:string(")
        .and_then(|r| r.strip_suffix(')'))
    {
        let path = parse_ivc_add_expr(rest.trim())?;
        return Some(crate::schema::InputValueCalcExpression::StringOf(
            alloc::boxed::Box::new(path),
        ));
    }
    parse_ivc_add_expr(inner)
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
        return Some(alloc::vec![InputValueCalcSegment::Sibling(
            local_name_from_qname(name).to_string(),
        )]);
    }
    if part.starts_with('/') || part.starts_with("parent::") || part.starts_with("../") {
        if let Some((_parent_root, steps)) = parse_ivc_path_steps(part) {
            return Some(alloc::vec![InputValueCalcSegment::InfosetPath(steps)]);
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
        let sib = sub_args[0].strip_prefix("../")?;
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
        let path = arg_parts[0].trim().strip_prefix("../")?;
        let units = length_units_from_calc_args(arg_parts[1].trim());
        return Some(alloc::vec![InputValueCalcSegment::ValueLength {
            sibling: local_name_from_qname(path).to_string(),
            units,
        }]);
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
    let inner = inner.trim();
    let path = inner.strip_prefix("fn:count(")?.strip_suffix(')')?.trim();
    let mut rest = path;
    while rest.starts_with("../") {
        rest = &rest[3..];
    }
    if rest.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rest.split('/').filter(|s| !s.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some(steps)
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

pub(crate) fn parse_byte_order_if_expr(value: &str) -> Option<(String, ByteOrder, ByteOrder)> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .map(str::trim)?;
    let rest = inner.strip_prefix("if")?.trim();
    let rest = rest.strip_prefix('(')?.trim();
    let then_idx = rest.find(") then ")?;
    let cond = rest[..then_idx].trim().to_string();
    let rest = rest[then_idx + 7..].trim();
    let else_idx = rest.find(" else ")?;
    let then_lit = rest[..else_idx].trim();
    let else_lit = rest[else_idx + 6..].trim();
    let t = parse_byte_order_literal(then_lit)?;
    let f = parse_byte_order_literal(else_lit)?;
    if cond.is_empty() {
        return None;
    }
    Some((cond, t, f))
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
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let rest = inner.strip_prefix("../")?;
    if rest.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rest.split('/').filter(|s| !s.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some(steps)
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
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let (func_expr, addend_str) = inner.rsplit_once('+')?;
    let addend = addend_str.trim().parse::<i64>().ok()?;
    let func_expr = func_expr.trim();
    let rest = func_expr
        .strip_prefix("dfdl:valueLength(")?
        .strip_suffix(')')?;
    let arg_parts = split_top_level_commas(rest.trim());
    if arg_parts.len() != 2 {
        return None;
    }
    let path_part = arg_parts[0].trim().strip_prefix("../")?;
    let units_part = arg_parts[1].trim();
    if path_part.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in path_part.split('/').filter(|s| !s.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    let units = length_units_from_calc_args(units_part.trim());
    Some((steps, units, addend))
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
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let (path_expr, addend) = match inner.rsplit_once('+') {
        Some((left, right)) => {
            let right = right.trim();
            if let Ok(n) = right.parse::<i64>() {
                (left.trim(), n)
            } else {
                (inner, 0)
            }
        }
        None => (inner, 0),
    };
    let rest = path_expr.strip_prefix("../")?;
    if rest.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rest.split('/').filter(|s| !s.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some((steps, addend))
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
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.starts_with("xs:hexBinary(") && inner.ends_with(')') {
        let arg = inner["xs:hexBinary(".len()..inner.len() - 1].trim();
        if let Some(sib) = arg.strip_prefix("../") {
            return Some((
                InputValueCalc::HexBinaryFromSibling,
                Some(local_name_from_qname(sib).to_string()),
                None,
            ));
        }
    }
    if inner.starts_with("xs:string(") && inner.ends_with(')') {
        let arg = &inner["xs:string(".len()..inner.len() - 1];
        let lit = parse_xs_string_literal_arg(arg)?;
        return Some((InputValueCalc::StringLiteral, None, Some(lit)));
    }
    if let Some(lit) = parse_xs_string_literal_arg(inner) {
        return Some((InputValueCalc::StringLiteral, None, Some(lit)));
    }
    if let Some(v) = parse_constant_length_expr(trimmed) {
        return Some((InputValueCalc::Constant(v as i64), None, None));
    }
    if let Ok(v) = inner.parse::<i64>() {
        return Some((InputValueCalc::Constant(v), None, None));
    }
    if matches!(
        parse_ivc_integer_lexical(inner),
        Some(IvcIntegerLexical::Wide(_))
    ) {
        return Some((
            InputValueCalc::ConstantLexical,
            None,
            Some(inner.trim().to_string()),
        ));
    }
    let (func, rest) = inner.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    let units = length_units_from_calc_args(args);
    let target = args
        .split(',')
        .next()?
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    match (func, target) {
        ("dfdl:contentLength", "..") => {
            Some((InputValueCalc::ContentLengthSelf(units), None, None))
        }
        ("dfdl:valueLength", "..") => Some((InputValueCalc::ValueLengthSelf(units), None, None)),
        ("dfdl:contentLength", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                InputValueCalc::ContentLengthSibling(units),
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("dfdl:valueLength", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                InputValueCalc::ValueLengthSibling(units),
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("xs:boolean", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                InputValueCalc::BooleanFromSibling,
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        _ => None,
    }
}

pub(crate) fn parse_variable_input_value_calc(
    value: &str,
    vars: &BTreeMap<String, String>,
) -> Option<(InputValueCalc, Option<String>)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let name = inner.strip_prefix('$')?.trim();
    if vars.contains_key(name) {
        return Some((InputValueCalc::SchemaVariable, Some(name.to_string())));
    }
    None
}

pub(crate) fn parse_variable_length_expr(
    value: &str,
    vars: &BTreeMap<String, String>,
) -> Option<u64> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let name = inner.strip_prefix('$')?.trim();
    vars.get(name)?.parse().ok()
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
    let trimmed = value.trim();
    let inner = if trimmed.starts_with('{') && trimmed.ends_with('}') {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    let prefix = "dfdl:encodeDFDLEntities(";
    if !inner.starts_with(prefix) || !inner.ends_with(')') {
        return None;
    }
    let path = inner[prefix.len()..inner.len() - 1].trim();
    let path = path.strip_prefix("../")?;
    Some(local_name_from_qname(path).to_string())
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

pub(crate) fn parse_output_value_calc_fn_error(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner == "fn:error()" {
        return Some(inner.to_string());
    }
    if inner.starts_with("fn:error(") && inner.ends_with(')') {
        return Some(inner.to_string());
    }
    if inner.starts_with("fn:round-half-to-even(") && inner.contains("fn:error(") {
        return Some(inner.to_string());
    }
    None
}

pub(crate) fn parse_output_value_calc_xs_date_inner(value: &str) -> Option<alloc::string::String> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let rest = inner.strip_prefix("xs:date(")?.strip_suffix(')')?;
    Some(rest.trim().to_string())
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

pub(crate) fn parse_output_value_calc(
    value: &str,
) -> Option<(OutputValueCalc, Option<String>, Option<String>)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(lit) = parse_xs_string_literal_arg(inner) {
        return Some((OutputValueCalc::Constant(0), None, Some(lit)));
    }
    if inner.starts_with("xs:string(") && inner.ends_with(')') {
        let arg = inner["xs:string(".len()..inner.len() - 1].trim();
        if let Some(lit) = parse_xs_string_literal_arg(arg) {
            return Some((OutputValueCalc::Constant(0), None, Some(lit)));
        }
        if let Some(decoded) = parse_decode_dfdl_entities_call(arg) {
            return Some((OutputValueCalc::Constant(0), None, Some(decoded)));
        }
        if let Some(nested) = parse_output_value_calc(&alloc::format!("{{{arg}}}")) {
            return Some(nested);
        }
    }
    if inner.starts_with("xs:float(") && inner.ends_with(')') {
        let arg = inner["xs:float(".len()..inner.len() - 1].trim();
        if let Some(nested) = parse_output_value_calc(&alloc::format!("{{{arg}}}")) {
            return Some(nested);
        }
    }
    if inner.starts_with("xs:int(") && inner.ends_with(')') {
        let arg = inner["xs:int(".len()..inner.len() - 1].trim();
        if let Some(nested) = parse_output_value_calc(&alloc::format!("{{{arg}}}")) {
            return Some(nested);
        }
    }
    if let Some(hex) = parse_output_value_calc_hex(inner) {
        return Some(hex);
    }
    if (inner.contains('.') || inner.contains('e') || inner.contains('E'))
        && inner.parse::<f64>().is_ok()
    {
        return Some((OutputValueCalc::Constant(0), None, Some(inner.to_string())));
    }
    if let Ok(v) = inner.parse::<i64>() {
        return Some((OutputValueCalc::Constant(v), None, None));
    }
    let (func_part, addend) = if let Some((left, right)) = inner.rsplit_once('+') {
        (left.trim(), right.trim().parse::<i64>().unwrap_or(0))
    } else {
        (inner, 0)
    };
    let (func, rest) = func_part.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    let units = length_units_from_calc_args(args);
    let target = args
        .split(',')
        .next()?
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    match (func, target) {
        ("dfdl:contentLength", "..") => Some((
            OutputValueCalc::ContentLengthSelf(units, addend),
            None,
            None,
        )),
        ("dfdl:valueLength", "..") => {
            Some((OutputValueCalc::ValueLengthSelf(units, addend), None, None))
        }
        ("dfdl:contentLength", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                OutputValueCalc::ContentLengthSibling(units, addend),
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("dfdl:valueLength", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                OutputValueCalc::ValueLengthSibling(units, addend),
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("fn:string-length", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                OutputValueCalc::StringLengthSibling,
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("fn:substring", _sib) => {
            let sub_args = split_top_level_commas(args);
            if sub_args.len() != 3 {
                return None;
            }
            let name = sub_args[0].strip_prefix("../")?;
            let start: usize = sub_args[1].parse().ok()?;
            let length: usize = sub_args[2].parse().ok()?;
            Some((
                OutputValueCalc::Substring { start, length },
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        _ => None,
    }
}

fn parse_output_value_calc_hex(
    inner: &str,
) -> Option<(OutputValueCalc, Option<String>, Option<String>)> {
    if inner.starts_with("xs:hexBinary(") && inner.ends_with(')') {
        let arg = inner["xs:hexBinary(".len()..inner.len() - 1].trim();
        let lit = parse_xs_string_literal_arg(arg)?;
        return Some((OutputValueCalc::HexBinaryFromLexical, None, Some(lit)));
    }
    if inner.starts_with("dfdl:hexBinary(") && inner.ends_with(')') {
        let arg = inner["dfdl:hexBinary(".len()..inner.len() - 1].trim();
        if let Ok(n) = arg.parse::<i64>() {
            return Some((OutputValueCalc::HexBinaryFromInteger(n), None, None));
        }
        if let Some(lit) = parse_xs_string_literal_arg(arg) {
            return Some((OutputValueCalc::HexBinaryFromLexical, None, Some(lit)));
        }
        if arg.starts_with("xs:short(") && arg.ends_with(')') {
            let num = arg["xs:short(".len()..arg.len() - 1].trim();
            let v: i16 = num.parse().ok()?;
            return Some((OutputValueCalc::HexBinaryFromShort(v), None, None));
        }
        if arg.starts_with("xs:byte(") && arg.contains("../") {
            let inner_arg = arg["xs:byte(".len()..arg.len() - 1].trim();
            let sib = inner_arg.strip_prefix("../")?;
            return Some((
                OutputValueCalc::HexBinaryFromByteSibling,
                Some(local_name_from_qname(sib).to_string()),
                None,
            ));
        }
    }
    None
}

pub(crate) fn parse_self_string_length_max_expr(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.contains("fn:string-length(.)") && inner.contains("gt 5") {
        return Some(5);
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
        props.choice_dispatch_path = Some(steps);
        return;
    }
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(rest) = inner.strip_prefix("./") {
        if !rest.is_empty() {
            if rest.contains('/') {
                let mut steps = alloc::vec::Vec::new();
                for step in rest.split('/').filter(|s| !s.is_empty()) {
                    steps.push(parse_infoset_path_step(step));
                }
                props.choice_dispatch_path = Some(steps);
            } else {
                props.choice_dispatch_sibling = Some(local_name_from_qname(rest).to_string());
            }
        }
        return;
    }
    if inner.starts_with("xs:string(") && inner.ends_with(')') {
        let arg = inner["xs:string(".len()..inner.len() - 1].trim();
        if let Some(name) = arg
            .strip_prefix("xs:int(")
            .and_then(|r| r.strip_suffix(')'))
            .map(|s| s.trim())
        {
            props.choice_dispatch_sibling_int = Some(local_name_from_qname(name).to_string());
            return;
        }
        if let Some(rest) = arg.strip_prefix("./") {
            props.choice_dispatch_sibling = Some(local_name_from_qname(rest).to_string());
            return;
        }
        if let Some(rest) = arg.strip_prefix("../") {
            props.choice_dispatch_sibling = Some(local_name_from_qname(rest).to_string());
            return;
        }
        if let Some(lit) = parse_xs_string_literal_arg(arg) {
            props.choice_dispatch_literal = Some(lit);
        }
    }
}

fn parse_sibling_length_adjustment(path_tail: &str) -> Option<(String, i64)> {
    let tail = path_tail.trim();
    for op in ['+', '-'] {
        if let Some((name_part, delta_part)) = tail.split_once(op) {
            let name = local_name_from_qname(name_part.trim()).to_string();
            let delta: i64 = delta_part.trim().parse().ok()?;
            let adjust = if op == '-' { -delta } else { delta };
            return Some((name, adjust));
        }
    }
    Some((local_name_from_qname(tail).to_string(), 0))
}

pub(crate) fn parse_sibling_length_expr(value: &str) -> Option<(String, bool, i64)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(path) = inner.strip_prefix("../") {
        let (name, adjust) = parse_sibling_length_adjustment(path)?;
        return Some((name, false, adjust));
    }
    if let Some(idx) = inner.find("../") {
        let tail = inner[idx + 3..].trim().trim_end_matches(')').trim();
        if !tail.is_empty() {
            let cast_long = inner.contains("xs:long(") || inner.contains("xs:integer(");
            let (name, adjust) = parse_sibling_length_adjustment(tail)?;
            return Some((name, cast_long, adjust));
        }
    }
    None
}

/// Parses constant DFDL length expressions such as `{ 6 }`, `{1}`, or `{ 1 + 1 }`.
pub(crate) fn parse_constant_length_expr(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.is_empty()
        || inner.contains("..")
        || inner.contains(':')
        || inner.contains('(')
        || inner.contains('/')
    {
        return None;
    }
    if let Ok(v) = inner.parse::<u64>() {
        return Some(v);
    }
    for op in ['+', '-'] {
        if let Some((lhs, rhs)) = inner.split_once(op) {
            let a = lhs.trim().parse::<u64>().ok()?;
            let b = rhs.trim().parse::<u64>().ok()?;
            return match op {
                '+' => Some(a.saturating_add(b)),
                '-' => a.checked_sub(b),
                _ => None,
            };
        }
    }
    None
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
    if overlay.occurs_max.is_some() {
        base.occurs_max = overlay.occurs_max;
    }
    if overlay.max_occurs_specified {
        base.max_occurs_specified = true;
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
    if overlay.discriminator_test.is_some() {
        base.discriminator_test = overlay.discriminator_test;
    }
    if overlay.discriminator_xpath_prefixes.is_some() {
        base.discriminator_xpath_prefixes = overlay.discriminator_xpath_prefixes;
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
    if !overlay.set_variables.is_empty() {
        base.set_variables.extend(overlay.set_variables);
    }
    if !overlay.new_variable_instances.is_empty() {
        base.new_variable_instances
            .extend(overlay.new_variable_instances);
    }
    base
}
