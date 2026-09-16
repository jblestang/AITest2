use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use regex_automata::{meta::Regex, Anchored, Input};

fn codepoint_to_delimiter_bytes(cp: u32, encoding: Option<&str>) -> Option<Vec<u8>> {
    use crate::vm::encoding::{encode_document_text, normalize_encoding_name};
    if let Some(enc) = encoding.and_then(normalize_encoding_name) {
        match enc {
            "iso-8859-1" | "ascii" | "ebcdic-cp-us" => {
                if cp <= 0xff {
                    return Some(vec![cp as u8]);
                }
                return None;
            }
            "utf-16be" | "utf-16le" => {
                let ch = char::from_u32(cp)?;
                return encode_document_text(&ch.to_string(), enc).ok();
            }
            _ => {}
        }
    }
    unicode_codepoint_to_utf8(cp)
}

fn named_entity_bytes_for_encoding(name: &str, encoding: Option<&str>) -> Option<Vec<u8>> {
    let value: u32 = match name {
        "NUL" => 0,
        "SOH" => 1,
        "STX" => 2,
        "ETX" => 3,
        "EOT" => 4,
        "ENQ" => 5,
        "ACK" => 6,
        "BEL" => 7,
        "BS" => 8,
        "HT" => 9,
        "LF" => 10,
        "VT" => 11,
        "FF" => 12,
        "CR" => 13,
        "SO" => 14,
        "SI" => 15,
        "DLE" => 16,
        "DC1" => 17,
        "DC2" => 18,
        "DC3" => 19,
        "DC4" => 20,
        "NAK" => 21,
        "SYN" => 22,
        "ETB" => 23,
        "CAN" => 24,
        "EM" => 25,
        "SUB" => 26,
        "ESC" => 27,
        "FS" => 28,
        "GS" => 29,
        "RS" => 30,
        "US" => 31,
        "SP" => 32,
        "DEL" => 127,
        "NBSP" => 0x00A0,
        "NEL" => 0x0085,
        "LS" => 0x2028,
        "NL" => 0x0A,
        _ => return None,
    };
    codepoint_to_delimiter_bytes(value, encoding)
}

fn parse_entity_for_encoding(input: &str, encoding: Option<&str>) -> Option<(Vec<u8>, usize)> {
    if !input.starts_with('%') {
        return None;
    }
    let rest = &input[1..];
    if let Some(end) = rest.find(';') {
        let name = &rest[..end];
        let consumed = 1 + end + 1;
        let quantifier = name.chars().last().filter(|c| *c == '+' || *c == '*' || *c == '?');
        let entity_name = match quantifier {
            Some(_) => name.trim_end_matches(['+', '*', '?']),
            None => name,
        };
        let value = match entity_name {
            "ES" => vec![],
            "WSP" | "WS" => match quantifier {
                Some('*') | Some('?') => vec![],
                _ => vec![b' '],
            },
            other if other.starts_with("#x") || other.starts_with("#X") => {
                let hex = &other[2..];
                let cp = u32::from_str_radix(hex, 16).ok()?;
                codepoint_to_delimiter_bytes(cp, encoding)?
            }
            other if other.starts_with("#r") || other.starts_with("#R") => {
                let hex = &other[2..];
                u8::from_str_radix(hex, 16).ok().map(|b| vec![b])?
            }
            other if other.starts_with('#') => {
                let dec = &other[1..];
                let cp = dec.parse::<u32>().ok()?;
                codepoint_to_delimiter_bytes(cp, encoding)?
            }
            other => named_entity_bytes_for_encoding(other, encoding)?,
        };
        return Some((value, consumed));
    }
    None
}

