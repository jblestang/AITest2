pub(crate) mod choice;
pub(crate) mod element;
pub(crate) mod helpers;
pub(crate) mod ivc;
pub(crate) mod particle;
pub(crate) mod sequence;
pub(crate) mod xpath;

pub(crate) use choice::*;
pub(crate) use element::*;
pub(crate) use helpers::*;
pub(crate) use ivc::*;
pub(crate) use sequence::*;
pub(crate) use xpath::*;

use super::runtime::{Cursor, RuntimeConfig, VmContext};
use crate::error::Result;
use crate::ir::{IrProgram, IrProps};
use crate::schema::BitOrder;
use crate::value::{DfdlValue, FieldDelimiterMeta};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

#[derive(Debug, Clone)]
pub(crate) struct SiblingState {
    pub(crate) value: DfdlValue,
    pub(crate) content_bytes: usize,
}

/// DFDL decoder VM — executes compiled IR against an input byte stream.
pub struct Decoder<'a> {
    ctx: VmContext<'a>,
    /// Enclosing complex elements (for terminator/separator conflict detection).
    enclosing: RefCell<Vec<IrProps>>,
    /// Element names aligned with [`Self::enclosing`] (for separator error context).
    enclosing_names: RefCell<Vec<String>>,
    /// Per-field delimiter alternative indices captured during the current sequence child.
    field_delimiters: RefCell<BTreeMap<String, FieldDelimiterMeta>>,
    /// Previous sibling `dfdl:bitOrder` within the innermost open sequence (runtime SDE).
    seq_bit_order: RefCell<Option<BitOrder>>,
    /// Parent postfix separator already consumed by separator-bounded implicit complex decode.
    parent_postfix_sep_consumed: RefCell<bool>,
    /// Nesting depth of [`Self::decode_element_occurrences`] (for postfix-sep flag lifetime).
    occurrence_decode_depth: Cell<u32>,
    /// Parent postfix separator consumed by the most recent separator-bounded complex decode.
    postfix_bounded_parent_sep_consumed: Cell<bool>,
    /// Parent infix between array occurrences is consumed by the occurrence loop, not fields.
    parent_infix_consumed_by_occurrence_loop: Cell<bool>,
    /// Runtime DFDL variable values (`defineVariable` + `setVariable`).
    runtime_variables: RefCell<BTreeMap<String, String>>,
    /// XPath sibling context for the active sequence decode (includes hidden elements).
    xpath_siblings: RefCell<BTreeMap<String, SiblingState>>,
    /// Outer sequence sibling maps for `../` / `../../` assert and xpath evaluation.
    xpath_ancestor_frames: RefCell<Vec<BTreeMap<String, SiblingState>>>,
    /// Set when a choice branch discriminator evaluates true (commits outer choice).
    discriminator_committed_branch: Cell<bool>,
    delimiter_occurrence_stack: RefCell<Vec<u64>>,
}

