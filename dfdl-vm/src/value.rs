use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Optional metadata captured during parse to guide faithful unparse.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SequenceMeta {
    /// For each infix separator slot (before child index 1..n-1), whether a newline
    /// prefix was consumed for `%NL;, ,`-style separator patterns.
    pub infix_sep_newline_prefix: Vec<bool>,
}

/// Named fields in a DFDL sequence with optional parse metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceValue {
    pub fields: BTreeMap<String, DfdlValue>,
    pub meta: SequenceMeta,
}

impl SequenceValue {
    pub fn new(fields: BTreeMap<String, DfdlValue>) -> Self {
        Self {
            fields,
            meta: SequenceMeta::default(),
        }
    }
}

/// Optional metadata captured during parse to guide faithful unparse.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StringMeta {
    /// Original encoded bytes when `encodingErrorPolicy=replace` consumed malformed data.
    pub source_bytes: Option<Vec<u8>>,
}

/// Decoded string with optional parse metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringValue {
    pub text: String,
    pub meta: StringMeta,
}

impl StringValue {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            meta: StringMeta::default(),
        }
    }

    pub fn with_source_bytes(text: impl Into<String>, source_bytes: Vec<u8>) -> Self {
        Self {
            text: text.into(),
            meta: StringMeta {
                source_bytes: Some(source_bytes),
            },
        }
    }
}

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
    /// Decimal value as canonical string (e.g. `123.45`).
    Decimal(String),
    /// ISO-like dateTime string (e.g. `2004-06-14T18:56:03`).
    DateTime(String),
    String(StringValue),
    HexBinary(Vec<u8>),
    /// Repeated occurrences of an element or group.
    Array(Vec<DfdlValue>),
    Sequence(SequenceValue),
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
            DfdlValue::String(v) => Some(&v.text),
            _ => None,
        }
    }

    pub fn string(text: impl Into<String>) -> Self {
        DfdlValue::String(StringValue::new(text))
    }

    pub fn string_value(&self) -> Option<&StringValue> {
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
            DfdlValue::Sequence(seq) => {
                if let Some(v) = seq.fields.get(name) {
                    return Some(v);
                }
                // Transparently search through a single root-element wrapper.
                if seq.fields.len() == 1 {
                    if let Some(inner) = seq.fields.values().next() {
                        return inner.field(name);
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn sequence(fields: BTreeMap<String, DfdlValue>) -> Self {
        DfdlValue::Sequence(SequenceValue::new(fields))
    }

    pub fn sequence_fields(&self) -> Option<&BTreeMap<String, DfdlValue>> {
        match self {
            DfdlValue::Sequence(seq) => Some(&seq.fields),
            _ => None,
        }
    }

    pub fn sequence_value(&self) -> Option<&SequenceValue> {
        match self {
            DfdlValue::Sequence(seq) => Some(seq),
            _ => None,
        }
    }

    pub fn sequence_value_mut(&mut self) -> Option<&mut SequenceValue> {
        match self {
            DfdlValue::Sequence(seq) => Some(seq),
            _ => None,
        }
    }

    pub fn choice(name: impl Into<String>, value: DfdlValue) -> Self {
        DfdlValue::Choice {
            discriminator: name.into(),
            value: Box::new(value),
        }
    }
}