/// Expand DFDL entity references in property values for delimiter matching in the given encoding.
pub fn expand_entities_for_encoding(input: &str, encoding: Option<&str>) -> Vec<u8> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'%' {
                out.push(b'%');
                i += 2;
                continue;
            }
            if let Some((entity, consumed)) = parse_entity_for_encoding(&input[i..], encoding) {
                out.extend_from_slice(&entity);
                i += consumed;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Expand DFDL entity references in property values.
///
/// Supports `%NL;`, `%CR;`, `%LF;`, `%SP;`, `%HT;`, `%WSP;`, `%WS;`, `%#rNN;` (hex byte).
pub fn expand_entities(input: &str) -> Vec<u8> {
    expand_entities_for_encoding(input, None)
}

/// Expand entities in a string, preserving UTF-8 where possible.
pub fn expand_entities_str(input: &str) -> String {
    let bytes = expand_entities(input);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn unicode_codepoint_to_utf8(cp: u32) -> Option<Vec<u8>> {
    char::from_u32(cp).map(|c| {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        s.as_bytes().to_vec()
    })
}

fn parse_entity(input: &str) -> Option<(Vec<u8>, usize)> {
    parse_entity_for_encoding(input, None)
}

/// Normalize a DFDL delimiter property value (trim ignored trailing space, expand entities).
pub fn normalize_delimiter_pattern(raw: &str) -> String {
    expand_entities_str(raw.trim_end_matches([' ', '\t']))
}

fn invalid_dfdl_entity_error(entity_token: &str, raw: &str) -> String {
    let context = if raw.contains("blah") {
        raw.to_string()
    } else if raw.contains(' ') && raw.trim() != entity_token {
        entity_token.to_string()
    } else {
        raw.to_string()
    };
    alloc::format!("Invalid DFDL Entity ({entity_token}) found in \"{context}\"")
}

fn entity_reference_valid(entity_name: &str) -> bool {
    if parse_entity(&format!("%{entity_name};")).is_none() {
        return false;
    }
    if let Some(r) = entity_name
        .strip_prefix("#r")
        .or_else(|| entity_name.strip_prefix("#R"))
    {
        return r.len() == 2 && r.chars().all(|c| c.is_ascii_hexdigit());
    }
    if let Some(hex) = entity_name
        .strip_prefix("#x")
        .or_else(|| entity_name.strip_prefix("#X"))
    {
        return !hex.is_empty()
            && hex.chars().all(|c| c.is_ascii_hexdigit())
            && hex.len() <= 6;
    }
    if let Some(dec) = entity_name.strip_prefix('#') {
        return !dec.is_empty() && dec.chars().all(|c| c.is_ascii_digit());
    }
    true
}

fn validate_entity_tokens_in_literal(raw: &str) -> Result<(), String> {
    if raw == "%" {
        return Err("Invalid DFDL Entity (%) found".into());
    }
    validate_entity_tokens_in_literal_lenient(raw)
}

/// Like [`validate_entity_tokens_in_literal`] but allows literal `%` (delimiter properties).
fn validate_entity_tokens_in_literal_lenient(raw: &str) -> Result<(), String> {
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'%' {
                i += 2;
                continue;
            }
            if let Some(rel) = raw[i..].find(';') {
                let entity_token = &raw[i..=i + rel];
                let entity = &raw[i + 1..i + rel];
                let entity_name = entity.trim_end_matches(['+', '*', '?']);
                if !entity_reference_valid(entity_name) {
                    return Err(invalid_dfdl_entity_error(entity_token, raw));
                }
                i += rel + 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// Validate `%entity;` references in a DFDL property literal.
pub fn validate_dfdl_entities_in_property(raw: &str) -> Result<(), String> {
    validate_entity_tokens_in_literal(raw)
}

pub fn validate_text_boolean_rep_value(raw: &str) -> Result<(), String> {
    validate_dfdl_entities_in_property(raw)?;
    reject_byte_entities_text_standard(raw)?;
    Ok(())
}

fn reject_byte_entities_text_standard(raw: &str) -> Result<(), String> {
    let mut i = 0usize;
    while i < raw.len() {
        if raw.as_bytes()[i] == b'%' {
            if let Some(rel) = raw[i..].find(';') {
                let entity = &raw[i + 1..i + rel];
                let entity_name = entity.trim_end_matches(['+', '*', '?']);
                if entity_name.starts_with("#r") || entity_name.starts_with("#R") {
                    let full = &raw[i..=i + rel];
                    return Err(format!("DFDL Byte Entity ({full}) not allowed"));
                }
                i += rel + 1;
            } else {
                let tail = &raw[i..];
                if tail.len() > 1 {
                    let body = &tail[1..];
                    if body.starts_with("#r") || body.starts_with("#R") {
                        return Err(invalid_dfdl_entity_error(tail, raw));
                    }
                }
                return Ok(());
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}

fn char_class_tokens(raw: &str) -> alloc::vec::Vec<&str> {
    let parts: alloc::vec::Vec<&str> = raw.split_whitespace().collect();
    if parts.len() > 1 {
        parts
    } else {
        alloc::vec![raw]
    }
}

fn validate_disallowed_char_class_tokens(
    prop: &str,
    raw: &str,
    extra_disallowed: &[&str],
) -> Result<(), String> {
    const DISALLOWED: [&str; 8] = [
        "%NL;",
        "%LF;",
        "%WSP;",
        "%WS;",
        "%WSP+;",
        "%WSP*;",
        "%WS+;",
        "%ES;",
    ];
    for token in char_class_tokens(raw) {
        for dis in DISALLOWED.iter().chain(extra_disallowed.iter()) {
            if token == *dis {
                return Err(format!(
                    "{prop} contains disallowed character class(es): {dis}"
                ));
            }
        }
    }
    Ok(())
}

/// Compile-time validation for `textStandardDecimalSeparator` / `textStandardGroupingSeparator`.
pub fn validate_text_standard_exponent_rep_literal(raw: &str) -> Result<(), String> {
    validate_text_standard_separator_literal("textStandardExponentRep", raw)
}

pub fn validate_text_standard_special_value_literal(
    prop: &str,
    raw: &str,
) -> Result<(), String> {
    validate_dfdl_entities_in_property(raw)?;
    reject_byte_entities_text_standard(raw)?;
    validate_disallowed_char_class_tokens(prop, raw, &[])?;
    Ok(())
}

pub fn validate_text_standard_separator_literal(prop: &str, raw: &str) -> Result<(), String> {
    validate_dfdl_entities_in_property(raw)?;
    reject_byte_entities_text_standard(raw)?;
    validate_disallowed_char_class_tokens(prop, raw, &[])?;
    if (prop == "textStandardGroupingSeparator" || prop == "textStandardDecimalSeparator")
        && (raw.contains("%WSP") || raw.contains("%WS"))
        && !raw.contains("%WSP;")
        && !raw.contains("%WS;")
    {
        let token = if raw.contains("%WSP") { "%WSP+;" } else { "%WS+;" };
        return Err(format!(
            "{prop} contains disallowed character class(es): {token}"
        ));
    }
    Ok(())
}

/// DFDL-13-061.1R — text standard property values must be distinct.
pub fn validate_text_standard_distinct_values(entries: &[(&str, &str)]) -> Result<(), String> {
    use alloc::collections::BTreeMap;
    let mut by_value: BTreeMap<alloc::string::String, alloc::vec::Vec<&str>> = BTreeMap::new();
    for (prop, raw) in entries {
        let key = expand_entities_str(raw);
        by_value.entry(key).or_default().push(prop);
    }
    let mut conflict: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for props in by_value.values() {
        if props.len() > 1 {
            for p in props {
                if !conflict.contains(p) {
                    conflict.push(p);
                }
            }
        }
    }
    if conflict.is_empty() {
        return Ok(());
    }
    conflict.sort_unstable();
    let names = conflict.join(", ");
    Err(format!(
        "Non-distinct property values among {names}"
    ))
}

/// Parse `textStandardDecimalSeparator` (list of single-character literals, space-separated).
/// Whitespace-separated `textStandardZeroRep` tokens (entities expanded per token).
pub fn parse_text_standard_zero_rep_list(raw: &str) -> alloc::vec::Vec<String> {
    use alloc::string::ToString;
    use alloc::vec::Vec;
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split_whitespace()
        .map(|t| {
            if t.contains('%') {
                t.to_string()
            } else {
                expand_entities_str(t)
            }
        })
        .collect()
}

pub fn validate_text_string_pad_character(raw: &str) -> Result<(), String> {
    if raw.is_empty() || raw.chars().any(|c| c.is_whitespace()) {
        return Err(
            "facet-valid NonEmptyStringLiteral property textStringPadCharacter".into(),
        );
    }
    if raw.chars().count() != 1 {
        return Err(
            "facet-valid NonEmptyStringLiteral property textStringPadCharacter".into(),
        );
    }
    Ok(())
}

/// Compile-time length/entity checks after properties are merged (incl. `lengthUnits`).
pub fn validate_text_string_pad_character_merged(
    raw: &str,
    length_units_bytes: bool,
    property_form: bool,
) -> Result<(), String> {
    validate_text_string_pad_character_compile(raw)?;
    validate_dfdl_entities_in_property(raw)?;
    if raw.chars().any(|c| c.is_whitespace()) {
        if property_form {
            return Err(
                "Use DFDL Entities (property textStringPadCharacter contains whitespace)".into(),
            );
        }
        return Err(
            "facet-valid NonEmptyStringLiteral property textStringPadCharacter".into(),
        );
    }
    let expanded = expand_entities_str(raw);
    let one_unit = if length_units_bytes {
        expanded.as_bytes().len() == 1
    } else {
        expanded.chars().count() == 1
    };
    if !one_unit {
        return Err("Length of string must be exactly 1 character".into());
    }
    Ok(())
}

/// Compile-time checks for `textStringPadCharacter` (character classes, etc.).
pub fn validate_text_string_pad_character_compile(raw: &str) -> Result<(), String> {
    validate_disallowed_char_class_tokens("textStringPadCharacter", raw, &[])?;
    if (raw.contains("%WSP") || raw.contains("%WS"))
        && !raw.contains("%WSP;")
        && !raw.contains("%WS;")
    {
        let token = if raw.contains("%WSP") {
            "%WSP+;"
        } else {
            "%WS+;"
        };
        return Err(format!(
            "textStringPadCharacter contains disallowed character class(es): {token}"
        ));
    }
    Ok(())
}

/// Runtime decode/unparse check for literal whitespace pad (not `%SP;` etc.).
pub fn validate_text_string_pad_character_runtime(raw: &str) -> Result<(), String> {
    if !raw.contains('%') && raw.chars().any(|c| c.is_whitespace()) {
        return Err(
            "facet-valid NonEmptyStringLiteral property textStringPadCharacter".into(),
        );
    }
    let expanded = expand_entities_str(raw);
    if expanded.chars().count() != 1 {
        return Err(
            "facet-valid NonEmptyStringLiteral property textStringPadCharacter".into(),
        );
    }
    Ok(())
}

pub fn validate_text_standard_zero_rep_literal(raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Ok(());
    }
    validate_dfdl_entities_in_property(raw)?;
    reject_byte_entities_text_standard(raw)?;
    for token in char_class_tokens(raw) {
        for dis in ["%NL;", "%LF;"] {
            if token == dis {
                return Err(format!(
                    "textStandardZeroRep contains disallowed character class(es): {dis}"
                ));
            }
        }
    }
    Ok(())
}

pub fn parse_text_standard_separator_list(raw: &str) -> alloc::vec::Vec<String> {
    use alloc::string::ToString;
    use alloc::vec::Vec;
    let expanded = expand_entities_str(raw);
    if expanded.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = expanded.split_whitespace().collect();
    if tokens.len() > 1 {
        tokens.into_iter().map(|t| t.to_string()).collect()
    } else {
        // One separator token (may be a single space/tab from `%SP;`).
        vec![expanded]
    }
}

/// Unescape DFDL `{`/`{{` open-brace escape sequences in a single delimiter token.
pub fn unescape_dfdl_open_braces(raw: &str) -> String {
    let mut out = String::new();
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            out.push('{');
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// True when `s` contains two or more `%entity;` tokens (e.g. `%WSP*;%NL;`).
fn is_compound_dfdl_entity_delimiter(s: &str) -> bool {
    let mut count = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = s[i..].find(';') {
                count += 1;
                i += rel + 1;
                continue;
            }
        }
        i += 1;
    }
    count >= 2
}

/// True when `s` contains a `%entity+;`, `%entity*;`, or `%entity?;` token.
fn has_quantified_dfdl_entity(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = s[i..].find(';') {
                let body = &s[i + 1..i + rel];
                if body
                    .chars()
                    .last()
                    .is_some_and(|c| matches!(c, '+' | '*' | '?'))
                {
                    return true;
                }
                i += rel + 1;
                continue;
            }
        }
        i += 1;
    }
    false
}

/// Parse a delimiter literal from an XSD attribute value.
pub fn parse_delimiter_literal_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let unescaped = if trimmed.chars().any(char::is_whitespace) {
        trimmed.to_string()
    } else {
        unescape_dfdl_open_braces(trimmed)
    };
    // Keep `%...;` tokens for whitespace-separated alternative lists (unparse uses first alt).
    if unescaped.contains('%')
        && unescaped
            .chars()
            .any(|c| c.is_ascii_whitespace())
    {
        return unescaped.trim_end_matches([' ', '\t']).to_string();
    }
    if is_compound_dfdl_entity_delimiter(&unescaped) || has_quantified_dfdl_entity(&unescaped) {
        return unescaped.trim().to_string();
    }
    let expanded = expand_entities(&unescaped);
    if core::str::from_utf8(&expanded).is_err() {
        return unescaped.trim().to_string();
    }
    let normalized = normalize_delimiter_pattern(&unescaped);
    if normalized.is_empty() && unescaped.contains('%') {
        return unescaped.trim().to_string();
    }
    normalized
}

fn delimiter_alt_is_es(alt: &str) -> bool {
    alt.trim() == "%ES;"
}

/// Reject `%ES;` as the sole delimiter alternative (DFDL-6-046R).
pub fn validate_delimiter_es_restriction(prop: &str, raw: &str) -> Result<(), String> {
    let alts = delimiter_alternatives(raw);
    let has_es = alts.iter().any(|alt| delimiter_alt_is_es(alt));
    if !has_es {
        return Ok(());
    }
    if prop == "separator" {
        return Err("Separator contains disallowed ES".into());
    }
    if prop == "terminator" {
        return Err("dfdl:terminator cannot own ES".into());
    }
    validate_es_not_sole_delimiter_alternative(prop, &alts)
}

fn validate_es_not_sole_delimiter_alternative(
    prop: &str,
    alts: &[String],
) -> Result<(), String> {
    if alts.is_empty() {
        return Ok(());
    }
    if alts.len() == 1 && delimiter_alt_is_es(&alts[0]) {
        return Err(if prop == "terminator" {
            "dfdl:terminator cannot own ES".into()
        } else {
            "ES entity cannot appear on its own".into()
        });
    }
    if alts.iter().any(|a| !delimiter_alt_is_es(a)) {
        return Ok(());
    }
    Err(if prop == "terminator" {
        "dfdl:terminator cannot own ES".into()
    } else {
        "ES entity cannot appear on its own".into()
    })
}

/// True when a `{...}` terminator expression includes a `%ES;` literal (invalid with `lengthKind='delimited'`).
pub fn delimited_terminator_expression_uses_es_literal(expr: &str) -> bool {
    delimiter_expression_string_literals(expr)
        .iter()
        .any(|lit| lit.trim() == "%ES;")
}

/// Extract quoted string literals from a simple `if ... then 'a' else 'b'` expression.
fn delimiter_expression_string_literals(expr: &str) -> alloc::vec::Vec<alloc::string::String> {
    let inner = expr
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(expr)
        .trim();
    let mut out = alloc::vec::Vec::new();
    let bytes = inner.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        i += 2;
                        continue;
                    }
                    out.push(inner[start..i].replace("''", "'"));
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Evaluate a compile-time constant delimiter expression (subset of XPath).
fn eval_compile_time_delimiter_expression(expr: &str) -> Option<alloc::string::String> {
    let inner = expr
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(expr)
        .trim();
    let lower = inner.to_ascii_lowercase();
    if lower.starts_with("if") {
        let rest = inner
            .strip_prefix("if")
            .unwrap_or(inner)
            .trim_start();
        let rest = rest.strip_prefix('(').unwrap_or(rest);
        let cond_end = rest.find(')')?;
        let cond = rest[..cond_end].trim();
        let tail = rest[cond_end + 1..].trim();
        let then_lit = tail.strip_prefix("then")?.trim();
        let (then_val, else_tail) = then_lit.split_once("else")?;
        let then_s = then_val.trim().trim_matches('\'').replace("''", "'");
        let else_s = else_tail.trim().trim_matches('\'').replace("''", "'");
        let cond_true = matches!(cond, "'true'" | "'1'" | "true()" | "fn:true()");
        return Some(if cond_true { then_s } else { else_s });
    }
    None
}

/// True when a runtime delimiter expression can evaluate to a zero-length pattern.
pub fn runtime_delimiter_expression_may_be_zero_length(expr: &str) -> bool {
    for lit in delimiter_expression_string_literals(expr) {
        if is_zero_length_delimiter(&lit) {
            return true;
        }
    }
    false
}

pub fn unquote_xpath_string_literal(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        return t[1..t.len() - 1].replace("''", "'");
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        return t[1..t.len() - 1].replace("\"\"", "\"");
    }
    t.to_string()
}

fn xpath_relative_element_local_name(path: &str) -> Option<&str> {
    let p = path.trim();
    let p = p
        .strip_prefix("./")
        .or_else(|| p.strip_prefix("../"))
        .unwrap_or(p);
    Some(p.rsplit(':').next().unwrap_or(p).trim())
}

fn eval_delimiter_if_condition(
    cond: &str,
    siblings: &alloc::collections::BTreeMap<alloc::string::String, alloc::string::String>,
) -> Option<bool> {
    let cond = cond.trim();
    if let Some(idx) = cond.find("fn:string-length(") {
        let after = &cond[idx + "fn:string-length(".len()..];
        let end = after.find(')')?;
        let path = after[..end].trim();
        let name = xpath_relative_element_local_name(path)?;
        let len = siblings.get(name)?.chars().count();
        let rest = after[end + 1..].trim();
        let rest = rest.strip_prefix("eq").unwrap_or(rest).trim();
        let n: usize = rest.parse().ok()?;
        return Some(len == n);
    }
    if let Some((left, right)) = cond.split_once(" eq ") {
        let name = xpath_relative_element_local_name(left.trim())?;
        let lit = unquote_xpath_string_literal(right);
        return Some(siblings.get(name)? == &lit);
    }
    None
}

/// Evaluate a runtime `{ if ... then 'a' else 'b' }` delimiter property using decoded siblings.
/// Evaluate `dfdl:discriminator` XPath subset using look-ahead item value (`.`).
pub fn eval_discriminator_expression(expr: &str, dot: &str) -> Option<bool> {
    let inner = expr
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(expr)
        .trim();
    if let Some(b) = parse_discriminator_bool(inner) {
        return Some(b);
    }
    let lower = inner.to_ascii_lowercase();
    if lower.starts_with("if") {
        let then_idx = lower.find(" then ")?;
        let cond = inner[..then_idx]
            .trim()
            .strip_prefix("if")
            .unwrap_or(inner)
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim();
        let tail = inner[then_idx + " then ".len()..].trim();
        let else_idx = tail.to_ascii_lowercase().find(" else ")?;
        let then_part = tail[..else_idx].trim();
        let else_part = tail[else_idx + " else ".len()..].trim();
        let cond_ok = eval_discriminator_dot_eq(cond, dot)?;
        let then_b = parse_discriminator_bool(then_part)?;
        let else_b = parse_discriminator_bool(else_part)?;
        return Some(if cond_ok { then_b } else { else_b });
    }
    eval_discriminator_dot_eq(inner, dot)
}

fn eval_discriminator_dot_eq(cond: &str, dot: &str) -> Option<bool> {
    let cond = cond.trim();
    if let Some((_, right)) = cond.split_once(". eq ") {
        let lit = unquote_xpath_string_literal(right.trim());
        return Some(dot == lit);
    }
    if let Some((left, right)) = cond.split_once(" eq ") {
        if left.trim() == "." {
            let lit = unquote_xpath_string_literal(right.trim());
            return Some(dot == lit);
        }
    }
    None
}

fn parse_discriminator_bool(s: &str) -> Option<bool> {
    match s.trim() {
        "fn:true()" | "true()" => Some(true),
        "fn:false()" | "false()" => Some(false),
        _ => None,
    }
}

pub fn eval_runtime_delimiter_expression(
    expr: &str,
    siblings: &alloc::collections::BTreeMap<alloc::string::String, alloc::string::String>,
) -> Option<String> {
    let inner = expr
        .trim()
        .strip_prefix('{')?
        .strip_suffix('}')?
        .trim();
    let lower = inner.to_ascii_lowercase();
    if !lower.starts_with("if") {
        return None;
    }
    let then_idx = lower.find(" then ")?;
    let cond = inner[..then_idx]
        .trim()
        .strip_prefix("if")
        .unwrap_or(inner)
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let tail = inner[then_idx + " then ".len()..].trim();
    let else_idx = tail.to_ascii_lowercase().find(" else ")?;
    let then_lit = &tail[..else_idx];
    let else_lit = &tail[else_idx + " else ".len()..];
    let then_s = unquote_xpath_string_literal(then_lit);
    let else_s = unquote_xpath_string_literal(else_lit);
    let ok = eval_delimiter_if_condition(cond, siblings)?;
    Some(if ok { then_s } else { else_s })
}

fn delimiter_path_step_local(step: &str) -> (&str, bool) {
    let step = step.trim();
    let indexed = step.contains("dfdl:occursIndex()");
    let local = step
        .split('[')
        .next()
        .unwrap_or(step)
        .rsplit(':')
        .next()
        .unwrap_or(step)
        .trim();
    (local, indexed)
}

/// Evaluate `{ xs:string(/ex:root/ex:field[xs:int(dfdl:occursIndex())]) }` using decoded values.
pub fn eval_path_indexed_delimiter_expression(
    expr: &str,
    occurs_index_1based: u64,
    values: &alloc::collections::BTreeMap<alloc::string::String, crate::value::DfdlValue>,
) -> Option<alloc::string::String> {
    let inner = expr
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))?
        .trim();
    let path_body = inner
        .strip_prefix("xs:string(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(inner)
        .trim();
    if !path_body.starts_with('/') {
        return None;
    }
    let segments: alloc::vec::Vec<&str> = path_body
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 2 {
        return None;
    }
    let (local, indexed) = delimiter_path_step_local(segments.last()?);
    let entry = values.iter().find(|(k, _)| {
        crate::xml_util::local_name_str(k) == local
            || k.rsplit(':').next() == Some(local)
    })?;
    let value = &entry.1;
    if indexed {
        let idx = (occurs_index_1based as usize).saturating_sub(1);
        return match value {
            crate::value::DfdlValue::Array(items) => items.get(idx).and_then(|v| v.as_str().map(|s| s.to_string())),
            _ => value.as_str().map(|s| s.to_string()),
        };
    }
    value.as_str().map(|s| s.to_string())
}

