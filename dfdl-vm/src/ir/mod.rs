mod builder;

pub use builder::{compile, compile_named};
use crate::error::VmError;
use crate::schema::{
    BinaryFloatRep, BinaryNumberRep, BitOrder, ByteOrder, LengthKind, LengthUnits, Representation,
    SeparatorPosition, SequenceKind, TextTrimKind,
};
use alloc::string::String;
use alloc::vec::Vec;

/// Compiled in-memory intermediate representation executed by the DFDL VM.
#[derive(Debug, Clone, PartialEq)]
pub struct IrProgram {
    pub root_element: String,
    pub root: u32,
    pub nodes: Vec<IrNode>,
    pub strings: StringPool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrNode {
    Sequence {
        children: Vec<u32>,
        props: IrProps,
    },
    Choice {
        branches: Vec<ChoiceBranch>,
        props: IrProps,
    },
    Element {
        name: StringId,
        kind: ValueKind,
        props: IrProps,
        child: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceBranch {
    pub name: StringId,
    pub initiator: Option<StringId>,
    pub node: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Boolean,
    Int,
    Long,
    Short,
    Byte,
    UnsignedInt,
    UnsignedShort,
    UnsignedByte,
    Float,
    Double,
    Decimal,
    DateTime,
    String,
    HexBinary,
    Complex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrPrefixLength {
    pub kind: ValueKind,
    pub props: IrProps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrProps {
    pub representation: Representation,
    pub byte_order: ByteOrder,
    pub bit_order: BitOrder,
    pub length_kind: LengthKind,
    pub length: Option<u64>,
    pub length_sibling: Option<StringId>,
    pub length_units: LengthUnits,
    pub encoding: StringId,
    pub text_trim_kind: TextTrimKind,
    pub text_number_pad_character: Option<StringId>,
    pub text_string_pad_character: Option<StringId>,
    pub binary_number_rep: BinaryNumberRep,
    pub binary_calendar_rep: BinaryNumberRep,
    pub binary_float_rep: BinaryFloatRep,
    pub binary_decimal_virtual_point: u32,
    pub calendar_pattern: Option<StringId>,
    pub initiator: Option<StringId>,
    pub terminator: Option<StringId>,
    pub separator: Option<StringId>,
    pub occurs_min: u64,
    pub occurs_max: Option<u64>,
    pub length_pattern: Option<StringId>,
    pub separator_position: SeparatorPosition,
    pub text_boolean_true_rep: Option<StringId>,
    pub text_boolean_false_rep: Option<StringId>,
    pub default_value: Option<StringId>,
    pub sequence_kind: SequenceKind,
    pub alignment: u64,
    pub alignment_units: LengthUnits,
    pub fill_byte: u8,
    pub prefix_length: Option<alloc::boxed::Box<IrPrefixLength>>,
    pub prefix_includes_prefix_length: bool,
}

impl Default for IrProps {
    fn default() -> Self {
        Self {
            representation: Representation::Binary,
            byte_order: ByteOrder::BigEndian,
            bit_order: BitOrder::MostSignificantBitFirst,
            length_kind: LengthKind::Implicit,
            length: None,
            length_sibling: None,
            length_units: LengthUnits::Bytes,
            encoding: StringId(0),
            text_trim_kind: TextTrimKind::None,
            text_number_pad_character: None,
            text_string_pad_character: None,
            binary_number_rep: BinaryNumberRep::Binary,
            binary_calendar_rep: BinaryNumberRep::Binary,
            binary_float_rep: BinaryFloatRep::Ieee,
            binary_decimal_virtual_point: 0,
            calendar_pattern: None,
            initiator: None,
            terminator: None,
            separator: None,
            occurs_min: 1,
            occurs_max: Some(1),
            length_pattern: None,
            separator_position: SeparatorPosition::Infix,
            text_boolean_true_rep: None,
            text_boolean_false_rep: None,
            default_value: None,
            sequence_kind: SequenceKind::Ordered,
            alignment: 0,
            alignment_units: LengthUnits::Bytes,
            fill_byte: 0,
            prefix_length: None,
            prefix_includes_prefix_length: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StringPool {
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StringId(pub u32);

impl StringPool {
    pub fn new() -> Self {
        let mut pool = Self { values: Vec::new() };
        pool.intern("UTF-8");
        pool
    }

    pub fn intern(&mut self, value: impl Into<String>) -> StringId {
        let value = value.into();
        if let Some(idx) = self.values.iter().position(|v| v == &value) {
            return StringId(idx as u32);
        }
        let id = StringId(self.values.len() as u32);
        self.values.push(value);
        id
    }

    pub fn get(&self, id: StringId) -> Result<&str, VmError> {
        self.values
            .get(id.0 as usize)
            .map(|s| s.as_str())
            .ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("invalid string pool id {}", id.0),
            })
    }
}

impl IrProgram {
    pub fn node(&self, id: u32) -> Result<&IrNode, VmError> {
        self.nodes.get(id as usize).ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("invalid IR node id {id}"),
        })
    }
}