impl<'a> Decoder<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self::with_config(program, RuntimeConfig::default())
    }

    pub fn with_config(program: &'a IrProgram, config: RuntimeConfig) -> Self {
        Self {
            ctx: VmContext { program, config },
            enclosing: RefCell::new(Vec::new()),
            enclosing_names: RefCell::new(Vec::new()),
            field_delimiters: RefCell::new(BTreeMap::new()),
            seq_bit_order: RefCell::new(None),
            parent_postfix_sep_consumed: RefCell::new(false),
            occurrence_decode_depth: Cell::new(0),
            postfix_bounded_parent_sep_consumed: Cell::new(false),
            parent_infix_consumed_by_occurrence_loop: Cell::new(false),
            runtime_variables: RefCell::new(BTreeMap::new()),
            xpath_siblings: RefCell::new(BTreeMap::new()),
            xpath_ancestor_frames: RefCell::new(Vec::new()),
            discriminator_committed_branch: Cell::new(false),
            delimiter_occurrence_stack: RefCell::new(alloc::vec![1]),
        }
    }

    /// Decode one logical value from `input`.
    pub fn decode(&self, input: &[u8]) -> Result<DfdlValue> {
        self.decode_with_bit_limit(input, None, None)
    }

    /// Decode with an optional significant bit length (TDML `type="bits"` documents).
    pub fn decode_with_bit_limit(
        &self,
        input: &[u8],
        frame_bits: Option<usize>,
        transmission_bit_order: Option<crate::schema::BitOrder>,
    ) -> Result<DfdlValue> {
        self.decode_with_tdml_options(input, frame_bits, transmission_bit_order, None)
    }

    pub fn decode_with_tdml_options(
        &self,
        input: &[u8],
        frame_bits: Option<usize>,
        transmission_bit_order: Option<crate::schema::BitOrder>,
        tdml_bit_order_regions: Option<alloc::vec::Vec<(crate::schema::BitOrder, usize)>>,
    ) -> Result<DfdlValue> {
        use crate::schema::BitOrder;
        let transmission = transmission_bit_order.unwrap_or(BitOrder::MostSignificantBitFirst);
        let mut cursor = match frame_bits {
            Some(bits) => Cursor::with_frame_bits_and_transmission(input, bits, transmission),
            None => {
                let mut c = Cursor::new(input);
                c.transmission_bit_order = transmission;
                c
            }
        };
        cursor.tdml_bit_order_regions = tdml_bit_order_regions;
        let mut vars = self.ctx.program.variables.clone();
        vars.entry("dfdl:encoding".to_string())
            .or_insert_with(|| "UTF-8".to_string());
        vars.entry("encoding".to_string())
            .or_insert_with(|| "UTF-8".to_string());
        vars.entry("dfdl:byteOrder".to_string())
            .or_insert_with(|| "bigEndian".to_string());
        vars.entry("byteOrder".to_string())
            .or_insert_with(|| "bigEndian".to_string());
        vars.entry("dfdl:binaryFloatRep".to_string())
            .or_insert_with(|| "ieee".to_string());
        vars.entry("binaryFloatRep".to_string())
            .or_insert_with(|| "ieee".to_string());
        vars.entry("dfdl:outputNewLine".to_string())
            .or_insert_with(|| "%LF;".to_string());
        vars.entry("outputNewLine".to_string())
            .or_insert_with(|| "%LF;".to_string());
        for (k, v) in &self.ctx.config.runtime_variable_overrides {
            vars.insert(k.clone(), v.clone());
            let local = k.rsplit(':').next().unwrap_or(k.as_str());
            if local != k.as_str() {
                vars.insert(local.to_string(), v.clone());
            }
        }
        *self.runtime_variables.borrow_mut() = vars;
        self.xpath_siblings.borrow_mut().clear();
        self.xpath_ancestor_frames.borrow_mut().clear();
        let value = self.decode_node(
            self.ctx.program.root,
            &mut cursor,
            false,
            None,
            None,
            None,
            false,
            &[],
        )?;
        self.consume_root_delimited_suffix(&mut cursor)?;
        if self.ctx.config.strict_eos {
            if let Some(limit) = cursor.frame_bit_limit {
                let consumed = cursor.absolute_bit_index();
                if consumed < limit {
                    let remaining_bits = limit.saturating_sub(consumed);
                    return Err(crate::error::VmError::TrailingData {
                        consumed_bits: consumed,
                        remaining_bits,
                    }
                    .into());
                }
            } else if cursor.bit_count == 0 && cursor.remaining() > 0 {
                let total_bits = cursor.data.len().saturating_mul(8);
                let remaining_bits = cursor.remaining().saturating_mul(8);
                return Err(crate::error::VmError::TrailingData {
                    consumed_bits: total_bits.saturating_sub(remaining_bits),
                    remaining_bits,
                }
                .into());
            }
        }
        Ok(wrap_root(&self.ctx.program.root_element, value))
    }
}