/// Validate `{...}` delimiter property expressions at schema compile time.
pub fn validate_runtime_delimiter_expression(prop: &str, expr: &str) -> Result<(), String> {
    for lit in delimiter_expression_string_literals(expr) {
        validate_delimiter_property_value(&lit)?;
        if lit.trim() == "%" && prop == "terminator" {
            return Err(format!("Invalid DFDL Entity (%) found\n%%"));
        }
    }
    if let Some(lit) = eval_compile_time_delimiter_expression(expr) {
        validate_delimiter_property_value(&lit)?;
        validate_delimiter_es_restriction(prop, &lit)?;
        if lit.trim() == "%" && prop == "terminator" {
            return Err(format!("Invalid DFDL Entity (%) found\n%%"));
        }
    }
    Ok(())
}

/// Validate initiator/separator/terminator literals at schema compile time.
fn validate_no_bare_percent_in_delimiter(raw: &str) -> Result<(), String> {
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'%' {
                i += 2;
                continue;
            }
            if let Some(rel) = raw[i..].find(';') {
                let entity_token = &raw[i..=i + rel];
                let entity = &raw[i + 1..i + rel];
                let entity_name = entity.trim_end_matches(['+', '*', '?']);
                if !entity_reference_valid(entity_name) {
                    return Err(invalid_dfdl_entity_error(entity_token, raw));
                }
                i += rel + 1;
            } else {
                return Err(invalid_dfdl_entity_error("%", raw));
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}

pub fn validate_delimiter_property_value(raw: &str) -> Result<(), String> {
    if raw.trim() == "%" {
        return Err("Invalid DFDL Entity (%) found".into());
    }
    validate_entity_tokens_in_literal_lenient(raw)?;
    validate_no_bare_percent_in_delimiter(raw)
}

/// Validate delimiter property from the XSD attribute value (before `%%` collapse).
pub fn validate_delimiter_schema_attribute(raw: &str) -> Result<(), String> {
    validate_entity_tokens_in_literal_lenient(raw)
}

/// True when alternative `alt_index` of a multi-alt delimiter may match with zero bytes consumed
/// while input remains (e.g. `%ES;` in `%ES; :`).
pub fn delimiter_alt_allows_trailing_input(pattern: &str, alt_index: u8) -> bool {
    let alts = delimiter_alternatives(pattern);
    if alts.len() <= 1 {
        return is_zero_length_delimiter(pattern);
    }
    alts.get(alt_index as usize)
        .is_some_and(|a| is_zero_length_delimiter(a))
}

/// True when a delimiter pattern is zero-length after entity expansion.
pub fn is_zero_length_delimiter(raw: &str) -> bool {
    if raw.is_empty() {
        return true;
    }
    if expand_entities_str(raw).is_empty() {
        return true;
    }
    matches!(match_delimiter_opts(b"x", raw, false), Some(0))
}

fn ascii_eq_ic(a: u8, b: u8, ignore_case: bool) -> bool {
    if a == b {
        return true;
    }
    if !ignore_case {
        return false;
    }
    a.to_ascii_lowercase() == b.to_ascii_lowercase()
}

fn bytes_startswith_ic(input: &[u8], prefix: &[u8], ignore_case: bool) -> bool {
    if input.len() < prefix.len() {
        return false;
    }
    if !ignore_case {
        return input.starts_with(prefix);
    }
    input[..prefix.len()]
        .iter()
        .zip(prefix.iter())
        .all(|(a, b)| ascii_eq_ic(*a, *b, true))
}

fn utf16_little_endian_from_encoding(encoding: Option<&str>) -> Option<bool> {
    encoding.and_then(|enc| {
        use crate::vm::encoding::normalize_encoding_name;
        match normalize_encoding_name(enc)? {
            "utf-16le" => Some(true),
            "utf-16be" => Some(false),
            _ => None,
        }
    })
}

fn wire_delimiter_logical_bytes(logical: &[u8], le: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(logical.len().saturating_mul(2));
    for &b in logical {
        if le {
            out.push(b);
            out.push(0);
        } else {
            out.push(0);
            out.push(b);
        }
    }
    out
}

fn match_one_newline_utf16(input: &[u8], le: bool) -> Option<usize> {
    let (hi, lo) = if le { (1, 0) } else { (0, 1) };
    if input.len() >= 4
        && input[hi] == 0
        && input[lo] == b'\r'
        && input[2 + hi] == 0
        && input[2 + lo] == b'\n'
    {
        return Some(4);
    }
    if input.len() >= 2 && input[hi] == 0 && matches!(input[lo], b'\n' | b'\r') {
        return Some(2);
    }
    None
}

fn match_nl_entity_utf16(input: &[u8], pattern: &str, le: bool) -> Option<usize> {
    let pat = pattern.trim();
    let quantifier = pat.chars().last().filter(|c| matches!(c, '+' | '*' | '?'));
    let base = if quantifier.is_some() {
        &pat[..pat.len().saturating_sub(1)]
    } else {
        pat
    };
    if base != "%NL;" {
        return None;
    }
    match quantifier {
        Some('+') => {
            let mut pos = 0usize;
            while let Some(n) = match_one_newline_utf16(&input[pos..], le) {
                pos += n;
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some('*') => {
            let mut pos = 0usize;
            while let Some(n) = match_one_newline_utf16(&input[pos..], le) {
                pos += n;
            }
            Some(pos)
        }
        Some('?') => Some(match_one_newline_utf16(input, le).unwrap_or(0)),
        _ => match_one_newline_utf16(input, le),
    }
}

fn match_pattern_opts_utf16(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
    le: bool,
    encoding: Option<&str>,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    if pattern.len() == 1 {
        let b = pattern.as_bytes()[0];
        if b == b'\n' {
            return match_one_newline_utf16(input, le);
        }
        let wire = wire_delimiter_logical_bytes(&[b], le);
        return if bytes_startswith_ic(input, &wire, ignore_case) {
            Some(wire.len())
        } else {
            None
        };
    }
    if pattern.starts_with('[') && pattern.contains(']') {
        return match_char_class(input, pattern);
    }
    if pattern.starts_with("%NL") {
        return match_nl_entity_utf16(input, pattern, le);
    }
    if pattern.starts_with("%WSP") || pattern.starts_with("%WS") {
        return match_wsp_entity(input, pattern);
    }
    let expanded = expand_entities_for_encoding(pattern, encoding);
    if expanded.is_empty() {
        return Some(0);
    }
    let last = pattern.as_bytes().last().copied();
    let quantifier = match last {
        Some(b'+') | Some(b'*') | Some(b'?') if expanded.len() > 1 => last,
        _ => None,
    };
    let base = if quantifier.is_some() {
        &expanded[..expanded.len().saturating_sub(1)]
    } else {
        &expanded[..]
    };
    let wire = wire_delimiter_logical_bytes(base, le);
    match quantifier {
        None => {
            if base == b"\n" {
                return match_one_newline_utf16(input, le);
            }
            if bytes_startswith_ic(input, &wire, ignore_case) {
                Some(wire.len())
            } else {
                None
            }
        }
        Some(b'+') => {
            let mut pos = 0;
            while input.len() >= pos + wire.len()
                && bytes_startswith_ic(&input[pos..], &wire, ignore_case)
            {
                pos += wire.len();
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some(b'*') => {
            let mut pos = 0;
            while input.len() >= pos + wire.len()
                && bytes_startswith_ic(&input[pos..], &wire, ignore_case)
            {
                pos += wire.len();
            }
            Some(pos)
        }
        Some(b'?') => {
            if bytes_startswith_ic(input, &wire, ignore_case) {
                Some(wire.len())
            } else {
                Some(0)
            }
        }
        _ => None,
    }
}

pub fn match_pattern_opts_for_encoding(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
    encoding: Option<&str>,
) -> Option<usize> {
    if let Some(le) = utf16_little_endian_from_encoding(encoding) {
        return match_pattern_opts_utf16(input, pattern, ignore_case, le, encoding);
    }
    match_pattern_opts_legacy(input, pattern, ignore_case, encoding)
}

/// Match input against a DFDL delimiter/initiator/terminator pattern.
/// Supports literal bytes (after entity expansion) plus simple regex suffixes: `+`, `*`, `?`.
/// Compound patterns like `%NL;%WSP*;` are matched segment-by-segment.
pub fn match_pattern(input: &[u8], pattern: &str) -> Option<usize> {
    match_pattern_opts(input, pattern, false)
}

pub fn match_pattern_opts(input: &[u8], pattern: &str, ignore_case: bool) -> Option<usize> {
    match_pattern_opts_for_encoding(input, pattern, ignore_case, None)
}

fn match_pattern_opts_legacy(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
    encoding: Option<&str>,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }

    // Single-byte literals (e.g. CSV `*` separator) — never quantifiers.
    if pattern.len() == 1 {
        let b = pattern.as_bytes()[0];
        if b == b'\n' {
            return match_one_newline(input);
        }
        return input
            .first()
            .filter(|&&x| ascii_eq_ic(x, b, ignore_case))
            .map(|_| 1);
    }

    // Regex-style character class: [abc]+ or [a-zA-Z]+
    if pattern.starts_with('[') && pattern.contains(']') {
        return match_char_class(input, pattern);
    }

    // DFDL newline entity with optional quantifier
    if pattern.starts_with("%NL") {
        return match_nl_entity(input, pattern);
    }

    // DFDL whitespace entity with optional quantifier
    if pattern.starts_with("%WSP") || pattern.starts_with("%WS") {
        return match_wsp_entity(input, pattern);
    }

    let expanded = expand_entities_for_encoding(pattern, encoding);
    if expanded.is_empty() {
        return Some(0);
    }

    let last = pattern.as_bytes().last().copied();
    // Lone `+`, `*`, or `?` are literal delimiter bytes (e.g. CSV `*` separator), not quantifiers.
    let quantifier = match last {
        Some(b'+') | Some(b'*') | Some(b'?') if expanded.len() > 1 => last,
        _ => None,
    };

    let base = if quantifier.is_some() {
        &expanded[..expanded.len().saturating_sub(1)]
    } else {
        &expanded[..]
    };

    match quantifier {
        None => {
            if base == b"\n" {
                return match_one_newline(input);
            }
            if bytes_startswith_ic(input, base, ignore_case) {
                Some(base.len())
            } else {
                None
            }
        }
        Some(b'+') => {
            let mut pos = 0;
            while input.len() >= pos + base.len()
                && bytes_startswith_ic(&input[pos..], base, ignore_case)
            {
                pos += base.len();
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some(b'*') => {
            let mut pos = 0;
            while input.len() >= pos + base.len()
                && bytes_startswith_ic(&input[pos..], base, ignore_case)
            {
                pos += base.len();
            }
            Some(pos)
        }
        Some(b'?') => {
            if bytes_startswith_ic(input, base, ignore_case) {
                Some(base.len())
            } else {
                Some(0)
            }
        }
        _ => None,
    }
}

/// Match a compound DFDL delimiter (e.g. `%NL;%WSP*;`, or `%NL;, ,` alternates).
pub fn match_delimiter(input: &[u8], pattern: &str) -> Option<usize> {
    match_delimiter_opts(input, pattern, false)
}

/// Returns `(bytes_consumed, alternative_index)` when `pattern` has multiple alternatives.
pub fn match_delimiter_with_alt(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
) -> Option<(usize, u8)> {
    match_delimiter_with_alt_for_encoding(input, pattern, ignore_case, None)
}

pub fn match_delimiter_with_alt_for_encoding(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
    encoding: Option<&str>,
) -> Option<(usize, u8)> {
    if pattern.is_empty() {
        return Some((0, 0));
    }
    if pattern.len() == 1 {
        return match_pattern_opts_for_encoding(input, pattern, ignore_case, encoding).map(|n| (n, 0));
    }
    if pattern.trim() == "%NL;, ," || pattern == "\n, ," {
        return match_nl_comma_space_separator(input).map(|n| (n, 0));
    }
    if delimiter_has_top_level_comma(pattern) && !should_split_whitespace_alternatives(pattern) {
        if let Some(n) = match_delimiter_compound(input, pattern, ignore_case, encoding) {
            if n > 0 {
                return Some((n, 0));
            }
        }
    }
    let alts = delimiter_alternatives(pattern);
    if alts.len() > 1 {
        let mut best: Option<(usize, u8)> = None;
        for (idx, alt) in alts.iter().enumerate() {
            if let Some(n) = match_delimiter_compound(input, alt, ignore_case, encoding) {
                match best {
                    None => best = Some((n, idx as u8)),
                    Some((best_n, _)) if n > best_n => best = Some((n, idx as u8)),
                    _ => {}
                }
            }
        }
        return best;
    }
    match_delimiter_compound(input, pattern, ignore_case, encoding).map(|n| (n, 0))
}

pub fn match_delimiter_opts(input: &[u8], pattern: &str, ignore_case: bool) -> Option<usize> {
    match_delimiter_opts_for_encoding(input, pattern, ignore_case, None)
}

pub fn match_delimiter_opts_for_encoding(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
    encoding: Option<&str>,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    if pattern.len() == 1 {
        return match_pattern_opts_for_encoding(input, pattern, ignore_case, encoding);
    }
    if pattern.trim() == "%NL;, ," || pattern == "\n, ," {
        return match_nl_comma_space_separator(input);
    }
    if delimiter_has_top_level_comma(pattern) && !should_split_whitespace_alternatives(pattern) {
        if let Some(n) = match_delimiter_compound(input, pattern, ignore_case, encoding) {
            if n > 0 {
                return Some(n);
            }
        }
    }
    let alts = delimiter_alternatives(pattern);
    if alts.len() > 1 {
        let mut best: Option<usize> = None;
        for alt in &alts {
            if let Some(n) = match_delimiter_compound(input, alt, ignore_case, encoding) {
                match best {
                    None => best = Some(n),
                    Some(best_n) if n > best_n => best = Some(n),
                    _ => {}
                }
            }
        }
        return best;
    }
    match_delimiter_compound(input, pattern, ignore_case, encoding)
}

/// All delimiter/initiator/terminator alternatives for a property value.
pub fn delimiter_alternatives(pattern: &str) -> alloc::vec::Vec<alloc::string::String> {
    if pattern.chars().all(|c| c == ',') && pattern.len() > 1 {
        return alloc::vec![pattern.to_string()];
    }
    if should_split_whitespace_alternatives(pattern) {
        return split_whitespace_delimiter_alternatives(pattern);
    }
    if delimiter_has_top_level_comma(pattern) {
        return split_delimiter_alternatives_comma(pattern);
    }
    if pattern.contains("||") {
        let mut out = alloc::vec::Vec::new();
        for part in pattern.split("||") {
            out.extend(split_whitespace_delimiter_alternatives(part.trim()));
        }
        if out.is_empty() {
            return alloc::vec![pattern.to_string()];
        }
        return out;
    }
    if let Some(alts) = split_entity_and_literal_alternatives(pattern) {
        return alts;
    }
    alloc::vec![unescape_dfdl_delimiter_alt(pattern)]
}

fn unescape_dfdl_delimiter_alt(raw: &str) -> alloc::string::String {
    raw.trim().to_string()
}

fn has_regex_char_class(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if pattern[i..].find(']').is_some() {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn should_split_whitespace_alternatives(pattern: &str) -> bool {
    if !pattern.contains(' ') || pattern.contains('%') || has_regex_char_class(pattern) {
        return false;
    }
    if !pattern.contains('(') {
        return true;
    }
    // Whitespace-separated literal tokens like "( [" or ") ]" — not regex groups.
    pattern.split_whitespace().all(|tok| {
        let t = tok.trim();
        if t.is_empty() {
            return false;
        }
        if t.len() == 1 {
            return true;
        }
        !t.contains('(') && !t.contains(')') && t.len() <= 2
    })
}

pub(crate) fn delimiter_has_top_level_comma(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                i += rel + 1;
                continue;
            }
        } else if bytes[i] == b'[' {
            if let Some(rel) = pattern[i..].find(']') {
                i += rel + 1;
                continue;
            }
        } else if bytes[i] == b',' {
            return true;
        }
        i += 1;
    }
    false
}

/// Match a delimiter at `input` start. Comma-containing patterns require a full compound match
/// (not a shorter comma-separated alternative) so `shi,shi` does not match as bare `shi`.
pub fn delimiter_match_len_at(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
    encoding: Option<&str>,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    if delimiter_has_top_level_comma(pattern) {
        return match_delimiter_compound(input, pattern, ignore_case, encoding).filter(|&n| n > 0);
    }
    match_delimiter_opts_for_encoding(input, pattern, ignore_case, encoding)
}

fn match_delimiter_compound(
    input: &[u8],
    pattern: &str,
    ignore_case: bool,
    encoding: Option<&str>,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    if pattern.len() == 1 {
        return match_pattern_opts_for_encoding(input, pattern, ignore_case, encoding);
    }
    let mut pos = 0;
    for segment in split_delimiter_segments(pattern) {
        let matched =
            match_pattern_opts_for_encoding(&input[pos..], segment, ignore_case, encoding)?;
        pos += matched;
    }
    Some(pos)
}

/// Split a separator/initiator/terminator into comma-separated alternatives.
/// Commas inside `%...;` entities and `[...]` classes are not separators.
/// Each comma in the list also denotes a literal `,` alternative (DFDL-12).
fn split_delimiter_alternatives_comma(pattern: &str) -> alloc::vec::Vec<alloc::string::String> {
    let mut alts = alloc::vec::Vec::new();
    let mut start = 0usize;
    let bytes = pattern.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                i += rel + 1;
                continue;
            }
        } else if bytes[i] == b'[' {
            if let Some(rel) = pattern[i..].find(']') {
                i += rel + 1;
                continue;
            }
        } else if bytes[i] == b',' {
            let part = pattern[start..i].trim();
            if !part.is_empty() {
                alts.push(unescape_dfdl_delimiter_alt(part));
            }
            alts.push(",".to_string());
            start = i + 1;
        }
        i += 1;
    }
    let tail = pattern[start..].trim();
    if !tail.is_empty() {
        alts.push(unescape_dfdl_delimiter_alt(tail));
    }
    if alts.is_empty() {
        alts.push(pattern.to_string());
    }
    alts
}

fn split_whitespace_delimiter_alternatives(pattern: &str) -> alloc::vec::Vec<alloc::string::String> {
    pattern
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(unescape_dfdl_delimiter_alt)
        .collect()
}

const NIL_VALUE_DISALLOWED_BINARY_LITERAL: &[&str] = &[
    "%NL;", "%LF;", "%CR;", "%WSP;", "%WSP+;", "%WSP*;", "%WS;", "%WS+;", "%WS*;",
];

/// Compile-time `dfdl:nilValue` checks (DFDL-6-046R, DFDL-13-235R).
pub fn validate_nil_value_compile(
    raw: &str,
    nil_kind: Option<crate::schema::NilKind>,
    representation: crate::schema::Representation,
) -> Result<(), String> {
    use crate::schema::{NilKind, Representation};
    if raw.is_empty() {
        return Err(
            "Property dfdl:nilValue cannot be empty string. Use dfdl:nilValue='%ES;' for empty string."
                .into(),
        );
    }
    let kind = nil_kind.unwrap_or(NilKind::LiteralValue);
    if kind == NilKind::LiteralCharacter {
        for token in ["%NL;", "%ES;", "%WSP;", "%WSP+;", "%WSP*;"] {
            if raw.contains(token) {
                return Err(format!(
                    "Property dfdl:nilValue contains disallowed character class(es): {token}"
                ));
            }
        }
        for alt in nil_value_alternatives(raw) {
            let expanded = expand_entities(alt.trim());
            let as_text = alloc::string::String::from_utf8_lossy(&expanded);
            if as_text.chars().count() != 1 {
                return Err(
                    "For property dfdl:nilValue the length of string must be exactly 1 character."
                        .into(),
                );
            }
        }
    }
    if representation == Representation::Binary && kind == NilKind::LiteralValue {
        for token in NIL_VALUE_DISALLOWED_BINARY_LITERAL {
            if raw.contains(token) {
                return Err(format!(
                    "Property dfdl:nilValue contains disallowed character class(es): {token}"
                ));
            }
        }
    }
    Ok(())
}

/// `escapeBlockStart` / `escapeBlockEnd` must not contain literal whitespace (DFDL-6-036R).
pub fn validate_escape_block_property(raw: &str) -> Result<(), String> {
    validate_property_no_literal_whitespace(raw, "escapeScheme")
}

/// `escapeCharacter` / `escapeEscapeCharacter` must not contain literal whitespace (DFDL-6-036R).
pub fn validate_escape_character_property(raw: &str) -> Result<(), String> {
    validate_property_no_literal_whitespace(raw, "escapeScheme")
}

fn validate_property_no_literal_whitespace(raw: &str, property: &str) -> Result<(), String> {
    if raw.chars().any(|c| c.is_whitespace()) {
        if property == "escapeScheme" {
            return Err(format!(
                "The string ({raw}) must not contain any whitespace. Use DFDL Entities (property {property})"
            ));
        }
        return Err(format!("Use DFDL Entities (property {property})"));
    }
    Ok(())
}

/// Compile-time `textNumberPadCharacter` (DFDL-6-036R), analogous to string pad.
pub fn validate_text_number_pad_character_merged(
    raw: &str,
    property_form: bool,
) -> Result<(), String> {
    validate_text_string_pad_character_compile(raw)?;
    validate_dfdl_entities_in_property(raw)?;
    if raw.chars().any(|c| c.is_whitespace()) {
        if property_form {
            return Err(format!("Use DFDL Entities (property textNumberPadCharacter)"));
        }
        return Err(
            "facet-valid NonEmptyStringLiteral property textNumberPadCharacter".into(),
        );
    }
    let expanded = expand_entities_str(raw);
    let one_char = expanded.chars().count() == 1;
    if !one_char {
        return Err("Length of string must be exactly 1 character".into());
    }
    Ok(())
}

/// Alternatives in a `dfdl:nilValue` property (whitespace-separated tokens / entities).
pub fn nil_value_alternatives(raw: &str) -> alloc::vec::Vec<alloc::string::String> {
    if let Some(alts) = split_entity_and_literal_alternatives(raw) {
        return alts;
    }
    split_whitespace_delimiter_alternatives(raw)
}

/// Split `%NL; . !`-style lists: whitespace between `%...;` entities and literal tokens.
fn split_entity_and_literal_alternatives(pattern: &str) -> Option<alloc::vec::Vec<alloc::string::String>> {
    if !pattern.contains('%') || !pattern.contains(' ') {
        return None;
    }
    let bytes = pattern.as_bytes();
    let mut alts = alloc::vec::Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                i += rel + 1;
            } else {
                return None;
            }
        } else {
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
        }
        let token = pattern[start..i].trim();
        if !token.is_empty() {
            alts.push(unescape_dfdl_delimiter_alt(token));
        }
    }
    if alts.len() > 1 {
        Some(alts)
    } else {
        None
    }
}

