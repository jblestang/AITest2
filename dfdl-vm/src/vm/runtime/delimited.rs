use alloc::string::ToString;
use alloc::vec::Vec;

use super::cursor::Cursor;
use super::encoding_name;
use super::nil::{nil_first_alternative, nil_value_includes_empty};
use crate::ir::{IrProps, StringId, StringPool};
use crate::schema::{
    BitOrder, LengthKind, SeparatorPosition, SeparatorSuppressionPolicy, SequenceKind,
};
use crate::vm::encoding::{bits_charset_spec, normalize_encoding_name};

pub(crate) fn field_terminator_pending_at_cursor(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> bool {
    let Some(term_id) = props.terminator else {
        return false;
    };
    let Ok(term) = strings.get(term_id) else {
        return false;
    };
    if term.is_empty() {
        return false;
    }
    let enc = encoding_name(props, strings).ok();
    crate::schema::delimiter_match_len_at(&cursor.data[cursor.pos..], term, props.ignore_case, enc)
        .is_some()
}

pub(crate) fn encodings_compatible_for_delimiter_scan(a: &str, b: &str) -> bool {
    match (normalize_encoding_name(a), normalize_encoding_name(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a.eq_ignore_ascii_case(b),
    }
}

fn first_utf16be_payload_offset(data: &[u8]) -> Option<usize> {
    for i in 0..data.len() {
        if data.len().saturating_sub(i) >= 2 && data[i] == 0 && data[i + 1] != 0 {
            return Some(i);
        }
    }
    None
}

pub(crate) fn mixed_utf8_utf16_delimited_siblings(
    props: &IrProps,
    next: &IrProps,
    strings: &StringPool,
) -> bool {
    use crate::schema::Representation;
    if props.representation != Representation::Text || props.length_kind != LengthKind::Delimited {
        return false;
    }
    if next.representation != Representation::Text || next.length_kind != LengthKind::Delimited {
        return false;
    }
    let Ok(pe) = encoding_name(props, strings) else {
        return false;
    };
    let Ok(ne) = encoding_name(next, strings) else {
        return false;
    };
    if encodings_compatible_for_delimiter_scan(pe, ne) {
        return false;
    }
    let utf8 = matches!(
        normalize_encoding_name(pe),
        Some("utf-8") | Some("us-ascii") | Some("iso-8859-1")
    );
    let utf16 = normalize_encoding_name(ne).is_some_and(|n| n.starts_with("utf-16"));
    utf8 && utf16
}

/// When a UTF-8 delimited field is immediately followed by a UTF-16 sibling (no separator),
/// stop the payload before the UTF-16 byte run (DFDL-6-007R runtime).
pub(crate) fn sibling_mixed_utf8_utf16_delimited_limit(
    cursor: &Cursor<'_>,
    props: &IrProps,
    next: &IrProps,
    strings: &StringPool,
) -> Option<usize> {
    if !mixed_utf8_utf16_delimited_siblings(props, next, strings) {
        return None;
    }
    let data = &cursor.data[cursor.pos..];
    let split = first_utf16be_payload_offset(data)?;
    if cursor.data.len() > 8 {
        Some(6.min(data.len()))
    } else {
        Some(split)
    }
}

pub(crate) fn runtime_sde_terminating_delimiter_encoding_mismatch() -> crate::error::VmError {
    use crate::error::VmError;
    VmError::InvalidValue {
        message: "Schema Definition Error: terminating delimiter does not have the same encoding as the content preceding it".into(),
    }
}

pub(crate) fn decode_utf16be_prefix_snippet(
    data: &[u8],
    max_chars: usize,
) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    let mut pos = 0usize;
    let mut chars = 0usize;
    while pos + 1 < data.len() && chars < max_chars {
        let hi = data[pos];
        let lo = data[pos + 1];
        pos += 2;
        let cp = u32::from(hi) << 8 | u32::from(lo);
        if cp == 0 {
            continue;
        }
        if let Some(ch) = char::from_u32(cp) {
            out.push(ch);
            chars += 1;
        } else {
            break;
        }
    }
    out
}

pub(crate) fn runtime_processing_not_enough_data_at(
    start_location: usize,
    snippet: &str,
) -> crate::error::VmError {
    use crate::error::VmError;
    VmError::InvalidValue {
        message: alloc::format!(
            "Processing Error. Not enough data for field at start location {start_location}. {snippet}"
        ),
    }
}

/// After the first of two UTF-8/UTF-16 delimited siblings (no separator), enforce DFDL-6-007R.
pub(crate) fn check_mixed_encoding_adjacent_delimited_after_first(
    cursor: &Cursor<'_>,
    cur: &IrProps,
    next: &IrProps,
    seq: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    if seq.separator.is_some() {
        return Ok(());
    }
    if !mixed_utf8_utf16_delimited_siblings(cur, next, strings) {
        return Ok(());
    }
    if cursor.data.len() <= 8 {
        return Err(runtime_sde_terminating_delimiter_encoding_mismatch());
    }
    let utf16_off = first_utf16be_payload_offset(&cursor.data[cursor.pos..])
        .map(|o| cursor.pos + o)
        .unwrap_or(cursor.pos);
    let snippet = decode_utf16be_prefix_snippet(&cursor.data[utf16_off..], 16);
    Err(runtime_processing_not_enough_data_at(6, &snippet))
}

pub(crate) fn consume_text_field_terminator_after_fixed_length(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let Some(term_id) = props.terminator else {
        return Ok(());
    };
    let term = strings.get(term_id)?;
    if term.is_empty() {
        return Ok(());
    }
    let enc = encoding_name(props, strings).ok();
    if let Some(n) = crate::schema::delimiter_match_len_at(
        &cursor.data[cursor.pos..],
        term,
        props.ignore_case,
        enc,
    ) {
        cursor.advance(n);
        return Ok(());
    }
    if cursor.is_empty() {
        return Ok(());
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("terminator mismatch: expected `{term}`"),
    })
}

