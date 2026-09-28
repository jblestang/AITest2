use super::helpers::*;
use super::ovc::{
    eval_output_value_calc, ovc_deferred_to_encode_occurrence, ovc_value_for_element_kind,
};
use super::Encoder;
use crate::error::{Error, Result, VmError};
use crate::ir::{IrProps, ValueKind};
use crate::schema::{SeparatorPosition, TextPadKind};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use super::super::runtime::{
    is_suppressible_empty_representation, nil_unparse_bytes_for_encode,
    should_suppress_occurrence_separator, trailing_suppressed_count,
    write_alignment_with_config, write_byte_aligned,
    write_framed_payload, write_simple,
};

impl<'a> Encoder<'a> {
    pub(crate) fn encode_framed_element(
        &self,
        child_id: u32,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        field_name: Option<&str>,
        delim_meta: Option<&crate::value::FieldDelimiterMeta>,
        encode_scope: Option<&BTreeMap<String, DfdlValue>>,
        encode_siblings: Option<&BTreeMap<String, DfdlValue>>,
        sequence_particle: bool,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed = trailing_suppressed_count(items, props, self.ctx.strings(), None)?;
        let encode_len = items.len().saturating_sub(suppressed);
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            if !sequence_particle || encode_len > 1 {
                self.write_occurrence_separator(props, out, bit_count, idx, encode_len)?;
            }
            write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config))?;
            if let Some(id) = props.initiator {
                let raw = self.ctx.strings().get(id)?;
                if !raw.is_empty() {
                    let bytes = self.encode_framing_property_pattern(
                        raw,
                        props,
                        encode_siblings,
                        delim_meta.and_then(|m| m.initiator_alt),
                    )?;
                    write_byte_aligned(out, bit_count, &bytes).map_err(Error::from)?;
                }
            }
            let mut payload = Vec::new();
            let mut payload_bit_count = 0u8;
            if matches!(item, DfdlValue::Null) {
                let nil_bytes =
                    nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
                write_byte_aligned(&mut payload, &mut payload_bit_count, &nil_bytes)
                    .map_err(Error::from)?;
            } else {
                self.encode_node(
                    child_id,
                    item,
                    &mut payload,
                    &mut payload_bit_count,
                    encode_scope,
                )?;
            }
            write_framed_payload(
                out,
                bit_count,
                &payload,
                payload_bit_count,
                props,
                self.ctx.strings(),
                Some(&self.ctx.config),
                field_name,
                delim_meta,
                encode_siblings,
            )?;
        }
        Ok(())
    }

    pub(crate) fn encode_element_occurrences(
        &self,
        node_id: u32,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        sep_props: &IrProps,
        encode_scope: Option<&BTreeMap<String, DfdlValue>>,
        encode_siblings: Option<&BTreeMap<String, DfdlValue>>,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed =
            trailing_suppressed_count(items, props, self.ctx.strings(), Some(sep_props))?;
        let encode_len = items.len().saturating_sub(suppressed);
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            if sep_props.separator_position != SeparatorPosition::Postfix
                && !should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    true,
                    self.ctx.strings(),
                )?
            {
                self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
            }
            write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config))?;
            self.check_bit_order_change(props, *bit_count)?;
            self.write_initiator(props, out, bit_count, None, encode_siblings)?;
            if matches!(item, DfdlValue::Null) {
                let nil_bytes =
                    nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
                write_byte_aligned(out, bit_count, &nil_bytes).map_err(Error::from)?;
            } else {
                self.with_array_occurrence(idx + 1, encode_len, || {
                    self.encode_node(node_id, item, out, bit_count, encode_scope)
                })?;
            }
            self.write_terminator(props, out, bit_count, None, encode_siblings)?;
            self.note_bit_order(props);
            if sep_props.separator_position == SeparatorPosition::Postfix
                && !should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    false,
                    self.ctx.strings(),
                )?
            {
                self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
            }
        }
        Ok(())
    }

    pub(crate) fn encode_simple_occurrences(
        &self,
        kind: ValueKind,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        field_name: Option<&str>,
        delim_meta: Option<&crate::value::FieldDelimiterMeta>,
        sep_props: &IrProps,
        encode_siblings: Option<&BTreeMap<String, DfdlValue>>,
        encode_scope: Option<&BTreeMap<String, DfdlValue>>,
        sequence_particle: bool,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed =
            trailing_suppressed_count(items, props, self.ctx.strings(), Some(sep_props))?;
        let encode_len = items.len().saturating_sub(suppressed);
        let empty = BTreeMap::new();
        let sibling_lookup = merged_encode_lookup(encode_scope, encode_siblings.unwrap_or(&empty));
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            if sep_props.separator_position != SeparatorPosition::Postfix {
                let suppress = should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    true,
                    self.ctx.strings(),
                )?;
                if !suppress && (!sequence_particle || encode_len > 1) {
                    self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
                }
            }
            if is_suppressible_empty_representation(item, props, self.ctx.strings())?
                && !matches!(item, DfdlValue::Null)
            {
                let pad_empty = kind == ValueKind::String
                    && props.text_pad_kind == TextPadKind::PadChar;
                if !pad_empty {
                    continue;
                }
            }
            write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config))?;
            self.check_bit_order_change(props, *bit_count)?;
            write_simple(
                out,
                bit_count,
                item,
                kind,
                props,
                self.ctx.strings(),
                &self.ctx.program.tunables,
                &self.ctx.config,
                field_name,
                delim_meta,
                Some(&sibling_lookup),
                Some(sep_props),
            )
            .map_err(Error::from)?;
            self.note_bit_order(props);
            if sep_props.separator_position == SeparatorPosition::Postfix
                && !should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    false,
                    self.ctx.strings(),
                )?
                && (!sequence_particle || encode_len > 1)
            {
                self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
            }
        }
        Ok(())
    }

    pub(crate) fn element_encode_value(
        &self,
        kind: ValueKind,
        props: &IrProps,
        key: &str,
        map: &BTreeMap<String, DfdlValue>,
        sequence_children: &[u32],
    ) -> Result<DfdlValue> {
        if props.output_value_calc.is_some() || props.output_value_calc_conditional {
            let local = crate::xml_util::local_name_str(key);
            if !ovc_deferred_to_encode_occurrence(props) && !props.output_value_calc_conditional {
                if let Some(k) = map_has_local_key(map, local) {
                    if let Some(v) = map.get(&k) {
                        return Ok(v.clone());
                    }
                }
                if let Some(v) = map.get(key) {
                    return Ok(v.clone());
                }
            }
            let computed = eval_output_value_calc(self, props, map, sequence_children, props)?;
            return Ok(ovc_value_for_element_kind(kind, computed));
        }
        if props.hidden {
            if let Some(default_id) = props.default_value {
                let text = self.ctx.strings().get(default_id)?;
                return Ok(DfdlValue::string(text));
            }
            return Ok(DfdlValue::sequence(BTreeMap::new()));
        }
        if let Some(v) = map.get(key) {
            return Ok(v.clone());
        }
        Err(VmError::MissingField { name: key.into() }.into())
    }

    pub(crate) fn encode_nil_element(
        &self,
        _kind: ValueKind,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
    ) -> Result<()> {
        write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config))
            .map_err(Error::from)?;
        self.write_initiator(props, out, bit_count, None, None)?;
        let payload =
            nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
        write_byte_aligned(out, bit_count, &payload).map_err(Error::from)?;
        self.write_terminator(props, out, bit_count, None, None)?;
        Ok(())
    }
}
