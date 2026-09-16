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
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DfdlProps {
    pub representation: Option<Representation>,
    pub byte_order: Option<ByteOrder>,
    /// `if (../../ex:a eq 'b') then 'bigEndian' else 'littleEndian'` condition text.
    pub byte_order_conditional_test: Option<String>,
    pub byte_order_if_true: Option<ByteOrder>,
    pub byte_order_if_false: Option<ByteOrder>,
    pub bit_order: Option<BitOrder>,
    pub length_kind: Option<LengthKind>,
    /// True when `dfdl:lengthKind` was set on this construct or merged format ref.
    pub length_kind_defined: bool,
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
    /// Raw lexical value for `dfdl:textNumberPadCharacter` (expanded when building IR).
    pub text_number_pad_character: Option<String>,
    pub text_number_pad_character_property_form: bool,
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
    pub calendar_language_segments: Option<alloc::vec::Vec<InputValueCalcSegment>>,
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
    /// Initiator XSD value contained `%%` (literal percent, not entity syntax).
    pub initiator_percent_escaped: bool,
    pub terminator: Option<String>,
    pub separator: Option<String>,
    pub output_new_line: Option<String>,
    pub occurs_min: Option<u64>,
    pub occurs_max: Option<u64>,
    /// True when `maxOccurs` was present in XSD (distinguishes unset vs unbounded).
    pub max_occurs_specified: bool,
    pub choice_dispatch_key: Option<String>,
    /// Parsed sibling from `{ xs:string(./name) }` or `{ xs:string(../name) }` in choiceDispatchKey.
    pub choice_dispatch_sibling: Option<String>,
    /// Parsed `{ ../a/b }` path in choiceDispatchKey (no xs:string wrapper).
    pub choice_dispatch_path: Option<
        alloc::vec::Vec<(
            Option<alloc::string::String>,
            alloc::string::String,
            Option<u32>,
        )>,
    >,
    /// Literal dispatch key from `{ xs:string('…') }` in choiceDispatchKey.
    pub choice_dispatch_literal: Option<String>,
    /// `{ xs:string(xs:int(sibling)) }` — parse prior sibling text as xs:int for dispatch.
    pub choice_dispatch_sibling_int: Option<String>,
    /// Branch discriminator for choice dispatch (`dfdl:choiceBranchKey`).
    pub choice_branch_key: Option<String>,
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
    pub choice_length_kind: Option<ChoiceLengthKind>,
    pub choice_length: Option<u64>,
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
    /// Hex literal from `{ xs:hexBinary('...') }` / `{ dfdl:hexBinary('...') }` outputValueCalc.
    pub output_value_calc_literal: Option<String>,
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
    pub empty_element_parse_policy: Option<EmptyElementParsePolicy>,
    pub occurs_count_kind: Option<OccursCountKind>,
    /// Steps after `fn:count(` / parent `../` segments for `{ fn:count(../../a/b) }`.
    pub occurs_count_fn_path: Option<
        alloc::vec::Vec<(
            Option<alloc::string::String>,
            alloc::string::String,
            Option<u32>,
        )>,
    >,
    /// `dfdl:hiddenGroupRef` on a sequence (inline hidden model group).
    pub hidden_group_ref: Option<String>,
    /// Set when `hiddenGroupRef` came from appinfo `dfdl:sequence` (attribute or property form).
    pub hidden_group_ref_from_appinfo_sequence: bool,
    /// When true, initiator/terminator/separator matching ignores ASCII case.
    pub ignore_case: Option<bool>,
    pub initiated_content: Option<bool>,
    /// True when a DFDL statement annotation (e.g. `dfdl:assert`) appears on this construct.
    pub has_statement_annotation: bool,
    /// `dfdl:assert/@message` when present (facet tests use checkConstraints).
    pub assert_message: Option<alloc::string::String>,
    /// Parsed `{ fn:concat(...) }` assert message (evaluated at runtime on failure).
    pub assert_message_segments: Option<alloc::vec::Vec<InputValueCalcSegment>>,
    /// True when `dfdl:assert/@test` references `dfdl:checkConstraints`.
    pub facet_check_constraints: bool,
    /// `{ xs:int(.) eq N }` on `dfdl:assert` (section 02 assert tests).
    pub assert_int_eq: Option<i64>,
    /// `{ xs:int(.) eq dfdl:occursIndex() (+ addend) }` element assert.
    pub assert_eq_occurs_index_addend: Option<i64>,
    /// `dfdl:discriminator` body text for choice branch selection.
    pub discriminator_test: Option<String>,
    /// XPath prefix bindings in scope on this construct's `dfdl:discriminator` only.
    pub discriminator_xpath_prefixes: Option<alloc::collections::BTreeMap<String, String>>,
    /// Daffodil extension `dfdlx:objectKind` (`bytes` / `chars`).
    pub object_kind: Option<ObjectKind>,
    /// `dfdlx:parseUnparsePolicy` (`both` / `parseOnly` / `unparseOnly`).
    pub parse_unparse_policy: Option<ParseUnparsePolicy>,
    /// `{ ../ex:a/b }` style inputValueCalc (path after `../`).
    pub input_value_calc_path: Option<
        alloc::vec::Vec<(
            Option<alloc::string::String>,
            alloc::string::String,
            Option<u32>,
        )>,
    >,
    /// `{ ../ex:a/b + N }` on outputValueCalc.
    pub output_value_calc_path: Option<
        alloc::vec::Vec<(
            Option<alloc::string::String>,
            alloc::string::String,
            Option<u32>,
        )>,
    >,
    pub output_value_calc_path_addend: Option<i64>,
    /// Arithmetic / absolute-path inputValueCalc (e.g. AC000 product expression).
    pub input_value_calc_expression: Option<InputValueCalcExpression>,
    pub text_bidi: Option<bool>,
    pub floating: Option<bool>,
    /// `dfdl:escapeSchemeRef` (empty string clears inherited scheme).
    pub escape_scheme_ref: Option<String>,
    /// `daf:suppressSchemaDefinitionWarnings` on this construct.
    pub suppress_schema_definition_warnings: Option<String>,
    /// `dfdl:setVariable` ref → value pairs from annotations on this construct.
    pub set_variables: alloc::vec::Vec<(alloc::string::String, alloc::string::String)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EscapeSchemeDef {
    pub escape_kind: EscapeKind,
    pub escape_character_raw: Option<String>,
    pub escape_character: Option<String>,
    pub escape_escape_character_raw: Option<String>,
    pub escape_escape_character: Option<String>,
    /// Raw `escapeBlockStart` attribute (before entity expansion), for compile-time SDE checks.
    pub escape_block_start_raw: Option<String>,
    pub escape_block_start: Option<String>,
    /// Raw `escapeBlockEnd` attribute (before entity expansion), for compile-time SDE checks.
    pub escape_block_end_raw: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChoiceLengthKind {
    #[default]
    Implicit,
    Explicit,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmptyElementParsePolicy {
    #[default]
    TreatAsEmpty,
    TreatAsAbsent,
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

/// XSD cast in `{ xs:int(...) }` style inputValueCalc expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvcXsCast {
    Byte,
    Short,
    Int,
    Long,
    UnsignedByte,
    UnsignedShort,
    UnsignedInt,
    UnsignedLong,
    Float,
    Double,
    String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputValueCalcExpression {
    Add(alloc::vec::Vec<InputValueCalcExpression>),
    Mul(alloc::vec::Vec<InputValueCalcExpression>),
    Div(
        alloc::boxed::Box<InputValueCalcExpression>,
        alloc::boxed::Box<InputValueCalcExpression>,
    ),
    Path {
        parent_root: bool,
        steps: alloc::vec::Vec<(
            Option<alloc::string::String>,
            alloc::string::String,
            Option<u32>,
        )>,
    },
    StringOf(alloc::boxed::Box<InputValueCalcExpression>),
    Literal(i64),
    LiteralLexical(alloc::string::String),
    Cast {
        kind: IvcXsCast,
        inner: alloc::boxed::Box<InputValueCalcExpression>,
    },
}

/// One segment of `{ fn:concat(...) }` in `dfdl:inputValueCalc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputValueCalcSegment {
    Sibling(String),
    Literal(String),
    Substring {
        sibling: String,
        start: usize,
        length: usize,
    },
    InfosetPath(alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
    )>),
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
    /// Integer literal that does not fit in i64; lexical in [`DfdlProps::input_value_calc_literal`].
    ConstantLexical,
    ContentLengthSelf(LengthUnits),
    ValueLengthSelf(LengthUnits),
    ContentLengthSibling(LengthUnits),
    ValueLengthSibling(LengthUnits),
    BooleanFromSibling,
    /// `{ xs:hexBinary(../sibling) }` — decode sibling lexical as hexBinary.
    HexBinaryFromSibling,
    /// `{ xs:string('...') }` — literal lexical value for calendar/text tests.
    StringLiteral,
    /// `{ $varName }` — resolved from defineVariable / setVariable at runtime.
    SchemaVariable,
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
    /// `{ xs:hexBinary('...') }` / `{ dfdl:hexBinary('...') }` — literal in [`DfdlProps::output_value_calc_literal`].
    HexBinaryFromLexical,
    /// `{ dfdl:hexBinary(n) }` for integer `n`.
    HexBinaryFromInteger(i64),
    /// `{ dfdl:hexBinary(xs:short(n)) }`.
    HexBinaryFromShort(i16),
    /// `{ dfdl:hexBinary(xs:byte(../sibling)) }`.
    HexBinaryFromByteSibling,
    /// `{ ../path/to/elem + N }` — path in [`DfdlProps::output_value_calc_path`], addend stored separately.
    InfosetPathAddend,
    /// `{ if (dfdl:occursIndex() lt fn:count(..)) then 1 else 0 }` on repeat indicators (GRI/FRI).
    RepeatIndicatorFromParentCount,
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
    pub type_xsd_qname: Option<String>,
    pub type_qname_scope: Option<alloc::collections::BTreeMap<String, String>>,
    pub props: DfdlProps,
    pub particle: Option<Box<Particle>>,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SequenceDecl {
    pub props: DfdlProps,
    pub particles: Vec<Particle>,
    /// True when `xs:annotation` appeared before model-group particles (hiddenGroupRef SDE).
    pub had_markup_before_particles: bool,
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
        /// True when this restriction's `base="xs:date"` (not `xs:dateTime`).
        restriction_base_is_xs_date: bool,
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
    /// True when the simple type chain derives from `xs:date` (not `xs:dateTime`).
    pub fn simple_base_is_xs_date(&self, base: &SimpleBase) -> bool {
        match base {
            SimpleBase::Restriction {
                restriction_base_is_xs_date: true,
                ..
            } => true,
            SimpleBase::Restriction {
                base: RestrictionBase::Named(name),
                ..
            } => self
                .types
                .get(name)
                .and_then(|def| {
                    if let TypeDef::Simple { base: inner, .. } = def {
                        Some(self.simple_base_is_xs_date(inner))
                    } else {
                        None
                    }
                })
                .unwrap_or(false),
            _ => false,
        }
    }

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
        /// Schema-level `dfdl:format` in effect when this type was declared.
        format_context: DfdlProps,
        source_label: Option<String>,
    },
    Complex {
        name: TypeName,
        content: ComplexContent,
        props: DfdlProps,
        format_context: DfdlProps,
        source_label: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalElement {
    pub name: String,
    pub type_name: TypeName,
    /// True when the XSD `type="..."` attribute used a prefixed QName (e.g. `ex:itemType`).
    pub type_qname_prefixed: bool,
    /// Raw `type="..."` attribute when present (e.g. `c03:nestType`).
    pub type_xsd_qname: Option<String>,
    /// Namespace prefix bindings in scope where `type="..."` was written.
    pub type_qname_scope: Option<alloc::collections::BTreeMap<String, String>>,
    pub props: DfdlProps,
    pub format_context: DfdlProps,
    pub source_label: Option<String>,
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
    /// Prefix → URI from `xs:schema` xmlns declarations (format ref resolution).
    pub namespace_prefixes: BTreeMap<String, String>,
    /// `xs:schema/@elementFormDefault` (default unqualified).
    pub element_form_default_qualified: bool,
    /// True when `elementFormDefault` was present on `xs:schema`.
    pub element_form_default_explicit: bool,
    pub format_defaults: FormatDefaults,
    /// Named DFDL formats from `dfdl:defineFormat`.
    pub named_formats: BTreeMap<String, DfdlProps>,
    /// Named escape schemes from `dfdl:defineEscapeScheme`.
    pub named_escape_schemes: BTreeMap<String, EscapeSchemeDef>,
    /// `dfdl:defineVariable` name → default lexical value.
    pub variables: BTreeMap<String, String>,
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
        let format_default_len = self.format_defaults.props.length;
        let overlay_only_default_length = props.length.is_some()
            && props.length == format_default_len
            && props.length_kind.is_none()
            && props.format_ref.is_none();
        let base_length = out.length;
        out = crate::schema::parser::merge_dfdl_props(out, props.clone());
        // Schema-file format default length (injected at parse) must not override restriction-chain
        // length from a format ref in an imported/included schema (long_chain_04 / format_03).
        let restore_chain_length = props.length_kind.is_none()
            && props.format_ref.is_none()
            && base_length.is_some()
            && props.length.is_some()
            && props.length != base_length;
        if overlay_only_default_length || restore_chain_length {
            if let Some(len) = base_length {
                out.length = Some(len);
            }
        }
        if props.length_kind_defined {
            out.length_kind_defined = true;
        }
        Some(out)
    }

    /// Schema file labels along a simple-type restriction chain (for lengthKind SDEs).
    pub fn type_chain_source_labels(&self, name: &TypeName) -> alloc::vec::Vec<String> {
        use alloc::collections::BTreeSet;
        let mut labels = BTreeSet::new();
        self.collect_type_chain_labels(name, &mut labels);
        labels.into_iter().collect()
    }

    /// Schema file labels for lengthKind SDEs (type chain + declaring elements/groups).
    pub fn length_kind_missing_diag_labels(&self, type_name: &TypeName) -> alloc::vec::Vec<String> {
        use alloc::collections::BTreeSet;
        let mut labels = BTreeSet::new();
        self.collect_type_chain_labels(type_name, &mut labels);
        for ge in self.global_elements.values() {
            if ge.type_name == *type_name {
                if let Some(label) = &ge.source_label {
                    labels.insert(label.clone());
                }
            }
        }
        for td in self.types.values() {
            if let TypeDef::Complex { content, source_label, .. } = td {
                if self.complex_content_uses_type(content, type_name) {
                    if let Some(label) = source_label {
                        labels.insert(label.clone());
                    }
                }
            }
        }
        labels.into_iter().collect()
    }

    fn complex_content_uses_type(&self, content: &ComplexContent, type_name: &TypeName) -> bool {
        match content {
            ComplexContent::Empty => false,
            ComplexContent::Sequence(seq) => self.particles_use_type(&seq.particles, type_name),
            ComplexContent::Choice(ch) => self.particles_use_type(&ch.branches, type_name),
        }
    }

    fn particles_use_type(&self, particles: &[Particle], type_name: &TypeName) -> bool {
        particles.iter().any(|p| match p {
            Particle::Element(el) => {
                el.type_name == *type_name
                    || el
                        .element_ref
                        .as_ref()
                        .and_then(|r| crate::schema::get_global_element(self, r))
                        .is_some_and(|g| g.type_name == *type_name)
            }
            Particle::GroupRef(gr) => self
                .groups
                .get(gr.name.rsplit(':').next().unwrap_or(gr.name.as_str()))
                .is_some_and(|g| match g {
                    GroupDecl::Sequence(s) => self.particles_use_type(&s.particles, type_name),
                    GroupDecl::Choice(c) => self.particles_use_type(&c.branches, type_name),
                }),
            Particle::Sequence(seq) => self.particles_use_type(&seq.particles, type_name),
            Particle::Choice(ch) => self.particles_use_type(&ch.branches, type_name),
        })
    }

    fn collect_type_chain_labels(
        &self,
        name: &TypeName,
        out: &mut alloc::collections::BTreeSet<String>,
    ) {
        let Some(td) = self.types.get(name) else {
            return;
        };
        match td {
            TypeDef::Simple {
                base,
                source_label,
                ..
            } => {
                if let Some(label) = source_label {
                    out.insert(label.clone());
                }
                if let SimpleBase::Restriction {
                    base: RestrictionBase::Named(parent),
                    ..
                } = base
                {
                    self.collect_type_chain_labels(parent, out);
                }
            }
            TypeDef::Complex { source_label, .. } => {
                if let Some(label) = source_label {
                    out.insert(label.clone());
                }
            }
        }
    }
}
