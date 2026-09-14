mod builder;

pub use builder::{compile, compile_named, compile_named_with_tunables};
use crate::error::VmError;
use crate::schema::{
    BinaryFloatRep, BinaryNumberCheckPolicy, BinaryNumberRep, BitOrder, ByteOrder,
    EncodingErrorPolicy, InputValueCalc, InputValueCalcSegment,
    LengthKind, LengthUnits, NilKind, ObjectKind, OccursCountKind, OutputValueCalc,
    Representation, SeparatorPosition, SeparatorSuppressionPolicy, SequenceKind,
    TextNumberJustification, TextPadKind, TextStringJustification, TextTrimKind,
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
    pub tunables: crate::length_validate::DaffodilTunables,
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
    Integer,
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
    Time,
    String,
    HexBinary,
    Complex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrPrefixLength {
    pub kind: ValueKind,
    pub props: IrProps,
    pub min_inclusive: Option<i64>,
    pub max_inclusive: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrInputValueCalcSegment {
    Sibling(StringId),
    Substring {
        sibling: StringId,
        start: u32,
        length: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrProps {
    pub representation: Representation,
    pub byte_order: ByteOrder,
    /// True when `dfdl:byteOrder` was present on merged DFDL props (not schema default only).
    pub byte_order_defined: bool,
    pub bit_order: BitOrder,
    /// True when `dfdl:bitOrder` was present on merged DFDL props.
    pub bit_order_defined: bool,
    pub length_kind: LengthKind,
    pub length: Option<u64>,
    pub length_sibling: Option<StringId>,
    pub length_sibling_cast_long: bool,
    pub length_expr_unparsed: bool,
    pub length_units: LengthUnits,
    pub encoding: StringId,
    pub encoding_error_policy: EncodingErrorPolicy,
    pub text_trim_kind: TextTrimKind,
    pub text_pad_kind: TextPadKind,
    pub truncate_specified_length_string: bool,
    pub text_number_pad_character: Option<StringId>,
    pub text_string_pad_character: Option<StringId>,
    pub text_string_pad_character_property_form: bool,
    pub binary_number_rep: BinaryNumberRep,
    pub binary_packed_sign_codes: StringId,
    pub binary_number_check_policy: BinaryNumberCheckPolicy,
    pub binary_calendar_rep: BinaryNumberRep,
    pub binary_calendar_epoch: Option<StringId>,
    pub binary_float_rep: BinaryFloatRep,
    pub binary_decimal_virtual_point: u32,
    pub binary_decimal_virtual_point_signed: Option<i32>,
    pub decimal_signed: bool,
    pub calendar_pattern: Option<StringId>,
    pub calendar_pattern_kind: crate::schema::CalendarPatternKind,
    pub text_number_pattern: Option<StringId>,
    pub text_number_check_policy: BinaryNumberCheckPolicy,
    pub text_number_rep: crate::schema::TextNumberRep,
    pub text_number_rounding: crate::schema::TextNumberRounding,
    pub text_number_rounding_increment: StringId,
    pub text_number_rounding_mode: crate::schema::TextNumberRoundingMode,
    pub text_number_rounding_increment_defined: bool,
    pub text_zoned_sign_style: Option<crate::schema::TextZonedSignStyle>,
    pub text_standard_decimal_separator: StringId,
    /// Set when `textStandardDecimalSeparator` was present on merged DFDL props.
    pub text_standard_decimal_separator_defined: bool,
    pub text_standard_decimal_separator_sibling: Option<StringId>,
    pub text_standard_grouping_separator: Option<StringId>,
    pub text_standard_grouping_separator_defined: bool,
    pub text_standard_grouping_separator_sibling: Option<StringId>,
    pub text_standard_exponent_rep: StringId,
    pub text_standard_exponent_rep_sibling: Option<StringId>,
    pub text_standard_infinity_rep: StringId,
    pub text_standard_nan_rep: StringId,
    pub text_standard_zero_rep: StringId,
    pub text_standard_zero_rep_defined: bool,
    pub text_standard_exponent_rep_defined: bool,
    /// Runtime-resolved separator literals (from `{ ../sibling }` expressions).
    pub resolved_text_standard_decimal_separator: Option<alloc::string::String>,
    pub resolved_text_standard_grouping_separator: Option<alloc::string::String>,
    pub resolved_text_standard_exponent_rep: Option<alloc::string::String>,
    /// Element/type declared `textNumberPattern` (not only the format default).
    pub custom_text_number_pattern: bool,
    pub initiator: Option<StringId>,
    pub terminator: Option<StringId>,
    pub separator: Option<StringId>,
    pub output_new_line: Option<StringId>,
    pub occurs_min: u64,
    pub occurs_max: Option<u64>,
    pub length_pattern: Option<StringId>,
    pub separator_position: SeparatorPosition,
    pub text_boolean_true_rep: Option<StringId>,
    pub text_boolean_false_rep: Option<StringId>,
    pub text_boolean_true_rep_defined: bool,
    pub text_boolean_false_rep_defined: bool,
    pub text_boolean_pad_character: Option<StringId>,
    pub binary_boolean_true_rep: Option<u64>,
    pub binary_boolean_true_rep_defined: bool,
    pub binary_boolean_false_rep: Option<u64>,
    pub binary_boolean_false_rep_defined: bool,
    pub default_value: Option<StringId>,
    pub sequence_kind: SequenceKind,
    pub alignment: u64,
    pub alignment_implicit: bool,
    pub alignment_units: LengthUnits,
    /// Pre-element alignment from type/format before an element `dfdl:alignment` override (0 = none).
    pub framing_alignment: u64,
    pub framing_alignment_units: LengthUnits,
    pub leading_skip: u64,
    pub trailing_skip: u64,
    pub fill_byte: u8,
    /// True when `dfdl:fillByte` was explicitly set (not cleared with `%NUL;`).
    pub fill_byte_defined: bool,
    /// Expanded `dfdl:fillByte` bytes (for runtime SDE when encoding is computed).
    pub fill_byte_utf8: Option<alloc::vec::Vec<u8>>,
    pub input_value_calc: Option<InputValueCalc>,
    pub input_value_calc_literal: Option<StringId>,
    pub input_value_calc_sibling: Option<StringId>,
    pub input_value_calc_segments: Option<Vec<IrInputValueCalcSegment>>,
    /// True when the XSD type is `xs:date` (vs `xs:dateTime`).
    pub calendar_date_only: bool,
    pub output_value_calc: Option<OutputValueCalc>,
    pub output_value_calc_sibling: Option<StringId>,
    pub text_string_justification: TextStringJustification,
    pub text_number_justification: TextNumberJustification,
    pub text_standard_base: u32,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    /// Fixed length from `xs:length` facet.
    pub facet_length: Option<u64>,
    /// Fixed representation span for `lengthKind=implicit` when minLength=maxLength.
    pub implicit_facet_length: Option<u64>,
    /// Each entry is one restriction-level pattern group (OR within, AND across groups).
    pub facet_pattern_groups: alloc::vec::Vec<StringId>,
    pub value_min_inclusive: Option<i64>,
    pub value_max_inclusive: Option<i64>,
    pub value_min_exclusive: Option<i64>,
    pub value_max_exclusive: Option<i64>,
    pub total_digits: Option<u64>,
    pub fraction_digits: Option<u64>,
    pub facet_check_constraints: bool,
    pub facet_assert_message: Option<StringId>,
    pub prefix_length: Option<alloc::boxed::Box<IrPrefixLength>>,
    pub prefix_includes_prefix_length: bool,
    pub nillable: bool,
    pub nil_kind: Option<NilKind>,
    pub nil_value: Option<StringId>,
    pub separator_suppression_policy: Option<SeparatorSuppressionPolicy>,
    pub occurs_count_kind: OccursCountKind,
    /// When true, parsed value is not placed in the infoset (hidden model group member).
    pub hidden: bool,
    pub ignore_case: bool,
    pub initiated_content: bool,
    /// When true, treat Long bit fields as unsigned (xs:unsignedLong).
    pub unsigned_integer: bool,
    /// xs:nonNegativeInteger (unbounded, non-negative).
    pub non_negative_integer: bool,
    pub object_kind: ObjectKind,
    /// XSD type QName for union/facet post-decode validation (element declaration).
    pub xsd_type: Option<StringId>,
}

impl Default for IrProps {
    fn default() -> Self {
        Self {
            representation: Representation::Binary,
            byte_order: ByteOrder::BigEndian,
            byte_order_defined: false,
            bit_order: BitOrder::MostSignificantBitFirst,
            bit_order_defined: false,
            length_kind: LengthKind::Implicit,
            length: None,
            length_sibling: None,
            length_sibling_cast_long: false,
            length_expr_unparsed: false,
            length_units: LengthUnits::Bytes,
            encoding: StringId(0),
            encoding_error_policy: EncodingErrorPolicy::Error,
            text_trim_kind: TextTrimKind::None,
            text_pad_kind: TextPadKind::PadChar,
            truncate_specified_length_string: false,
            text_number_pad_character: None,
            text_string_pad_character: None,
            text_string_pad_character_property_form: false,
            binary_number_rep: BinaryNumberRep::Binary,
            binary_packed_sign_codes: StringId(0),
            binary_number_check_policy: BinaryNumberCheckPolicy::Lax,
            binary_calendar_rep: BinaryNumberRep::Binary,
            binary_calendar_epoch: None,
            binary_float_rep: BinaryFloatRep::Ieee,
            binary_decimal_virtual_point: 0,
            binary_decimal_virtual_point_signed: None,
            decimal_signed: true,
            calendar_pattern: None,
            calendar_pattern_kind: crate::schema::CalendarPatternKind::Implicit,
            text_number_pattern: None,
            text_number_check_policy: BinaryNumberCheckPolicy::Lax,
            text_number_rep: crate::schema::TextNumberRep::Standard,
            text_number_rounding: crate::schema::TextNumberRounding::Pattern,
            text_number_rounding_increment: StringId(0),
            text_number_rounding_mode: crate::schema::TextNumberRoundingMode::RoundHalfEven,
            text_number_rounding_increment_defined: false,
            text_zoned_sign_style: None,
            text_standard_decimal_separator: StringId(0),
            text_standard_decimal_separator_defined: false,
            text_standard_decimal_separator_sibling: None,
            text_standard_exponent_rep: StringId(0),
            text_standard_exponent_rep_sibling: None,
            text_standard_infinity_rep: StringId(0),
            text_standard_nan_rep: StringId(0),
            text_standard_zero_rep: StringId(0),
            text_standard_zero_rep_defined: false,
            text_standard_exponent_rep_defined: false,
            resolved_text_standard_decimal_separator: None,
            resolved_text_standard_grouping_separator: None,
            resolved_text_standard_exponent_rep: None,
            text_standard_grouping_separator: None,
            text_standard_grouping_separator_defined: false,
            text_standard_grouping_separator_sibling: None,
            custom_text_number_pattern: false,
            initiator: None,
            terminator: None,
            separator: None,
            output_new_line: None,
            occurs_min: 1,
            occurs_max: Some(1),
            length_pattern: None,
            separator_position: SeparatorPosition::Infix,
            text_boolean_true_rep: None,
            text_boolean_false_rep: None,
            text_boolean_true_rep_defined: false,
            text_boolean_false_rep_defined: false,
            text_boolean_pad_character: None,
            binary_boolean_true_rep: None,
            binary_boolean_true_rep_defined: false,
            binary_boolean_false_rep: None,
            binary_boolean_false_rep_defined: false,
            default_value: None,
            sequence_kind: SequenceKind::Ordered,
            alignment: 0,
            alignment_implicit: false,
            alignment_units: LengthUnits::Bytes,
            framing_alignment: 0,
            framing_alignment_units: LengthUnits::Bytes,
            leading_skip: 0,
            trailing_skip: 0,
            fill_byte: 0,
            fill_byte_defined: false,
            fill_byte_utf8: None,
            input_value_calc: None,
            input_value_calc_literal: None,
            input_value_calc_sibling: None,
            input_value_calc_segments: None,
            calendar_date_only: false,
            output_value_calc: None,
            output_value_calc_sibling: None,
            text_string_justification: TextStringJustification::Left,
            text_number_justification: TextNumberJustification::Right,
            text_standard_base: 10,
            min_length: None,
            max_length: None,
            facet_length: None,
            implicit_facet_length: None,
            facet_pattern_groups: alloc::vec::Vec::new(),
            value_min_inclusive: None,
            value_max_inclusive: None,
            value_min_exclusive: None,
            value_max_exclusive: None,
            total_digits: None,
            fraction_digits: None,
            facet_check_constraints: false,
            facet_assert_message: None,
            prefix_length: None,
            prefix_includes_prefix_length: false,
            nillable: false,
            nil_kind: None,
            nil_value: None,
            separator_suppression_policy: None,
            occurs_count_kind: OccursCountKind::Parsed,
            hidden: false,
            ignore_case: false,
            initiated_content: false,
            unsigned_integer: false,
            non_negative_integer: false,
            object_kind: ObjectKind::Normal,
            xsd_type: None,
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

    pub fn lookup(&self, value: &str) -> Option<StringId> {
        self.values
            .iter()
            .position(|v| v == value)
            .map(|idx| StringId(idx as u32))
    }
}

impl IrProgram {
    pub fn node(&self, id: u32) -> Result<&IrNode, VmError> {
        self.nodes.get(id as usize).ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("invalid IR node id {id}"),
        })
    }

    /// Merged format `bitOrder` on the root particle (stream bit extraction within bytes).
    pub fn format_transmission_bit_order(&self) -> BitOrder {
        match self.node(self.root) {
            Ok(IrNode::Element { props, .. }) | Ok(IrNode::Sequence { props, .. }) => {
                props.bit_order
            }
            Ok(IrNode::Choice { props, .. }) => props.bit_order,
            Err(_) => BitOrder::MostSignificantBitFirst,
        }
    }
}
