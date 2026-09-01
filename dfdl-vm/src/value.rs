use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Decoded logical data tree produced by the VM.
#[derive(Debug, Clone, PartialEq)]
pub enum DfdlValue {
    Null,
    Boolean(bool),
    Int(i32),
    Long(i64),
    Short(i16),
    Byte(i8),
    UnsignedInt(u32),
    UnsignedShort(u16),
    UnsignedByte(u8),
    Float(f32),
    Double(f64),
    String(String),
    HexBinary(Vec<u8>),
    /// Repeated occurrences of an element or group.
    Array(Vec<DfdlValue>),
    Sequence(BTreeMap<String, DfdlValue>),
    Choice {
        /// Name of the selected element in the choice group.
        discriminator: String,
        value: Box<DfdlValue>,
    },
}

impl DfdlValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            DfdlValue::Boolean(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            DfdlValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            DfdlValue::Long(v) => Some(*v),
            DfdlValue::Int(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            DfdlValue::Double(v) => Some(*v),
            DfdlValue::Float(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            DfdlValue::String(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            DfdlValue::HexBinary(v) => Some(v),
            _ => None,
        }
    }

    pub fn field(&self, name: &str) -> Option<&DfdlValue> {
        match self {
            DfdlValue::Choice { discriminator, value } => {
                if discriminator == name {
                    return Some(value.as_ref());
                }
                value.field(name)
            }
            DfdlValue::Sequence(map) => {
                if let Some(v) = map.get(name) {
                    return Some(v);
                }
                // Transparently search through a single root-element wrapper.
                if map.len() == 1 {
                    if let Some(inner) = map.values().next() {
                        return inner.field(name);
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn sequence(fields: BTreeMap<String, DfdlValue>) -> Self {
        DfdlValue::Sequence(fields)
    }

    pub fn choice(name: impl Into<String>, value: DfdlValue) -> Self {
        DfdlValue::Choice {
            discriminator: name.into(),
            value: Box::new(value),
        }
    }
}
