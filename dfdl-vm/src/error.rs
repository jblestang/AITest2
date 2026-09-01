use core::fmt;

/// Errors produced while parsing XSD/DFDL, building IR, or running the VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Parse(ParseError),
    Schema(SchemaError),
    Vm(VmError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => write!(f, "parse error: {e}"),
            Error::Schema(e) => write!(f, "schema error: {e}"),
            Error::Vm(e) => write!(f, "vm error: {e}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    InvalidXml { message: alloc::string::String },
    UnexpectedEof,
    MissingAttribute { element: alloc::string::String, attribute: alloc::string::String },
    UnknownElement { name: alloc::string::String },
    UnknownType { name: alloc::string::String },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::InvalidXml { message } => write!(f, "{message}"),
            ParseError::UnexpectedEof => write!(f, "unexpected end of input"),
            ParseError::MissingAttribute { element, attribute } => {
                write!(f, "element `{element}` missing attribute `{attribute}`")
            }
            ParseError::UnknownElement { name } => write!(f, "unknown element `{name}`"),
            ParseError::UnknownType { name } => write!(f, "unknown type `{name}`"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    NoRootElement,
    AmbiguousRootElement,
    UndefinedType { name: alloc::string::String },
    UnsupportedFeature { feature: alloc::string::String },
    InvalidProperty { message: alloc::string::String },
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::NoRootElement => write!(f, "schema has no global element"),
            SchemaError::AmbiguousRootElement => write!(f, "schema has multiple global elements"),
            SchemaError::UndefinedType { name } => write!(f, "undefined type `{name}`"),
            SchemaError::UnsupportedFeature { feature } => write!(f, "unsupported: {feature}"),
            SchemaError::InvalidProperty { message } => write!(f, "{message}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmError {
    UnexpectedEof,
    TrailingData { remaining: usize },
    InvalidChoice,
    LengthMismatch { expected: usize, actual: usize },
    InvalidValue { message: alloc::string::String },
    TypeMismatch { expected: alloc::string::String },
    MissingField { name: alloc::string::String },
    UnsupportedOperation { op: alloc::string::String },
    /// Optional element (minOccurs=0) absent at current offset.
    ElementAbsent,
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VmError::UnexpectedEof => write!(f, "unexpected end of input"),
            VmError::TrailingData { remaining } => {
                write!(f, "{remaining} trailing byte(s) after decode")
            }
            VmError::InvalidChoice => write!(f, "no choice branch matched"),
            VmError::LengthMismatch { expected, actual } => {
                write!(f, "length mismatch: expected {expected}, got {actual}")
            }
            VmError::InvalidValue { message } => write!(f, "{message}"),
            VmError::TypeMismatch { expected } => write!(f, "expected value of type {expected}"),
            VmError::MissingField { name } => write!(f, "missing required field `{name}`"),
            VmError::UnsupportedOperation { op } => write!(f, "unsupported VM operation `{op}`"),
            VmError::ElementAbsent => write!(f, "element absent"),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

impl From<ParseError> for Error {
    fn from(value: ParseError) -> Self {
        Error::Parse(value)
    }
}

impl From<SchemaError> for Error {
    fn from(value: SchemaError) -> Self {
        Error::Schema(value)
    }
}

impl From<VmError> for Error {
    fn from(value: VmError) -> Self {
        Error::Vm(value)
    }
}