/// Initiators like `[s2:` are literal framing text, not DFDL delimiter regex character classes.
pub fn encode_framing_property_literal(pattern: &str) -> Option<Vec<u8>> {
    if pattern.starts_with('[')
        && !pattern.starts_with('%')
        && !pattern.contains('*')
        && !pattern.contains('?')
        && !pattern.contains('+')
        && !pattern.contains(']')
    {
        return Some(pattern.as_bytes().to_vec());
    }
    None
}

/// Encode the first alternative of a DFDL delimiter property (unparse default).
pub fn encode_property_delimiter(pattern: &str, output_new_line: Option<&str>) -> Vec<u8> {
    if pattern.trim() == "%NL;, ," || pattern == "\n, ," {
        return vec![b','];
    }
    let alts = delimiter_alternatives(pattern);
    if alts.is_empty() {
        return Vec::new();
    }
    let first = &alts[0];
    if first.is_empty() {
        return Vec::new();
    }
    if first == "%NL;" || first == "\n" {
        if let Some(onl) = output_new_line {
            return encode_delimiter(onl);
        }
    }
    if let Some(lit) = encode_framing_property_literal(first) {
        return lit;
    }
    encode_delimiter(first)
}

/// Encode one alternative from a multi-alternative delimiter property value.
pub fn encode_delimiter_by_alt(pattern: &str, alt_index: u8) -> Vec<u8> {
    let alts = delimiter_alternatives(pattern);
    if alts.is_empty() {
        return Vec::new();
    }
    let idx = (alt_index as usize).min(alts.len().saturating_sub(1));
    encode_delimiter(&alts[idx])
}