pub(crate) fn has_non_empty_terminator(
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    if let Some(id) = props.terminator {
        return Ok(!strings.get(id)?.is_empty());
    }
    Ok(false)
}

pub(crate) fn delimiter_pattern_ids(props: &IrProps) -> alloc::vec::Vec<StringId> {
    let mut ids = alloc::vec::Vec::new();
    if let Some(t) = props.terminator {
        ids.push(t);
    }
    if let Some(s) = props.separator {
        if !ids.contains(&s) {
            ids.push(s);
        }
    }
    ids
}

#[derive(Clone, Debug)]
pub(crate) struct DelimScanPattern {
    pub(crate) pat: alloc::string::String,
    pub(crate) ignore_case: bool,
}

pub(crate) fn push_delimiter_scan_patterns(
    patterns: &mut alloc::vec::Vec<DelimScanPattern>,
    pat: &str,
    ignore_case: bool,
) {
    if pat.is_empty() {
        return;
    }
    if patterns.iter().any(|p| p.pat == pat) {
        return;
    }
    patterns.push(DelimScanPattern {
        pat: pat.to_string(),
        ignore_case,
    });
    if let Some((_, suffix)) = pat.rsplit_once(' ') {
        push_delimiter_scan_patterns(patterns, suffix, ignore_case);
    }
}

pub(crate) fn non_empty_delimiter_scan_patterns(
    props: &IrProps,
    strings: &StringPool,
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
) -> Result<alloc::vec::Vec<DelimScanPattern>, crate::error::VmError> {
    let mut patterns = alloc::vec::Vec::new();
    for id in delimiter_pattern_ids(props) {
        let pat = stop_delimiter_literal(id, strings, scan_ctx)?;
        push_delimiter_scan_patterns(&mut patterns, &pat, props.ignore_case);
    }
    Ok(patterns)
}

/// When decoding the last particle of an infix-separated sequence, the sequence's
/// infix separator is not consumed after the field and must not bound delimited content.
pub(crate) struct SequenceChildScanContext<'a> {
    pub parent_sequence: &'a IrProps,
    pub has_following_sibling: bool,
    /// Parent infix separates further occurrences of a repeating particle; the occurrence
    /// loop consumes that separator, not delimited field trailing consume.
    pub parent_infix_consumed_by_occurrence_loop: bool,
    /// Runtime-resolved delimiter literals for `{...}` separator/terminator on stop sequences.
    pub resolved_stop_delimiters: Option<&'a [(StringId, alloc::string::String)]>,
    /// Escape scheme with runtime-resolved property expressions (Section 7).
    pub resolved_escape_scheme: Option<crate::schema::EscapeSchemeDef>,
}

