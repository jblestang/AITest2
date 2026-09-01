use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Logical XSD type name (e.g. `xs:int`, `MyRecord`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeName(pub String);

impl TypeName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Parsed DFDL representation properties attached to a schema construct.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DfdlProps {
    pub representation: Option<Representation>,
    pub byte_order: Option<ByteOrder>,
    pub bit_order: Option<BitOrder>,
    pub length_kind: Option<LengthKind>,
    pub length: Option<u64>,
    pub length_units: Option<LengthUnits>,
    pub encoding: Option<String>,
    pub text_trim_kind: Option<TextTrimKind>,
    pub binary_number_rep: Option<BinaryNumberRep>,
    pub binary_float_rep: Option<BinaryFloatRep>,
    pub initiator: Option<String>,
    pub terminator: Option<String>,
    pub separator: Option<String>,
    pub occurs_min: Option<u64>,
    pub occurs_max: Option<u64>,
    /// True when `maxOccurs` was present in XSD (distinguishes unset vs unbounded).
    pub max_occurs_specified: bool,
    pub choice_dispatch_key: Option<String>,
    pub length_pattern: Option<String>,
    pub separator_position: Option<SeparatorPosition>,
    pub text_boolean_true_rep: Option<String>,
    pub text_boolean_false_rep: Option<String>,
    pub default_value: Option<String>,
    pub alignment: Option<u64>,
    pub leading_skip: Option<u64>,
    pub trailing_skip: Option<u64>,
    pub sequence_kind: Option<SequenceKind>,
    pub fill_byte: Option<Vec<u8>>,
    /// Named format reference from `dfdl:ref` (resolved during parse).
    pub format_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Representation {
    Binary,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    BigEndian,
    LittleEndian,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitOrder {
    MostSignificantBitFirst,
    LeastSignificantBitFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthKind {
    Implicit,
    Explicit,
    Fixed,
    Delimited,
    Prefixed,
    Pattern,
    EndOfParent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeparatorPosition {
    Infix,
    Prefix,
    Postfix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceKind {
    Ordered,
    Unordered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthUnits {
    Bytes,
    Bits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTrimKind {
    None,
    Trim,
    Left,
    Right,
    /// Trim pad characters (typically `%SP;`) from both ends.
    PadChar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryNumberRep {
    Binary,
    Bcd,
    PackedBcd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryFloatRep {
    Ieee,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Particle {
    Element(ElementDecl),
    Sequence(SequenceDecl),
    Choice(ChoiceDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElementDecl {
    pub name: String,
    pub type_name: TypeName,
    pub props: DfdlProps,
    pub particle: Option<Box<Particle>>,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SequenceDecl {
    pub props: DfdlProps,
    pub particles: Vec<Particle>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceDecl {
    pub props: DfdlProps,
    pub branches: Vec<Particle>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComplexContent {
    Sequence(SequenceDecl),
    Choice(ChoiceDecl),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SimpleBase {
    Builtin(BuiltinType),
    Restriction { base: BuiltinType, max_length: Option<u64> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinType {
    String,
    Int,
    Long,
    Short,
    Byte,
    UnsignedInt,
    UnsignedShort,
    UnsignedByte,
    Float,
    Double,
    Boolean,
    HexBinary,
}

impl BuiltinType {
    pub fn from_xsd(name: &str) -> Option<Self> {
        match name {
            "xs:string" | "string" => Some(BuiltinType::String),
            "xs:int" | "int" => Some(BuiltinType::Int),
            "xs:long" | "long" => Some(BuiltinType::Long),
            "xs:short" | "short" => Some(BuiltinType::Short),
            "xs:byte" | "byte" => Some(BuiltinType::Byte),
            "xs:unsignedInt" | "unsignedInt" => Some(BuiltinType::UnsignedInt),
            "xs:unsignedShort" | "unsignedShort" => Some(BuiltinType::UnsignedShort),
            "xs:unsignedByte" | "unsignedByte" => Some(BuiltinType::UnsignedByte),
            "xs:float" | "float" => Some(BuiltinType::Float),
            "xs:double" | "double" => Some(BuiltinType::Double),
            "xs:boolean" | "boolean" => Some(BuiltinType::Boolean),
            "xs:hexBinary" | "hexBinary" => Some(BuiltinType::HexBinary),
            _ => None,
        }
    }

    pub fn xsd_name(self) -> &'static str {
        match self {
            BuiltinType::String => "xs:string",
            BuiltinType::Int => "xs:int",
            BuiltinType::Long => "xs:long",
            BuiltinType::Short => "xs:short",
            BuiltinType::Byte => "xs:byte",
            BuiltinType::UnsignedInt => "xs:unsignedInt",
            BuiltinType::UnsignedShort => "xs:unsignedShort",
            BuiltinType::UnsignedByte => "xs:unsignedByte",
            BuiltinType::Float => "xs:float",
            BuiltinType::Double => "xs:double",
            BuiltinType::Boolean => "xs:boolean",
            BuiltinType::HexBinary => "xs:hexBinary",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDef {
    Simple {
        name: TypeName,
        base: SimpleBase,
        props: DfdlProps,
    },
    Complex {
        name: TypeName,
        content: ComplexContent,
        props: DfdlProps,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalElement {
    pub name: String,
    pub type_name: TypeName,
    pub props: DfdlProps,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormatDefaults {
    pub props: DfdlProps,
}

/// Parsed XSD + DFDL schema document.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SchemaDocument {
    pub target_namespace: Option<String>,
    pub format_defaults: FormatDefaults,
    /// Named DFDL formats from `dfdl:defineFormat`.
    pub named_formats: BTreeMap<String, DfdlProps>,
    pub types: BTreeMap<TypeName, TypeDef>,
    pub global_elements: BTreeMap<String, GlobalElement>,
}

impl SchemaDocument {
    pub fn root_element(&self) -> Option<&GlobalElement> {
        self.global_elements.values().next()
    }

    pub fn resolve_type(&self, name: &TypeName) -> Option<&TypeDef> {
        self.types.get(name)
    }
}