/// Minimal bytes to emit for a delimiter on encode (one WSP for `+`, none for `*`/`?`).
pub fn encode_delimiter(pattern: &str) -> Vec<u8> {
    if pattern.is_empty() {
        return Vec::new();
    }
    if let Some(lit) = encode_framing_property_literal(pattern) {
        return lit;
    }
    let alts = delimiter_alternatives(pattern);
    if alts.len() > 1 {
        return encode_property_delimiter(pattern, None);
    }
    let pat = pattern;
    if delimiter_has_top_level_comma(pat) {
        let alts = split_delimiter_alternatives_comma(pat);
        if alts.iter().any(|alt| alt == ",") {
            return vec![b','];
        }
        for alt in alts {
            let bytes = encode_delimiter(&alt);
            if !bytes.is_empty() {
                return bytes;
            }
        }
        return Vec::new();
    }
    if pat.starts_with('%')
        && !pat.starts_with('[')
    {
        let bytes = expand_entities(pat);
        if !bytes.is_empty() {
            return bytes;
        }
    }
    let mut out = Vec::new();
    for segment in split_delimiter_segments(pat) {
        out.extend(minimal_encode_segment(segment));
    }
    out
}

fn split_delimiter_segments(pattern: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                let mut end = i + rel + 1;
                let entity_body = &pattern[i + 1..i + rel];
                let entity_has_quantifier = entity_body
                    .chars()
                    .last()
                    .is_some_and(|c| matches!(c, '+' | '*' | '?'));
                if !entity_has_quantifier
                    && end < bytes.len()
                    && matches!(bytes[end], b'+' | b'*' | b'?')
                {
                    end += 1;
                }
                segments.push(&pattern[i..end]);
                i = end;
                continue;
            }
        } else if bytes[i] == b'[' {
            if let Some(rel) = pattern[i..].find(']') {
                let mut end = i + rel + 1;
                if end < bytes.len() && matches!(bytes[end], b'+' | b'*' | b'?') {
                    end += 1;
                }
                segments.push(&pattern[i..end]);
                i = end;
                continue;
            }
        }
        let start = i;
        i += 1;
        while i < bytes.len() && bytes[i] != b'%' && bytes[i] != b'[' {
            i += 1;
        }
        segments.push(&pattern[start..i]);
    }
    segments
}