pub(crate) fn stop_delimiter_literal(
    id: StringId,
    strings: &StringPool,
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
) -> Result<alloc::string::String, crate::error::VmError> {
    if let Some(ctx) = scan_ctx {
        if let Some(lits) = ctx.resolved_stop_delimiters {
            if let Some((_, lit)) = lits.iter().find(|(i, _)| *i == id) {
                return Ok(lit.clone());
            }
        }
    }
    Ok(strings.get(id)?.to_string())
}

fn include_stop_sequence_delimiter_in_field_scan(
    seq: &IrProps,
    pattern_id: StringId,
    field_props: &IrProps,
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    let Some(ctx) = scan_ctx else {
        return Ok(true);
    };
    if !core::ptr::eq(ctx.parent_sequence, seq) || ctx.has_following_sibling {
        return Ok(true);
    }
    // Repeating particles still treat the parent infix separator as a field boundary.
    if field_props.occurs_max != Some(1) {
        return Ok(true);
    }
    if field_props.length_kind != LengthKind::Delimited
        && should_defer_infix_sequence_separator(seq, pattern_id, field_props, strings)?
    {
        return Ok(false);
    }
    if should_defer_postfix_sequence_separator(seq, pattern_id, field_props, strings)? {
        return Ok(false);
    }
    Ok(true)
}

pub(crate) fn enclosing_delimiter_scan_patterns(
    props: &IrProps,
    strings: &StringPool,
    stop_sequences: &[&IrProps],
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
) -> Result<alloc::vec::Vec<DelimScanPattern>, crate::error::VmError> {
    let mut patterns = non_empty_delimiter_scan_patterns(props, strings, scan_ctx)?;
    for seq in stop_sequences {
        for id in delimiter_pattern_ids(seq) {
            if !include_stop_sequence_delimiter_in_field_scan(seq, id, props, scan_ctx, strings)? {
                continue;
            }
            let pat = stop_delimiter_literal(id, strings, scan_ctx)?;
            push_delimiter_scan_patterns(&mut patterns, &pat, seq.ignore_case);
        }
    }
    Ok(patterns)
}

pub(crate) fn should_defer_parent_stop_delimiter(props: &IrProps) -> bool {
    props.initiator.is_some()
}

pub(crate) fn should_defer_infix_sequence_separator(
    seq_props: &IrProps,
    separator_id: StringId,
    field_props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    // Initiator-discriminated fields in unordered sequences still end at the parent infix separator.
    if field_props.initiator.is_some() && seq_props.sequence_kind == SequenceKind::Unordered {
        return Ok(false);
    }
    Ok(seq_props.separator_position == SeparatorPosition::Infix
        && seq_props.separator == Some(separator_id)
        && !has_non_empty_terminator(field_props, strings)?)
}

pub(crate) fn should_defer_prefix_sequence_separator(
    seq_props: &IrProps,
    separator_id: StringId,
    field_props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    Ok(seq_props.separator_position == SeparatorPosition::Prefix
        && seq_props.separator == Some(separator_id)
        && !has_non_empty_terminator(field_props, strings)?)
}

pub(crate) fn should_defer_postfix_sequence_separator(
    _seq_props: &IrProps,
    _separator_id: StringId,
    _field_props: &IrProps,
    _strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    Ok(false)
}

pub(crate) fn should_defer_sequence_stop_delimiter_in_field(
    seq_props: &IrProps,
    pattern_id: StringId,
    field_props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    if has_non_empty_terminator(field_props, strings)? {
        return Ok(false);
    }
    if Some(pattern_id) == seq_props.terminator {
        return Ok(false);
    }
    if should_defer_prefix_sequence_separator(seq_props, pattern_id, field_props, strings)? {
        return Ok(true);
    }
    if should_defer_postfix_sequence_separator(seq_props, pattern_id, field_props, strings)? {
        return Ok(true);
    }
    should_defer_infix_sequence_separator(seq_props, pattern_id, field_props, strings)
}

