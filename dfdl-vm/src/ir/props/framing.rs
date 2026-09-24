use crate::ir::StringId;
use crate::schema::{LengthUnits, NilKind, SeparatorPosition, SeparatorSuppressionPolicy};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct FramingProps {
    pub initiator: Option<StringId>,
    pub terminator: Option<StringId>,
    pub separator: Option<StringId>,
    pub output_new_line: Option<StringId>,
    pub output_new_line_sibling: Option<StringId>,
    pub separator_position: SeparatorPosition,
    pub alignment: u64,
    pub alignment_implicit: bool,
    pub alignment_units: LengthUnits,
    pub framing_alignment: u64,
    pub framing_alignment_units: LengthUnits,
    pub leading_skip: u64,
    pub trailing_skip: u64,
    pub fill_byte: u8,
    pub fill_byte_defined: bool,
    pub fill_byte_explicit: bool,
    pub fill_byte_utf8: Option<Vec<u8>>,
    pub nil_kind: Option<NilKind>,
    pub nil_value: Option<StringId>,
    pub separator_suppression_policy: Option<SeparatorSuppressionPolicy>,
}

impl Default for FramingProps {
    fn default() -> Self {
        Self {
            initiator: None,
            terminator: None,
            separator: None,
            output_new_line: None,
            output_new_line_sibling: None,
            separator_position: SeparatorPosition::Infix,
            alignment: 1,
            alignment_implicit: false,
            alignment_units: LengthUnits::Bytes,
            framing_alignment: 0,
            framing_alignment_units: LengthUnits::Bytes,
            leading_skip: 0,
            trailing_skip: 0,
            fill_byte: 0,
            fill_byte_defined: false,
            fill_byte_explicit: false,
            fill_byte_utf8: None,
            nil_kind: None,
            nil_value: None,
            separator_suppression_policy: None,
        }
    }
}
