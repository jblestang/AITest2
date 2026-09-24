use crate::ir::StringId;
use crate::schema::{BitOrder, ByteOrder, EncodingErrorPolicy, ObjectKind, Representation};

#[derive(Debug, Clone, PartialEq)]
pub struct RepresentationProps {
    pub representation: Representation,
    pub representation_defined: bool,
    pub byte_order: ByteOrder,
    pub byte_order_defined: bool,
    pub byte_order_conditional_test: Option<StringId>,
    pub byte_order_if_true: ByteOrder,
    pub byte_order_if_false: ByteOrder,
    pub bit_order: BitOrder,
    pub bit_order_defined: bool,
    pub encoding: StringId,
    pub encoding_error_policy: EncodingErrorPolicy,
    pub object_kind: ObjectKind,
}

impl Default for RepresentationProps {
    fn default() -> Self {
        Self {
            representation: Representation::Binary,
            representation_defined: false,
            byte_order: ByteOrder::BigEndian,
            byte_order_defined: false,
            byte_order_conditional_test: None,
            byte_order_if_true: ByteOrder::BigEndian,
            byte_order_if_false: ByteOrder::LittleEndian,
            bit_order: BitOrder::MostSignificantBitFirst,
            bit_order_defined: false,
            encoding: StringId(0),
            encoding_error_policy: EncodingErrorPolicy::Replace,
            object_kind: ObjectKind::Bytes,
        }
    }
}