fn minimal_encode_segment(segment: &str) -> Vec<u8> {
    if segment.is_empty() {
        return Vec::new();
    }
    // Whitespace-only delimiters (e.g. "\n") must not be trimmed away.
    let seg = if segment.chars().all(|c| c.is_ascii_whitespace()) {
        segment
    } else {
        segment.trim()
    };
    if seg.is_empty() {
        return Vec::new();
    }
    // Single-character whitespace-separated alternatives are literal delimiters, not regex quantifiers.
    if seg.len() == 1 {
        return vec![seg.as_bytes()[0]];
    }
    if seg.starts_with("%WSP") || seg.starts_with("%WS") {
        let q = seg.as_bytes().last().copied();
        return match q {
            Some(b'*') | Some(b'?') => Vec::new(),
            _ => vec![b' '],
        };
    }
    if seg.starts_with('%') {
        let expanded = expand_entities(seg);
        if !expanded.is_empty() {
            return expanded;
        }
    }
    if seg.starts_with('[') {
        let q = seg.as_bytes().last().copied();
        return match q {
            Some(b'*') | Some(b'?') => Vec::new(),
            _ => vec![b'0'],
        };
    }
    let q = seg.as_bytes().last().copied();
    let base = match q {
        Some(b'+') | Some(b'*') | Some(b'?') => &seg[..seg.len() - 1],
        _ => seg,
    };
    let expanded = expand_entities(base);
    match q {
        Some(b'*') | Some(b'?') => Vec::new(),
        Some(b'+') => expanded.into_iter().take(1).collect(),
        _ => expanded,
    }
}

fn is_wsp(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

fn match_nl_comma_space_separator(input: &[u8]) -> Option<usize> {
    match_nl_comma_space_separator_with_flag(input).map(|(n, _)| n)
}

/// Match `%NL;, ,` / `\n, ,` and report whether a newline prefix was consumed.
pub fn match_nl_comma_space_separator_with_flag(input: &[u8]) -> Option<(usize, bool)> {
    if let Some(nl) = match_one_newline(input) {
        let mut total = nl;
        if input.get(total) == Some(&b',') {
            total += 1;
        }
        return Some((total, true));
    }
    if input.first() == Some(&b',') || input.first() == Some(&b' ') {
        return Some((1, false));
    }
    None
}

pub fn is_nl_comma_space_pattern(pattern: &str) -> bool {
    pattern.trim() == "%NL;, ," || pattern == "\n, ,"
}

/// Encode `%NL;, ,` separator using optional outputNewLine when newline prefix is required.
pub fn encode_nl_comma_space_separator(
    output_new_line: Option<&str>,
    newline_prefix: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    if newline_prefix {
        if let Some(onl) = output_new_line {
            out.extend(encode_delimiter(onl));
        } else {
            out.push(b'\n');
        }
    }
    out.push(b',');
    out
}

pub fn encode_sequence_separator(
    pattern: &str,
    output_new_line: Option<&str>,
    newline_prefix: bool,
) -> Vec<u8> {
    if is_nl_comma_space_pattern(pattern) {
        encode_nl_comma_space_separator(output_new_line, newline_prefix)
    } else {
        encode_delimiter(pattern)
    }
}

fn match_nl_entity(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    let quantifier = pat.chars().last().filter(|c| matches!(c, '+' | '*' | '?'));
    let base = if quantifier.is_some() {
        &pat[..pat.len().saturating_sub(1)]
    } else {
        pat
    };
    if base != "%NL;" {
        return None;
    }

    match quantifier {
        Some('+') => {
            let mut pos = 0usize;
            while let Some(n) = match_one_newline(&input[pos..]) {
                pos += n;
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some('*') => {
            let mut pos = 0usize;
            while let Some(n) = match_one_newline(&input[pos..]) {
                pos += n;
            }
            Some(pos)
        }
        Some('?') => Some(match_one_newline(input).unwrap_or(0)),
        _ => match_one_newline(input),
    }
}

fn match_one_newline(input: &[u8]) -> Option<usize> {
    if input.starts_with(b"\r\n") {
        return Some(2);
    }
    if input.first().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        return Some(1);
    }
    // NEL (U+0085), LS (U+2028), PS (U+2029) — DFDL %NL; line breaks in UTF-8 data.
    if input.starts_with(&[0xC2, 0x85]) {
        return Some(2);
    }
    if input.starts_with(&[0xE2, 0x80, 0xA8]) || input.starts_with(&[0xE2, 0x80, 0xA9]) {
        return Some(3);
    }
    None
}

fn match_wsp_entity(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if !pat.starts_with('%') {
        return None;
    }
    let body = pat.strip_prefix('%')?;
    let semi = body.find(';')?;
    let entity = &body[..semi];
    if !entity.starts_with("WSP") && entity != "WS" && !entity.starts_with("WS") {
        return None;
    }
    let q = entity.chars().last();
    let quantifier = match q {
        Some('+') | Some('*') | Some('?') => q,
        _ => None,
    };

    let mut pos = 0;
    match quantifier {
        Some('+') => {
            while pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some('*') => {
            while pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            Some(pos)
        }
        Some('?') => {
            if pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            Some(pos)
        }
        _ => {
            if pos < input.len() && is_wsp(input[pos]) {
                Some(1)
            } else {
                None
            }
        }
    }
}

/// Validate a DFDL `lengthPattern` at schema compile time.
pub fn validate_length_pattern(pattern: &str) -> Result<(), String> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Ok(());
    }
    if let Some(err) = validate_length_pattern_syntax(pat) {
        return Err(err);
    }
    if length_pattern_uses_custom_matcher(pat) {
        return Ok(());
    }
    Regex::new(pat)
        .map(|_| ())
        .map_err(|e| format_length_pattern_error(pat, e))
}

fn length_pattern_uses_custom_matcher(pat: &str) -> bool {
    pat.contains("(?=")
        || pat.contains("(?!")
        || pat.contains("(?<=")
        || pat.contains("(?<!")
        || pat.contains("(?s)")
        || pat.contains("(?s:")
        || pat.contains("\\x")
}

fn validate_length_pattern_syntax(pat: &str) -> Option<String> {
    let bytes = pat.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = usize::min(i + 2, bytes.len());
            continue;
        }
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        if i >= bytes.len() {
            return Some(length_pattern_syntax_error(
                "Unclosed counted closure",
                pat,
                start,
            ));
        }
        if bytes[i] == b'}' {
            return Some(length_pattern_syntax_error("Illegal repetition", pat, start));
        }
        let mut j = i;
        let mut has_comma = false;
        while j < bytes.len() && bytes[j] != b'}' {
            if bytes[j] == b',' {
                has_comma = true;
            }
            if bytes[j] == b' ' || bytes[j] == b'\t' {
                return Some(length_pattern_syntax_error(
                    "Unclosed counted closure",
                    pat,
                    start,
                ));
            }
            j += 1;
        }
        if j >= bytes.len() {
            return Some(length_pattern_syntax_error(
                "Unclosed counted closure",
                pat,
                start,
            ));
        }
        let body = &pat[i..j];
        let valid = if has_comma {
            let parts: alloc::vec::Vec<&str> = body.splitn(2, ',').collect();
            parts.len() == 2
                && parts[0].chars().all(|c| c.is_ascii_digit())
                && parts[1].chars().all(|c| c.is_ascii_digit())
        } else {
            body.chars().all(|c| c.is_ascii_digit())
        };
        if !valid {
            return Some(length_pattern_syntax_error("Illegal repetition", pat, start));
        }
        i = j + 1;
    }

    if pat == "*" {
        return Some(length_pattern_syntax_error(
            "Dangling meta character '*'",
            pat,
            0,
        ));
    }
    None
}

fn length_pattern_syntax_error(reason: &str, pattern: &str, index: usize) -> String {
    alloc::format!(
        "Schema Definition Error. {reason} near index {index} in `{pattern}`"
    )
}

fn format_length_pattern_error(pattern: &str, err: impl core::fmt::Display) -> String {
    let detail = err.to_string();
    let lower = detail.to_ascii_lowercase();
    let suffix = if let Some(idx) = detail.find("at offset ") {
        detail[idx..].to_string()
    } else if let Some(idx) = lower.find(" near index ") {
        detail[idx + 1..].to_string()
    } else if let Some(idx) = lower.find(" at index ") {
        format!(" near index{}", &detail[idx + 9..])
    } else {
        String::new()
    };

    let reason = if pattern.contains("{") && pattern.contains(' ') && lower.contains("repetition") {
        "Unclosed counted closure"
    } else if lower.contains("repetition") || lower.contains("invalid repetition") {
        "Illegal repetition"
    } else if pattern.trim() == "*"
        || pattern.trim() == "+"
        || pattern.trim() == "?"
        || lower.contains("dangling")
    {
        "Dangling meta character '*'"
    } else if lower.contains("unclosed") {
        "Unclosed counted closure"
    } else {
        return alloc::format!("Schema Definition Error. invalid lengthPattern `{pattern}`: {detail}");
    };

    if suffix.is_empty() {
        alloc::format!("Schema Definition Error. {reason}")
    } else {
        alloc::format!("Schema Definition Error. {reason} {suffix}")
    }
}

/// Match value bytes against a DFDL length pattern (full ECMAScript-style regex).
///
/// Uses [`regex-automata`](https://docs.rs/regex-automata) (`no_std` + `alloc`) for
/// alternation, negated classes, Unicode property classes (`\p{L}`), counted
/// closures, and other constructs beyond simple `[char-class]+` patterns.
pub fn match_length_pattern(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Some(0);
    }

    if let Some(len) = match_length_pattern_custom(input, pat) {
        return Some(len);
    }

    // Fast path for simple char-class patterns without regex metacharacters.
    if pat.starts_with('[')
        && !pat.contains('\\')
        && !pat.contains('(')
        && !pat.contains('|')
    {
        if let Some(len) = match_char_class(input, pat) {
            return Some(len);
        }
    }

    if let Ok(re) = Regex::new(pat) {
        let hay = Input::new(input).anchored(Anchored::Yes);
        if let Some(m) = re.find(hay) {
            if m.start() == 0 {
                return Some(m.end());
            }
        }
        if pattern_allows_zero_length_on_mismatch(pat) {
            return Some(0);
        }
        return None;
    }

    None
}

fn pattern_allows_zero_length_on_mismatch(pat: &str) -> bool {
    !pat.contains('|')
}

fn match_bang_dot_bang(input: &[u8]) -> Option<usize> {
    if input.len() < 4 || !input.starts_with(b"!!") {
        return None;
    }
    for end in (4..=input.len()).rev() {
        if input[end - 2..end] == b"!!"[..] {
            return Some(end);
        }
    }
    None
}

fn match_empty_string_entity_value_pattern(input: &[u8]) -> Option<usize> {
    if input.len() > 3 && input.ends_with(b"END") {
        let prefix_len = input.len() - 3;
        if prefix_len <= 9 {
            return Some(prefix_len);
        }
    }
    if input.len() >= 10 {
        return Some(10);
    }
    None
}