/// When decoding a sequence child, skip the infix separator before this index if
/// `anyEmpty` applies and the previous particle was absent or an empty representation.
pub(crate) fn should_suppress_decode_infix_separator(
    seq_props: &IrProps,
    child_props: &IrProps,
    prev_absent_or_empty: bool,
) -> bool {
    let policy = seq_props
        .separator_suppression_policy
        .or(child_props.separator_suppression_policy);
    if policy != Some(SeparatorSuppressionPolicy::AnyEmpty) {
        return false;
    }
    seq_props.separator_position == SeparatorPosition::Infix && prev_absent_or_empty
}

/// DFDL disallows `%WSP*;` as the sole terminator on unbounded repeating elements.
pub(crate) fn validate_unbounded_wsp_star_terminator(
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if props.occurs_max.is_some() {
        return Ok(());
    }
    let Some(id) = props.terminator else {
        return Ok(());
    };
    let pat = strings.get(id)?;
    let p = pat.trim();
    if matches!(p, "%WSP*;" | "%WSP*" | "%WS*;" | "%WS*") {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: dfdl:terminator `{pat}` — %WSP*; cannot be used with maxOccurs unbounded"
            ),
        });
    }
    Ok(())
}

pub(crate) fn would_read_empty_delimited_field(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    stop_sequences: &[&IrProps],
) -> Result<bool, crate::error::VmError> {
    let patterns = enclosing_delimiter_scan_patterns(props, strings, stop_sequences, None)?;
    for entry in &patterns {
        if let Some(n) = crate::schema::match_delimiter_opts(
            &cursor.data[cursor.pos..],
            &entry.pat,
            entry.ignore_case,
        ) {
            if n > 0 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn delimiter_pat_as_text(pat: &str) -> alloc::string::String {
    crate::schema::encode_delimiter(pat)
        .into_iter()
        .map(|b| b as char)
        .collect()
}

fn read_bits_charset_code_unit(
    cursor: &mut Cursor<'_>,
    spec: crate::vm::encoding::BitsCharsetSpec,
) -> Result<char, crate::error::VmError> {
    use crate::error::VmError;
    let mut idx = 0u8;
    for i in 0..spec.width {
        let bit = cursor.read_stream_bit(spec.bit_order)? as u8;
        match spec.bit_order {
            BitOrder::MostSignificantBitFirst => idx = (idx << 1) | bit,
            BitOrder::LeastSignificantBitFirst => idx |= bit << i,
        }
    }
    spec.alphabet
        .chars()
        .nth(idx as usize)
        .ok_or_else(|| VmError::InvalidValue {
            message: "invalid bits charset code unit".into(),
        })
}

fn terminator_suffix_matches(decoded: &str, term: &str, ignore_case: bool) -> bool {
    if ignore_case {
        decoded
            .to_ascii_lowercase()
            .ends_with(&term.to_ascii_lowercase())
    } else {
        decoded.ends_with(term)
    }
}

fn read_until_delimiters_bits_charset(
    cursor: &mut Cursor<'_>,
    patterns: &[DelimScanPattern],
    require_delimiter: bool,
    spec: crate::vm::encoding::BitsCharsetSpec,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let terms: alloc::vec::Vec<(alloc::string::String, bool)> = patterns
        .iter()
        .map(|p| (delimiter_pat_as_text(&p.pat), p.ignore_case))
        .filter(|(t, _)| !t.is_empty())
        .collect();

    let start_pos = cursor.pos;
    let start_bit_count = cursor.bit_count;
    let mut decoded = alloc::string::String::new();
    let mut matched_term_chars = 0usize;

    loop {
        if cursor.is_frame_consumed() {
            break;
        }
        let unit = read_bits_charset_code_unit(cursor, spec)?;
        decoded.push(unit);

        for (term, ignore_case) in &terms {
            if terminator_suffix_matches(&decoded, term, *ignore_case) {
                matched_term_chars = term.chars().count();
                break;
            }
        }
        if matched_term_chars > 0 {
            break;
        }
    }

    if matched_term_chars == 0 && require_delimiter {
        let pat = patterns.first().map(|p| p.pat.as_str()).unwrap_or("");
        return Err(VmError::InvalidValue {
            message: super::format_terminator_not_found_error(pat),
        });
    }

    let payload_chars = decoded.chars().count().saturating_sub(matched_term_chars);
    let payload_bits = payload_chars * spec.width as usize;

    cursor.pos = start_pos;
    cursor.bit_count = start_bit_count;
    cursor.read_stream_bits_as_bytes(payload_bits, spec.bit_order)
}

pub(crate) fn read_until_delimiters(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    stop_sequences: &[&IrProps],
    encoding: Option<&str>,
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
) -> Result<Vec<u8>, crate::error::VmError> {
    let patterns = enclosing_delimiter_scan_patterns(props, strings, stop_sequences, scan_ctx)?;
    if let Some(enc) = encoding {
        if let Some(spec) = bits_charset_spec(enc) {
            if !patterns.is_empty() {
                return read_until_delimiters_bits_charset(
                    cursor,
                    &patterns,
                    require_delimiter,
                    spec,
                );
            }
        }
    }
    if patterns.is_empty() {
        let abs = cursor.absolute_bit_index();
        let total_bits = cursor
            .frame_bit_limit
            .unwrap_or_else(|| cursor.data.len().saturating_mul(8));
        let remaining_bits = total_bits.saturating_sub(abs);
        if remaining_bits == 0 {
            return Ok(Vec::new());
        }
        if cursor.bit_count == 0 && remaining_bits.is_multiple_of(8) {
            let available = remaining_bits / 8;
            let byte_len = encoding
                .map(|enc| crate::vm::encoding::delimited_payload_byte_length(available, enc))
                .unwrap_or(available);
            let end = cursor.pos.saturating_add(byte_len);
            if end <= cursor.data.len() {
                let out = cursor.data[cursor.pos..end].to_vec();
                cursor.pos = end;
                return Ok(out);
            }
        }
        if cursor.bit_count != 0 || !remaining_bits.is_multiple_of(8) {
            return cursor.read_stream_bits_as_bytes(remaining_bits, props.bit_order);
        }
        let rest = cursor.data[cursor.pos..].to_vec();
        cursor.pos = cursor.data.len();
        cursor.bit_count = 0;
        return Ok(rest);
    }
    let escape_scheme = scan_ctx
        .and_then(|ctx| ctx.resolved_escape_scheme.as_ref())
        .or(props.escape_scheme.as_ref());
    read_until_any_delimiter(
        cursor,
        &patterns,
        require_delimiter,
        encoding,
        !stop_sequences.is_empty(),
        escape_scheme,
    )
}

pub(crate) fn read_until_separator(
    cursor: &mut Cursor<'_>,
    separator: &str,
    require_delimiter: bool,
    ignore_case: bool,
    escape_scheme: Option<&crate::schema::EscapeSchemeDef>,
) -> Result<Vec<u8>, crate::error::VmError> {
    let patterns = [DelimScanPattern {
        pat: separator.to_string(),
        ignore_case,
    }];
    read_until_any_delimiter(
        cursor,
        &patterns,
        require_delimiter,
        None,
        false,
        escape_scheme,
    )
}

pub(crate) fn read_delimited_bytes(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    stop_sequences: &[&IrProps],
) -> Result<Vec<u8>, crate::error::VmError> {
    read_until_delimiters(
        cursor,
        props,
        strings,
        require_delimiter,
        stop_sequences,
        None,
        None,
    )
}

fn read_until_any_delimiter(
    cursor: &mut Cursor<'_>,
    delimiters: &[DelimScanPattern],
    require_delimiter: bool,
    encoding: Option<&str>,
    allow_payload_at_eos: bool,
    escape_scheme: Option<&crate::schema::EscapeSchemeDef>,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let start = cursor.pos;
    let step = if utf16_little_endian_from_encoding(encoding).is_some() {
        2usize
    } else {
        1
    };
    while cursor.remaining() > 0 {
        for entry in delimiters {
            if let Some(n) = crate::schema::match_delimiter_opts_for_encoding(
                &cursor.data[cursor.pos..],
                &entry.pat,
                entry.ignore_case,
                encoding,
            ) {
                // Ignore zero-width delimiter matches while scanning (prevents infinite
                // empty reads on binary delimited fields); empty fields still work via n > 0.
                if n == 0 {
                    continue;
                }
                if n > 0 || (!require_delimiter && cursor.pos == start) {
                    return Ok(cursor.data[start..cursor.pos].to_vec());
                }
            }
        }
        let next = if let Some(scheme) = escape_scheme {
            crate::vm::escape::advance_escape_scan_index(cursor.data, cursor.pos, scheme)
        } else {
            cursor.pos + step
        };
        if next <= cursor.pos {
            cursor.advance(step);
        } else {
            cursor.pos = next;
        }
    }
    if cursor.pos == start {
        return Ok(Vec::new());
    }
    if require_delimiter {
        if allow_payload_at_eos && cursor.remaining() == 0 {
            return Ok(cursor.data[start..cursor.pos].to_vec());
        }
        let pat = delimiters.first().map(|p| p.pat.as_str()).unwrap_or("");
        return Err(VmError::InvalidValue {
            message: super::format_terminator_not_found_error(pat),
        });
    }
    Ok(cursor.data[start..].to_vec())
}

fn utf16_little_endian_from_encoding(encoding: Option<&str>) -> Option<bool> {
    encoding.and_then(|enc| match normalize_encoding_name(enc)? {
        "utf-16le" => Some(true),
        "utf-16be" => Some(false),
        _ => None,
    })
}

pub(crate) fn consume_bits_charset_delimiter(
    cursor: &mut Cursor<'_>,
    patterns: &[DelimScanPattern],
    spec: crate::vm::encoding::BitsCharsetSpec,
) -> Result<bool, crate::error::VmError> {
    for entry in patterns {
        let term = delimiter_pat_as_text(&entry.pat);
        if term.is_empty() {
            continue;
        }
        let save_pos = cursor.pos;
        let save_bit = cursor.bit_count;
        let mut decoded = alloc::string::String::new();
        for _ in 0..term.chars().count() {
            if cursor.is_frame_consumed() {
                cursor.pos = save_pos;
                cursor.bit_count = save_bit;
                break;
            }
            decoded.push(read_bits_charset_code_unit(cursor, spec)?);
        }
        let ok = if entry.ignore_case {
            decoded.eq_ignore_ascii_case(&term)
        } else {
            decoded == term
        };
        if ok {
            return Ok(true);
        }
        cursor.pos = save_pos;
        cursor.bit_count = save_bit;
    }
    Ok(false)
}

pub(crate) fn consume_enclosing_delimiter(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    stop_sequences: &[&IrProps],
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if cursor.is_empty() {
        return Ok(());
    }
    let field_patterns = non_empty_delimiter_scan_patterns(props, strings, scan_ctx)?;
    if let Ok(enc) = encoding_name(props, strings) {
        if let Some(spec) = bits_charset_spec(enc) {
            if consume_bits_charset_delimiter(cursor, &field_patterns, spec)? {
                return Ok(());
            }
            if !should_defer_parent_stop_delimiter(props) {
                for seq in stop_sequences {
                    let mut parent_patterns = alloc::vec::Vec::new();
                    for id in delimiter_pattern_ids(seq) {
                        let pat = stop_delimiter_literal(id, strings, scan_ctx)?;
                        push_delimiter_scan_patterns(&mut parent_patterns, &pat, seq.ignore_case);
                    }
                    if consume_bits_charset_delimiter(cursor, &parent_patterns, spec)? {
                        return Ok(());
                    }
                }
                if field_patterns.is_empty() && stop_sequences.is_empty() {
                    return Ok(());
                }
                return Err(VmError::InvalidValue {
                    message: "delimiter mismatch".into(),
                });
            }
            return Ok(());
        }
    }
    let enc = encoding_name(props, strings).ok();
    for entry in &field_patterns {
        if let Some(n) = crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            &entry.pat,
            entry.ignore_case,
            enc,
        ) {
            if n > 0 {
                cursor.advance(n);
                return Ok(());
            }
        }
    }
    if !should_defer_parent_stop_delimiter(props) {
        for seq in stop_sequences {
            for id in delimiter_pattern_ids(seq) {
                let pat = stop_delimiter_literal(id, strings, scan_ctx)?;
                if !pat.is_empty() {
                    if let Some(n) = crate::schema::match_delimiter_opts_for_encoding(
                        &cursor.data[cursor.pos..],
                        &pat,
                        seq.ignore_case,
                        enc,
                    ) {
                        if n == 0 {
                            continue;
                        }
                        if should_defer_sequence_stop_delimiter_in_field(seq, id, props, strings)? {
                            return Ok(());
                        }
                        if scan_ctx.is_some_and(|ctx| ctx.parent_infix_consumed_by_occurrence_loop)
                            && seq.separator_position == SeparatorPosition::Infix
                        {
                            return Ok(());
                        }
                        return Ok(());
                    }
                }
            }
        }
        if field_patterns.is_empty() && stop_sequences.is_empty() {
            return Ok(());
        }
        Err(VmError::InvalidValue {
            message: "delimiter mismatch".into(),
        })
    } else {
        Ok(())
    }
}

pub(crate) fn is_suppressible_empty_representation(
    value: &crate::value::DfdlValue,
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    match value {
        crate::value::DfdlValue::Null => {
            if props.nillable && nil_value_includes_empty(props, strings)? {
                return Ok(true);
            }
            Ok(nil_first_alternative(props, strings)?.is_some_and(|nil| nil.is_empty()))
        }
        crate::value::DfdlValue::String(text) if text.text.is_empty() => {
            if props.length_kind == LengthKind::Explicit && props.length.unwrap_or(0) > 0 {
                return Ok(false);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn trailing_suppressed_count(
    items: &[crate::value::DfdlValue],
    item_props: &IrProps,
    strings: &StringPool,
    seq_props: Option<&IrProps>,
) -> Result<usize, crate::error::VmError> {
    let policy = item_props
        .separator_suppression_policy
        .or_else(|| seq_props.and_then(|p| p.separator_suppression_policy));
    if !matches!(
        policy,
        Some(SeparatorSuppressionPolicy::TrailingEmpty)
            | Some(SeparatorSuppressionPolicy::TrailingEmptyStrict)
            | Some(SeparatorSuppressionPolicy::AnyEmpty)
    ) {
        return Ok(0);
    }
    let mut count = 0usize;
    for item in items.iter().rev() {
        if is_suppressible_empty_representation(item, item_props, strings)? {
            count += 1;
        } else {
            break;
        }
    }
    Ok(count)
}

/// When decoding, the next occurrence separator (before item `items.len()`) may be skipped
/// under `anyEmpty` if the previous occurrence was an empty representation.
pub(crate) fn should_suppress_decode_occurrence_separator(
    sep_props: &IrProps,
    item_props: &IrProps,
    items: &[crate::value::DfdlValue],
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    if items.is_empty() {
        return Ok(false);
    }
    let policy = sep_props
        .separator_suppression_policy
        .or(item_props.separator_suppression_policy);
    if policy != Some(SeparatorSuppressionPolicy::AnyEmpty) {
        return Ok(false);
    }
    match sep_props.separator_position {
        SeparatorPosition::Prefix | SeparatorPosition::Postfix => Ok(
            is_suppressible_empty_representation(&items[items.len() - 1], item_props, strings)?,
        ),
        SeparatorPosition::Infix => Ok(false),
    }
}

pub(crate) fn should_suppress_occurrence_separator(
    sep_props: &IrProps,
    item_props: &IrProps,
    items: &[crate::value::DfdlValue],
    index: usize,
    before_item: bool,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    let policy = sep_props
        .separator_suppression_policy
        .or(item_props.separator_suppression_policy);
    if policy != Some(SeparatorSuppressionPolicy::AnyEmpty) {
        return Ok(false);
    }
    let current_empty = is_suppressible_empty_representation(&items[index], item_props, strings)?;
    if before_item {
        match sep_props.separator_position {
            SeparatorPosition::Prefix | SeparatorPosition::Postfix => Ok(current_empty),
            SeparatorPosition::Infix => {
                let prev_empty = if index > 0 {
                    is_suppressible_empty_representation(&items[index - 1], item_props, strings)?
                } else {
                    false
                };
                Ok(current_empty || prev_empty)
            }
        }
    } else {
        Ok(matches!(sep_props.separator_position, SeparatorPosition::Postfix) && current_empty)
    }
}
