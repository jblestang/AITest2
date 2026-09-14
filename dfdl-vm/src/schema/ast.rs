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
    /// Parsed sibling element name from `{ ../ex:name }` length expressions.
    pub length_sibling: Option<String>,
    /// True when `{ xs:long(../ex:name) }` wraps the length sibling reference.
    pub length_sibling_cast_long: bool,
    /// True when a `{ ... }` length expression was present but not fully compiled.
    pub length_expr_unparsed: bool,
    /// `{ if (fn:string-length(.) gt N) then N else fn:string-length(.) }` style cap.
    pub length_self_string_max_cap: Option<u64>,
    /// `dfdl:valueLength(., 'bytes')` self-referential length (compile-time error).
    pub length_self_value_length: bool,
    pub length_units: Option<LengthUnits>,
    pub encoding: Option<String>,
    pub encoding_error_policy: Option<EncodingErrorPolicy>,
    /// Set when `encodingErrorPolicy` appears on a DFDL format in this schema document.
    pub encoding_error_policy_defined: bool,
    pub text_trim_kind: Option<TextTrimKind>,
    pub text_pad_kind: Option<TextPadKind>,
    /// When false, explicit-length fields may leave unconsumed data in their frame.
    pub truncate_specified_length_string: Option<bool>,
    /// Expanded pad character for numeric text (`dfdl:textNumberPadCharacter`).
    pub text_number_pad_character: Option<String>,
    /// Expanded pad character for string text (`dfdl:textStringPadCharacter`).
    pub text_string_pad_character: Option<String>,
    /// Set when pad character came from `dfdl:property` (not an XSD attribute).
    pub text_string_pad_character_property_form: bool,
    pub text_calendar_pad_character: Option<String>,
    pub text_calendar_justification: Option<TextStringJustification>,
    pub binary_number_rep: Option<BinaryNumberRep>,
    pub binary_packed_sign_codes: Option<String>,
    pub binary_packed_sign_codes_defined: bool,
    pub binary_number_check_policy: Option<BinaryNumberCheckPolicy>,
    pub binary_calendar_rep: Option<BinaryNumberRep>,
    pub binary_calendar_epoch: Option<String>,
    pub binary_float_rep: Option<BinaryFloatRep>,
    pub binary_decimal_virtual_point: Option<u32>,
    /// Negative or invalid `binaryDecimalVirtualPoint` for compile-time SDE.
    pub binary_decimal_virtual_point_sde: Option<i32>,
    pub decimal_signed: Option<bool>,
    pub calendar_pattern: Option<String>,
    pub calendar_pattern_kind: Option<CalendarPatternKind>,
    pub calendar_check_policy_lax: Option<bool>,
    pub calendar_century_start: Option<u32>,
    pub calendar_language: Option<String>,
    pub calendar_days_in_first_week: Option<u32>,
    pub calendar_first_day_of_week: Option<String>,
    pub calendar_time_zone: Option<String>,
    pub calendar_time_zone_defined: bool,
    pub text_number_pattern: Option<String>,
    pub text_number_check_policy: Option<BinaryNumberCheckPolicy>,
    pub text_number_rep: Option<TextNumberRep>,
    pub text_zoned_sign_style: Option<TextZonedSignStyle>,
    pub text_standard_decimal_separator: Option<String>,
    /// Sibling local name when decimal separator is `{ ../ex:name }`.
    pub text_standard_decimal_separator_sibling: Option<String>,
    pub text_standard_grouping_separator: Option<String>,
    pub text_standard_grouping_separator_sibling: Option<String>,
    pub text_standard_exponent_rep: Option<String>,
    pub text_standard_exponent_rep_sibling: Option<String>,
    pub text_standard_infinity_rep: Option<String>,
    pub text_standard_nan_rep: Option<String>,
    pub text_standard_zero_rep: Option<String>,
    pub text_number_rounding: Option<TextNumberRounding>,
    pub text_number_rounding_increment: Option<String>,
    pub text_number_rounding_mode: Option<TextNumberRoundingMode>,
    pub initiator: Option<String>,
    pub terminator: Option<String>,
    pub separator: Option<String>,
    pub output_new_line: Option<String>,
    pub occurs_min: Option<u64>,
    pub occurs_max: Option<u64>,
    /// True when `maxOccurs` was present in XSD (distinguishes unset vs unbounded).
    pub max_occurs_specified: bool,
    pub choice_dispatch_key: Option<String>,
    pub length_pattern: Option<String>,
    pub separator_position: Option<SeparatorPosition>,
    pub text_boolean_true_rep: Option<String>,
    pub text_boolean_false_rep: Option<String>,
    pub text_boolean_true_rep_defined: bool,
    pub text_boolean_false_rep_defined: bool,
    pub text_boolean_pad_character: Option<String>,
    pub binary_boolean_true_rep: Option<u64>,
    /// `binaryBooleanTrueRep` was present (`""` allowed).
    pub binary_boolean_true_rep_defined: bool,
    pub binary_boolean_false_rep: Option<u64>,
    pub binary_boolean_false_rep_defined: bool,
    pub default_value: Option<String>,
    pub alignment: Option<u64>,
    pub alignment_implicit: Option<bool>,
    pub alignment_units: Option<LengthUnits>,
    pub leading_skip: Option<u64>,
    pub trailing_skip: Option<u64>,
    pub sequence_kind: Option<SequenceKind>,
    pub fill_byte: Option<Vec<u8>>,
    /// Raw `dfdl:fillByte` attribute before entity expansion (compile-time checks).
    pub fill_byte_raw: Option<String>,
    /// Named format reference from `dfdl:ref` (resolved during parse).
    pub format_ref: Option<String>,
    /// Type name for prefixed length fields (`dfdl:prefixLengthType`).
    pub prefix_length_type: Option<TypeName>,
    pub prefix_includes_prefix_length: Option<bool>,
    pub input_value_calc: Option<InputValueCalc>,
    /// Literal from `{ xs:string('...') }` in inputValueCalc.
    pub input_value_calc_literal: Option<String>,
    /// Local name of sibling referenced by `../name` in inputValueCalc.
    pub input_value_calc_sibling: Option<String>,
    pub input_value_calc_segments: Option<alloc::vec::Vec<InputValueCalcSegment>>,
    pub output_value_calc: Option<OutputValueCalc>,
    /// Local name of sibling referenced by `../name` in outputValueCalc.
    pub output_value_calc_sibling: Option<String>,
    /// XPath `if`/fn: expression on outputValueCalc (cycle detection for TDML negative tests).
    pub output_value_calc_conditional: bool,
    pub text_string_justification: Option<TextStringJustification>,
    pub text_number_justification: Option<TextNumberJustification>,
    pub text_standard_base: Option<u32>,
    pub nillable: Option<bool>,
    pub nil_kind: Option<NilKind>,
    /// Expanded nil literal (e.g. `%ES;` → empty string, or `nil`).
    pub nil_value: Option<String>,
    pub separator_suppression_policy: Option<SeparatorSuppressionPolicy>,
    pub occurs_count_kind: Option<OccursCountKind>,
    /// `dfdl:hiddenGroupRef` on a sequence (inline hidden model group).
    pub hidden_group_ref: Option<String>,
    /// When true, initiator/terminator/separator matching ignores ASCII case.
    pub ignore_case: Option<bool>,
    pub initiated_content: Option<bool>,
    /// True when a DFDL statement annotation (e.g. `dfdl:assert`) appears on this construct.
    pub has_statement_annotation: bool,
    /// `dfdl:assert/@message` when present (facet tests use checkConstraints).
    pub assert_message: Option<alloc::string::String>,
    /// True when `dfdl:assert/@test` references `dfdl:checkConstraints`.
    pub facet_check_constraints: bool,
    /// `{ xs:int(.) eq N }` on `dfdl:assert` (section 02 assert tests).
    pub assert_int_eq: Option<i64>,
    /// Daffodil extension `dfdlx:objectKind` (`bytes` / `chars`).
    pub object_kind: Option<ObjectKind>,
    /// `dfdlx:parseUnparsePolicy` (`both` / `parseOnly` / `unparseOnly`).
    pub parse_unparse_policy: Option<ParseUnparsePolicy>,
    /// `{ ../ex:a/b }` style inputValueCalc (path after `../`).
    pub input_value_calc_path: Option<alloc::vec::Vec<(Option<alloc::string::String>, alloc::string::String)>>,
    pub text_bidi: Option<bool>,
    pub floating: Option<bool>,
    /// `dfdl:escapeSchemeRef` (empty string clears inherited scheme).
    pub escape_scheme_ref: Option<String>,
    /// `daf:suppressSchemaDefinitionWarnings` on this construct.
    pub suppress_schema_definition_warnings: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EscapeSchemeDef {
    pub escape_kind: EscapeKind,
    pub escape_character: Option<String>,
    pub escape_escape_character: Option<String>,
    pub escape_block_start: Option<String>,
    pub escape_block_end: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EscapeKind {
    #[default]
    EscapeCharacter,
    EscapeBlock,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjectKind {
    #[default]
    Normal,
    Bytes,
    Chars,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CalendarPatternKind {
    #[default]
    Implicit,
    Explicit,
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
    Characters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodingErrorPolicy {
    #[default]
    Error,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NilKind {
    LiteralValue,
    LiteralCharacter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeparatorSuppressionPolicy {
    AnyEmpty,
    TrailingEmpty,
    /// Daffodil extension: trailing empty optional elements are not allowed.
    TrailingEmptyStrict,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OccursCountKind {
    #[default]
    Parsed,
    Implicit,
    Fixed,
    Expression,
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
pub enum TextPadKind {
    None,
    PadChar,
}

/// One segment of `{ fn:concat(...) }` in `dfdl:inputValueCalc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputValueCalcSegment {
    Sibling(String),
    Substring {
        sibling: String,
        start: usize,
        length: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseUnparsePolicy {
    Both,
    ParseOnly,
    UnparseOnly,
}

/// Narrow support for `dfdl:inputValueCalc` used in Daffodil prefixed length tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputValueCalc {
    Constant(i64),
    ContentLengthSelf(LengthUnits),
    ValueLengthSelf(LengthUnits),
    ContentLengthSibling(LengthUnits),
    ValueLengthSibling(LengthUnits),
    BooleanFromSibling,
    /// `{ xs:string('...') }` — literal lexical value for calendar/text tests.
    StringLiteral,
}

/// Narrow support for `dfdl:outputValueCalc` used on encode/unparse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputValueCalc {
    Constant(i64),
    ContentLengthSelf(LengthUnits, i64),
    ValueLengthSelf(LengthUnits, i64),
    ContentLengthSibling(LengthUnits, i64),
    ValueLengthSibling(LengthUnits, i64),
    StringLengthSibling,
    /// `fn:substring(../sibling, start, length)` — 1-based XPath start index.
    Substring { start: usize, length: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextStringJustification {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextNumberJustification {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryNumberRep {
    Binary,
    Bcd,
    PackedBcd,
    Ibm4690Packed,
    BinarySeconds,
    BinaryMilliseconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryNumberCheckPolicy {
    Strict,
    Lax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextNumberRep {
    #[default]
    Standard,
    Zoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextNumberRounding {
    #[default]
    Pattern,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextNumberRoundingMode {
    RoundCeiling,
    RoundFloor,
    RoundDown,
    RoundUp,
    #[default]
    RoundHalfEven,
    RoundHalfDown,
    RoundHalfUp,
    RoundUnnecessary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextZonedSignStyle {
    AsciiStandard,
    AsciiTranslatedEBCDIC,
    AsciiCARealiaModified,
    AsciiTandemModified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryFloatRep {
    Ieee,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupRefDecl {
    pub name: String,
    pub props: DfdlProps,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Particle {
    Element(ElementDecl),
    Sequence(SequenceDecl),
    Choice(ChoiceDecl),
    GroupRef(GroupRefDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElementDecl {
    pub name: String,
    /// Local name from `ref="..."` when present.
    pub element_ref: Option<String>,
    /// True when the XSD `name="..."` attribute was present (not only `ref`).
    pub has_element_name_attr: bool,
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
pub enum RestrictionBase {
    Builtin(BuiltinType),
    Named(TypeName),
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnionMember {
    /// `memberTypes="ex:foo ex:bar"`.
    Named(TypeName),
    /// Inline `<xs:simpleType>` under `<xs:union>`.
    Inline(SimpleBase),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SimpleBase {
    Builtin(BuiltinType),
    Union {
        members: alloc::vec::Vec<UnionMember>,
    },
    Restriction {
        base: RestrictionBase,
        length: Option<u64>,
        min_length: Option<u64>,
        max_length: Option<u64>,
        min_inclusive: Option<i64>,
        max_inclusive: Option<i64>,
        min_exclusive: Option<i64>,
        max_exclusive: Option<i64>,
        min_inclusive_lexical: Option<alloc::string::String>,
        max_inclusive_lexical: Option<alloc::string::String>,
        min_exclusive_lexical: Option<alloc::string::String>,
        max_exclusive_lexical: Option<alloc::string::String>,
        /// OR'd patterns within this restriction level.
        patterns: alloc::vec::Vec<alloc::string::String>,
        enumerations: alloc::vec::Vec<alloc::string::String>,
        total_digits: Option<u64>,
        fraction_digits: Option<u64>,
        /// Raw facet value when not a valid non-negative integer (compile SDE).
        invalid_min_length: Option<alloc::string::String>,
        invalid_max_length: Option<alloc::string::String>,
        invalid_length: Option<alloc::string::String>,
        invalid_total_digits: Option<alloc::string::String>,
        invalid_fraction_digits: Option<alloc::string::String>,
    },
}

impl SchemaDocument {
    /// Resolve a simple type base to its builtin XSD type (follows `restriction base="ex:…"` chains).
    pub fn builtin_for_simple_base(&self, base: &SimpleBase) -> Option<BuiltinType> {
        match base {
            SimpleBase::Builtin(b) => Some(*b),
            SimpleBase::Union { members } => members.iter().find_map(|member| match member {
                UnionMember::Inline(inner) => self.builtin_for_simple_base(inner),
                UnionMember::Named(name) => self.types.get(name).and_then(|def| {
                    if let TypeDef::Simple { base: inner, .. } = def {
                        self.builtin_for_simple_base(inner)
                    } else {
                        None
                    }
                }),
            }),
            SimpleBase::Restriction { base, .. } => match base {
                RestrictionBase::Builtin(b) => Some(*b),
                RestrictionBase::Named(name) => self
                    .types
                    .get(name)
                    .and_then(|def| {
                        if let TypeDef::Simple { base: inner, .. } = def {
                            self.builtin_for_simple_base(inner)
                        } else {
                            None
                        }
                    }),
            },
        }
    }
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
    Integer,
    NonNegativeInteger,
    Float,
    Double,
    Decimal,
    DateTime,
    Time,
    Boolean,
    HexBinary,
}

impl BuiltinType {
    pub fn from_xsd(name: &str) -> Option<Self> {
        match name {
            "xs:integer" | "integer" => Some(BuiltinType::Integer),
            "xs:string" | "string" => Some(BuiltinType::String),
            "xs:int" | "int" => Some(BuiltinType::Int),
            "xs:long" | "long" => Some(BuiltinType::Long),
            "xs:short" | "short" => Some(BuiltinType::Short),
            "xs:byte" | "byte" => Some(BuiltinType::Byte),
            "xs:unsignedInt" | "unsignedInt" => Some(BuiltinType::UnsignedInt),
            "xs:unsignedLong" | "unsignedLong" => Some(BuiltinType::Long),
            "xs:nonNegativeInteger" | "nonNegativeInteger" => Some(BuiltinType::NonNegativeInteger),
            "xs:unsignedShort" | "unsignedShort" => Some(BuiltinType::UnsignedShort),
            "xs:unsignedByte" | "unsignedByte" => Some(BuiltinType::UnsignedByte),
            "xs:float" | "float" => Some(BuiltinType::Float),
            "xs:double" | "double" => Some(BuiltinType::Double),
            "xs:decimal" | "decimal" => Some(BuiltinType::Decimal),
            "xs:date" | "date" => Some(BuiltinType::DateTime),
            "xs:dateTime" | "dateTime" => Some(BuiltinType::DateTime),
            "xs:time" | "time" => Some(BuiltinType::Time),
            "xs:boolean" | "boolean" => Some(BuiltinType::Boolean),
            "xs:hexBinary" | "hexBinary" => Some(BuiltinType::HexBinary),
            "xs:anyURI" | "anyURI" => Some(BuiltinType::String),
            _ => None,
        }
    }

    pub fn xsd_name(self) -> &'static str {
        match self {
            BuiltinType::String => "xs:string",
            BuiltinType::Int => "xs:int",
            BuiltinType::Integer => "xs:integer",
            BuiltinType::Long => "xs:long",
            BuiltinType::Short => "xs:short",
            BuiltinType::Byte => "xs:byte",
            BuiltinType::UnsignedInt => "xs:unsignedInt",
            BuiltinType::NonNegativeInteger => "xs:nonNegativeInteger",
            BuiltinType::UnsignedShort => "xs:unsignedShort",
            BuiltinType::UnsignedByte => "xs:unsignedByte",
            BuiltinType::Float => "xs:float",
            BuiltinType::Double => "xs:double",
            BuiltinType::Decimal => "xs:decimal",
            BuiltinType::DateTime => "xs:dateTime",
            BuiltinType::Time => "xs:time",
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
    /// True when the XSD `type="..."` attribute used a prefixed QName (e.g. `ex:itemType`).
    pub type_qname_prefixed: bool,
    pub props: DfdlProps,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormatDefaults {
    pub props: DfdlProps,
}

/// Parsed XSD + DFDL schema document.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SchemaDocument {
    /// True when the schema declares or uses the DFDL namespace (elements or xmlns).
    pub dfdl_annotations_seen: bool,
    /// Set when a top-level `dfdl:format` explicitly declares `encodingErrorPolicy`.
    pub explicit_encoding_error_policy_on_format: bool,
    pub target_namespace: Option<String>,
    /// `xs:schema/@elementFormDefault` (default unqualified).
    pub element_form_default_qualified: bool,
    /// True when `elementFormDefault` was present on `xs:schema`.
    pub element_form_default_explicit: bool,
    pub format_defaults: FormatDefaults,
    /// Named DFDL formats from `dfdl:defineFormat`.
    pub named_formats: BTreeMap<String, DfdlProps>,
    /// Named escape schemes from `dfdl:defineEscapeScheme`.
    pub named_escape_schemes: BTreeMap<String, EscapeSchemeDef>,
    pub types: BTreeMap<TypeName, TypeDef>,
    pub global_elements: BTreeMap<String, GlobalElement>,
    /// Named `xs:group` model groups (local name → sequence or choice).
    pub groups: BTreeMap<String, GroupDecl>,
    /// Non-fatal schema issues collected during parse (TDML multi-diagnostic tests).
    pub schema_diagnostics: alloc::vec::Vec<String>,
    /// Schema definition warnings not tied to a global element.
    pub schema_warnings: alloc::vec::Vec<String>,
    /// Warnings keyed by global element name (TDML escalate/suppress).
    pub scoped_schema_warnings: BTreeMap<String, alloc::vec::Vec<String>>,
    /// Original schema file label when parsed from an external path (TDML `model="*.xsd"`).
    pub schema_source_label: Option<String>,
    /// Raw schema text (for line/column in compile-time SDEs).
    pub schema_source_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroupDecl {
    Sequence(SequenceDecl),
    Choice(ChoiceDecl),
}

impl GroupDecl {
    pub fn props(&self) -> &DfdlProps {
        match self {
            GroupDecl::Sequence(s) => &s.props,
            GroupDecl::Choice(c) => &c.props,
        }
    }
}

impl SchemaDocument {
    pub fn root_element(&self) -> Option<&GlobalElement> {
        self.global_elements.values().next()
    }

    pub fn resolve_type(&self, name: &TypeName) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Merge DFDL properties along `restriction base="ex:…"` simple type chains.
    pub fn effective_simple_type_props(&self, name: &TypeName) -> Option<DfdlProps> {
        let TypeDef::Simple { base, props, .. } = self.types.get(name)? else {
            return None;
        };
        let mut out = match base {
            SimpleBase::Restriction {
                base: RestrictionBase::Named(parent),
                ..
            } => self
                .effective_simple_type_props(parent)
                .unwrap_or_default(),
            SimpleBase::Restriction {
                base: RestrictionBase::Builtin(_),
                ..
            }
            | SimpleBase::Union { .. }
            | SimpleBase::Builtin(_) => DfdlProps::default(),
        };
        out = crate::schema::parser::merge_dfdl_props(out, props.clone());
        Some(out)
    }
}