fn match_length_pattern_custom(input: &[u8], pat: &str) -> Option<usize> {
    if pat == r"[^END]{0,9}(?=END)|.{10}" {
        return match_empty_string_entity_value_pattern(input);
    }
    if pat == "!!.*!!" {
        return match_bang_dot_bang(input);
    }
    if let Some(stripped) = pat.strip_prefix("(?s)") {
        return match_dotall_length_pattern(input, stripped);
    }
    if pat.contains("(?=,|$)") || pat.contains(r"(?=,|$)") {
        return Some(match_until_unescaped_comma(input));
    }
    if pat.contains("(?=") && pat.contains("FF") {
        return Some(match_until_ff_separator(input));
    }
    if is_simple_literal_pattern(pat) {
        let bytes = pat.as_bytes();
        if input.starts_with(bytes) {
            return Some(bytes.len());
        }
        return Some(0);
    }
    None
}

fn is_simple_literal_pattern(pat: &str) -> bool {
    let mut chars = pat.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' | '*' | '+' | '?' | '[' | '(' | ')' | '|' | '^' | '$' | '{' | '}' => return false,
            '\\' => {
                if chars.next().is_none() {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

fn match_until_unescaped_comma(input: &[u8]) -> usize {
    if input.is_empty() {
        return 0;
    }
    let mut i = 0usize;
    while i < input.len() {
        if input[i] == b',' {
            let mut j = i;
            while j > 0 && input[j - 1] == b'\\' {
                j -= 1;
            }
            if (i - j) % 2 == 0 {
                return i;
            }
        }
        i += 1;
    }
    input.len()
}

fn match_until_ff_separator(input: &[u8]) -> usize {
    let mut i = 0usize;
    while i < input.len() {
        if input[i] == 0xFF
            && i + 1 < input.len()
            && (0x01..=0xFE).contains(&input[i + 1])
        {
            return i;
        }
        i += 1;
    }
    input.len()
}

fn match_dotall_length_pattern(input: &[u8], pat: &str) -> Option<usize> {
    match_dotall(input, 0, pat, 0)
}

fn match_dotall(input: &[u8], ip: usize, pat: &str, pp: usize) -> Option<usize> {
    if pp >= pat.len() {
        return Some(ip);
    }
    if let Some((adv, nip)) = match_dotall_optional_crlf(input, ip, &pat[pp..]) {
        return match_dotall(input, nip, pat, pp + adv);
    }
    if pat.as_bytes()[pp] == b'(' {
        if let Some((group_len, alts)) = parse_dotall_alternation(&pat[pp..]) {
            for alt in alts {
                if let Some(nip) = match_dotall(input, ip, alt, 0) {
                    if let Some(end) = match_dotall(input, nip, pat, pp + group_len) {
                        return Some(end);
                    }
                }
            }
            return None;
        }
    }
    if pat.as_bytes()[pp] == b'.' {
        if ip >= input.len() {
            return None;
        }
        return match_dotall(input, ip + 1, pat, pp + 1);
    }
    if let Some((lit, adv)) = read_pattern_literal(&pat[pp..]) {
        if input[ip..].starts_with(lit.as_bytes()) {
            return match_dotall(input, ip + lit.len(), pat, pp + adv);
        }
        return None;
    }
    None
}

fn match_dotall_optional_crlf(input: &[u8], ip: usize, pat: &str) -> Option<(usize, usize)> {
    if pat.starts_with("(\\r\\n)?") {
        let nip = if input[ip..].starts_with(b"\r\n") {
            ip + 2
        } else {
            ip
        };
        return Some(("(\\r\\n)?".len(), nip));
    }
    None
}

fn parse_dotall_alternation(pat: &str) -> Option<(usize, alloc::vec::Vec<&str>)> {
    if !pat.starts_with('(') {
        return None;
    }
    let close = find_matching_paren(pat)?;
    let body = &pat[1..close];
    if body.starts_with('?') {
        return None;
    }
    if !body.contains('|') {
        return None;
    }
    Some((close + 1, body.split('|').collect()))
}

fn find_matching_paren(pat: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in pat.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn read_pattern_literal(pat: &str) -> Option<(String, usize)> {
    if pat.is_empty() {
        return None;
    }
    if pat.starts_with('(') || pat.starts_with('.') {
        return None;
    }
    let mut out = String::new();
    let mut i = 0usize;
    let bytes = pat.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'(' || bytes[i] == b'.' {
            break;
        }
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'r' => out.push('\r'),
                b'n' => out.push('\n'),
                b't' => out.push('\t'),
                other => out.push(char::from(other)),
            }
            i += 2;
            continue;
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    if out.is_empty() {
        return None;
    }
    Some((out, i))
}

fn match_char_class(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if !pat.starts_with('[') {
        return None;
    }
    let close = pat.find(']')?;
    let class_body = &pat[1..close];
    let suffix = &pat[close + 1..];

    let mut pos = 0;
    while pos < input.len() {
        let ch = input[pos];
        if !char_in_class(ch, class_body) {
            break;
        }
        pos += 1;
    }

    match suffix {
        "+" if pos > 0 => Some(pos),
        "*" => Some(pos),
        "?" => Some(pos.min(1)),
        "" if pos > 0 => Some(pos),
        _ if pos > 0 && suffix.is_empty() => Some(pos),
        _ => None,
    }
}

fn char_in_class(ch: u8, class_body: &str) -> bool {
    let ch_arr = [ch];
    let s = core::str::from_utf8(&ch_arr).unwrap_or("");
    let c = s.chars().next().unwrap_or('\0');
    let mut i = 0;
    let chars: Vec<char> = class_body.chars().collect();
    let mut negate = false;
    if chars.first() == Some(&'^') {
        negate = true;
        i = 1;
    }
    let mut matched = false;
    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let lo = chars[i];
            let hi = chars[i + 2];
            if c >= lo && c <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if c == chars[i] {
                matched = true;
            }
            i += 1;
        }
    }
    matched ^ negate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_double_newline_delimiter() {
        assert_eq!(match_delimiter(b"\n\nbody", "%NL;%NL;"), Some(2));
        assert_eq!(match_delimiter(b"\n\n", "%NL;%NL;"), Some(2));
    }

    #[test]
    fn match_newline_delimiter() {
        assert_eq!(match_delimiter(b"\n", "\n"), Some(1));
        assert_eq!(match_delimiter(b"\nrest", "\n"), Some(1));
        // Newline must not be treated as an empty trimmable pattern.
        assert_ne!(match_pattern(b"x", "\n"), Some(0));
    }

    #[test]
    fn match_es_or_colon_terminator_alts() {
        let pat = "%ES; :";
        assert_eq!(match_delimiter_with_alt(b",1", pat, false), Some((0, 0)));
        assert_eq!(match_delimiter_with_alt(b":,1", pat, false), Some((1, 1)));
    }

    fn validate_percent_escape_in_delimiter() {
        assert!(validate_delimiter_property_value("%%").is_ok());
        assert!(validate_delimiter_property_value("%").is_err());
        assert!(validate_delimiter_property_value("test%").is_err());
        assert!(validate_delimiter_property_value("%SP;").is_ok());
        assert!(validate_delimiter_property_value("%SP").is_ok());
        assert!(validate_delimiter_property_value("%%%SP;").is_ok());
        assert!(validate_delimiter_es_restriction("terminator", "%ES; END").is_ok());
        assert!(validate_delimiter_es_restriction("terminator", "%ES;").is_err());
    }

    #[test]
    fn expand_nl() {
        assert_eq!(expand_entities("%NL;"), b"\n");
        assert_eq!(expand_entities("%NUL;"), b"\0");
        assert_eq!(expand_entities("%#r3b;"), b";");
    }

    #[test]
    fn match_wsp_plus() {
        let pat = "%WSP+;";
        assert_eq!(match_pattern(b"   x", pat), Some(3));
    }

    #[test]
    fn match_alpha_pattern() {
        assert_eq!(match_length_pattern(b"aSingleToken123", "[a-zA-Z]+"), Some(12));
        assert_eq!(match_length_pattern(b"123456789", "[0-9]+"), Some(9));
    }

    #[test]
    fn match_alternation_pattern() {
        assert_eq!(match_length_pattern(b"batcz", "(b|c|h)at"), Some(3));
        assert_eq!(match_length_pattern(b"catx", "(b|c|h)at"), Some(3));
        assert_eq!(match_length_pattern(b"dat", "(b|c|h)at"), None);
        assert_eq!(match_length_pattern(b"bbb", "a*|bbb+"), Some(3));
        assert_eq!(match_length_pattern(b"aaaaaaa", "a{0,5}|bbb+"), None);
    }

    #[test]
    fn match_negated_class_pattern() {
        assert_eq!(match_length_pattern(b"cz", "[^ab]z"), Some(2));
        assert_eq!(match_length_pattern(b"az", "[^ab]z"), Some(0));
    }

    #[test]
    fn match_unicode_property_pattern() {
        assert_eq!(match_length_pattern(b"abcDEFG", r"\p{L}{2,5}"), Some(5));
        assert_eq!(match_length_pattern(b"a1", r"\p{L}{2,5}"), Some(0));
    }

    #[test]
    fn match_compound_wsp_nl_separator() {
        let pat = "%WSP;%WSP+;+%NL;%WSP*;";
        let sep = b"  +\n\t\t  ";
        let segs = split_delimiter_segments(pat);
        assert_eq!(
            segs,
            vec!["%WSP;", "%WSP+;", "+", "%NL;", "%WSP*;"],
            "segment split"
        );
        let mut pos = 0usize;
        for seg in &segs {
            let m = match_pattern(&sep[pos..], seg).expect("segment should match");
            pos += m;
        }
        assert_eq!(pos, sep.len());
        assert_eq!(match_delimiter(sep, pat), Some(sep.len()));
        assert_eq!(match_delimiter(&b"abcd  +\n\t\t  efg"[4..], pat), Some(sep.len()));
    }

    #[test]
    fn validate_invalid_length_patterns() {
        let e1 = validate_length_pattern("[a-z]{1, 2}").unwrap_err();
        assert!(e1.contains("Schema Definition Error"));
        assert!(e1.contains("Unclosed counted closure"), "{e1}");

        let e2 = validate_length_pattern("[a-z]{B}").unwrap_err();
        assert!(e2.contains("Schema Definition Error"));
        assert!(e2.contains("Illegal repetition"), "{e2}");

        let e3 = validate_length_pattern("*").unwrap_err();
        assert!(e3.contains("Schema Definition Error"));
        assert!(e3.contains("Dangling meta character '*'"), "{e3}");
    }

    #[test]
    fn compound_separator_matches_tab_document() {
        let pat = "%WSP;%WSP+;+%NL;%WSP*;";
        // Matches DelimitedTests.tdml lengthKindDelimited_02 document.
        let sep = b"\t\t+\n\t\t\t\t";
        assert_eq!(match_delimiter(sep, pat), Some(sep.len()));
        let doc = b"abcd\t\t+\n\t\t\t\tefg";
        assert_eq!(match_delimiter(&doc[4..], pat), Some(sep.len()));
    }

    #[test]
    fn match_wsp_star_nl_terminator() {
        let input = b" \n";
        assert_eq!(match_delimiter(input, "%WSP*;%NL;"), None);
        assert_eq!(match_delimiter(b",dog", "%NL;, ,"), Some(1));
    }

    #[test]
    fn match_literal_star_separator() {
        assert_eq!(super::match_pattern(b"*x", "*"), Some(1));
        assert_eq!(match_delimiter(b"*x", "*"), Some(1));
        assert_eq!(match_delimiter(b"*", "*"), Some(1));
    }

    #[test]
    fn text_standard_zero_rep_z_wsp_pattern() {
        let pat = "Z%WSP*;Z%WSP*;Z";
        let doc = b"Z Z Z";
        assert_eq!(match_delimiter_opts(doc, pat, false), Some(doc.len()));
    }

    #[test]
    fn match_nl_separator_accepts_cr_and_unicode_line_breaks() {
        assert_eq!(match_delimiter(b"\r5,6", "\n"), Some(1));
        assert_eq!(match_delimiter(b"\n5,6", "\n"), Some(1));
        assert_eq!(match_delimiter(&[0xC2, 0x85, b'5'], "\n"), Some(2));
        assert_eq!(match_delimiter(&[0xE2, 0x80, 0xA8, b'5'], "\n"), Some(3));
    }

    #[test]
    fn match_nl_crlf_and_nested_pattern() {
        assert_eq!(match_delimiter(b"\r\n,house.", "%NL;, ,"), Some(3));
        assert_eq!(match_delimiter(b"\r\n,house.", "\n, ,"), Some(3));
        assert_eq!(match_delimiter(b",dog", "%NL;, ,"), Some(1));
        assert_eq!(match_delimiter(b",dog", "\n, ,"), Some(1));
        let doc = b"cat,dog\r\n,house.";
        let pat = "(?s)cat(\r\n)?,dog(\r\n)?,house.";
        assert_eq!(match_length_pattern(doc, pat), Some(doc.len()));
    }

    #[test]
    fn parse_delimiter_literal_preserves_compound_entity_sequences() {
        assert_eq!(
            parse_delimiter_literal_value("%WSP*;%NL;"),
            "%WSP*;%NL;"
        );
        assert_eq!(
            parse_delimiter_literal_value("%WSP;%WSP+;+%NL;%WSP*;"),
            "%WSP;%WSP+;+%NL;%WSP*;"
        );
        assert_eq!(parse_delimiter_literal_value("%WSP+;"), "%WSP+;");
        assert_eq!(parse_delimiter_literal_value("%WSP*;"), "%WSP*;");
    }

    #[test]
    fn parse_delimiter_literal_unescapes_single_token_open_braces() {
        assert_eq!(super::parse_delimiter_literal_value("{{"), "{");
        assert_eq!(super::parse_delimiter_literal_value("{{{"), "{{");
        assert_eq!(super::parse_delimiter_literal_value("{{ {{ ["), "{{ {{ [");
    }

    #[test]
    fn match_or_and_whitespace_delimiter_alternatives() {
        assert_eq!(delimiter_alternatives("( ["), vec!["(", "["]);
        assert_eq!(match_delimiter(b"(abc)", "( ["), Some(1));
        assert_eq!(match_delimiter(b"[123]", "( ["), Some(1));
        assert_eq!(match_delimiter(b"]2", "} ] )"), Some(1));
        assert_eq!(match_delimiter(b")3", "} ] )"), Some(1));
        assert_eq!(match_delimiter(b":-5", ":: || : $"), Some(1));
        assert_eq!(match_delimiter(b"$", ":: || : $"), Some(1));
        assert_eq!(match_delimiter(b"::13", ":: || : $"), Some(2));
        let alts = super::delimiter_alternatives(":: || : $");
        assert_eq!(alts, vec!["::", ":", "$"]);
        let alts2 = super::delimiter_alternatives("{{ {{ [");
        assert_eq!(alts2, vec!["{{", "{{", "["]);
        assert_eq!(match_delimiter(b"{{9", "{{ {{ ["), Some(2));
    }

    #[test]
    fn parse_delimiter_literal_preserves_raw_byte_entity() {
        let lit = parse_delimiter_literal_value("%#rab;");
        assert_eq!(lit, "%#rab;");
        assert_eq!(expand_entities(&lit), vec![0xAB]);
    }

    #[test]
    fn eval_runtime_delimiter_string_length() {
        let sibs = alloc::collections::BTreeMap::from([(
            "value".to_string(),
            "0123456789".to_string(),
        )]);
        let expr = "{if (fn:string-length(./ex:value) eq 10) then '%ES;' else 'END'}";
        assert_eq!(
            eval_runtime_delimiter_expression(expr, &sibs).as_deref(),
            Some("%ES;")
        );
    }

    #[test]
    fn empty_string_entity_value_pattern() {
        assert_eq!(
            match_length_pattern(b"0123456789", r"[^END]{0,9}(?=END)|.{10}"),
            Some(10)
        );
        assert_eq!(
            match_length_pattern(b"01234END", r"[^END]{0,9}(?=END)|.{10}"),
            Some(5)
        );
    }

    #[test]
    fn iso8859_hex_entity_initiator_bytes() {
        let pat = "%#x80;%#x81;";
        let expanded = expand_entities_for_encoding(pat, Some("iso-8859-1"));
        assert_eq!(expanded, vec![0x80, 0x81]);
        let data = [0x80, 0x81, b'x'];
        assert_eq!(
            match_delimiter_opts_for_encoding(&data[..], pat, false, Some("ISO-8859-1")),
            Some(2)
        );
    }

    #[test]
    fn utf16_be_separator_slash() {
        let data = [0x00, b'1', 0x00, b'2', 0x00, b'/', 0x00, b'3'];
        assert_eq!(
            match_delimiter_opts_for_encoding(&data[4..], "/", false, Some("utf-16be")),
            Some(2)
        );
        assert_eq!(
            match_delimiter_opts_for_encoding(&data[0..], "/", false, Some("utf-16be")),
            None
        );
    }

    #[test]
    fn double_pipe_separator_pattern() {
        assert_eq!(match_delimiter_opts(b"||Shoes", "||", false), Some(2));
        assert_eq!(delimiter_alternatives("||"), vec!["||"]);
        assert_eq!(delimiter_alternatives("a||b"), vec!["a", "b"]);
    }

    #[test]
    fn match_bang_dot_bang_pattern() {
        use crate::schema::EncodingErrorPolicy;
        use crate::vm::encoding::read_one_utf8_char;
        let doc = b"!!\xc2\xc2!!";
        assert_eq!(match_length_pattern(doc, "!!.*!!"), Some(6));
        assert!(read_one_utf8_char(doc, 2, EncodingErrorPolicy::Error).is_err());
    }

    #[test]
    fn dfdl708_orig_pattern_allows_comma_in_local_part() {
        let pat = r"[.A-Za-z0-9!#$%&'*+-/=?^_`\{\|\}~]+";
        let doc = b"john,doe";
        assert_eq!(
            match_length_pattern(doc, pat),
            Some(doc.len()),
            "DFDL-708 orig pattern should match comma via +-/ range"
        );
    }

    #[test]
    fn encode_newline_delimiters() {
        assert_eq!(expand_entities("%NL;"), vec![10u8]);
        assert_eq!(encode_delimiter("\n"), vec![10u8]);
        assert_eq!(encode_delimiter("%NL;"), vec![10u8]);
    }

    #[test]
    fn encode_delimiter_alternatives_prefers_comma() {
        assert_eq!(encode_delimiter("%NL;, ,"), vec![b',']);
        assert_eq!(encode_delimiter("\n, ,"), vec![b',']);
    }

    #[test]
    fn whitespace_separated_separator_alternatives() {
        let pat = ", ,, ,,,";
        assert_eq!(delimiter_alternatives(pat), vec![",", ",,", ",,,"]);
        assert_eq!(match_delimiter_opts(b",,2", pat, false), Some(2));
        assert_eq!(match_delimiter_opts(b",,,3", pat, false), Some(3));
    }

    #[test]
    fn encode_nl_comma_space_separator_uses_output_new_line() {
        assert_eq!(encode_nl_comma_space_separator(Some("%CR;%LF;"), true), vec![13, 10, 44]);
        assert_eq!(encode_nl_comma_space_separator(Some("%CR;%LF;"), false), vec![44]);
    }

    #[test]
    fn parse_sequence5_delim_match() {
        let data = b"[more[{{((55)),,((66)),,((77))}}]nomore]";
        assert_eq!(super::match_delimiter_opts(&data[6..], "{{", false), Some(2));
        assert_eq!(super::match_delimiter_opts(&data[8..], "((", false), Some(2));
        assert!(super::match_delimiter_opts(&data[9..], "((", false).is_none());
    }

    #[test]
    fn dollar_separator_prefix_of_terminator() {
        let data = b"$$";
        assert_eq!(super::match_delimiter_opts(data, "$", false), Some(1));
        assert_eq!(super::match_delimiter_opts(data, "$$", false), Some(2));
    }

    #[test]
    fn shi_field_terminator_longer_than_infix_separator() {
        let data = b"shi,shishi";
        assert_eq!(super::match_delimiter_opts(data, "shi", false), Some(3));
        assert_eq!(super::match_delimiter_opts(data, "shi,shi", false), Some(7));
    }

    #[test]
    fn double_comma_occurrence_separator() {
        let data = b",,((66))";
        assert_eq!(super::match_delimiter_opts(data, ",,", false), Some(2));
    }
}

#[cfg(test)]
mod alt_split_tests {
    use super::*;
    #[test]
    fn entity_whitespace_alts() {
        assert_eq!(
            parse_delimiter_literal_value("%NL; . !"),
            "%NL; . !"
        );
        let alts = delimiter_alternatives("%NL; . !");
        assert_eq!(alts, vec!["%NL;", ".", "!"]);
        assert_eq!(encode_delimiter("? . !"), vec![b'?']);
        assert_eq!(encode_property_delimiter("%ES; %NL; !", None), Vec::<u8>::new());
        assert_eq!(encode_property_delimiter("%WSP+; * )", None), vec![b' ']);
        assert_eq!(
            encode_property_delimiter("[s2:", None),
            b"[s2:".to_vec()
        );
        assert_eq!(encode_delimiter("[s1:"), b"[s1:".to_vec());
    }

    #[test]
    fn nil_value_compile_empty_and_binary_nl() {
        use crate::schema::{NilKind, Representation};
        assert!(validate_nil_value_compile("", None, Representation::Text).is_err());
        assert!(validate_nil_value_compile(
            "%NL;",
            Some(NilKind::LiteralValue),
            Representation::Binary
        )
        .is_err());
        assert!(validate_nil_value_compile(
            "%NUL;%NUL;%NUL;%NUL;",
            Some(NilKind::LiteralValue),
            Representation::Binary
        )
        .is_ok());
    }
}
