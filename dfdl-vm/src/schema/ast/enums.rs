//! DFDL schema AST enums.

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
    HexBinary,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputValueCalcExpression {
    Add(alloc::vec::Vec<InputValueCalcExpression>),
    Sub(alloc::vec::Vec<InputValueCalcExpression>),
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
            bool,
        )>,
    },
    StringOf(alloc::boxed::Box<InputValueCalcExpression>),
    Literal(i64),
    LiteralLexical(alloc::string::String),
    Cast {
        kind: IvcXsCast,
        inner: alloc::boxed::Box<InputValueCalcExpression>,
    },
    Ceiling(alloc::boxed::Box<InputValueCalcExpression>),
    /// `$varName` or `$prefix:varName` from defineVariable.
    Variable(alloc::string::String),
    ValueLength {
        sibling: alloc::string::String,
        units: LengthUnits,
    },
}

/// One segment of `{ fn:concat(...) }` in `dfdl:inputValueCalc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputValueCalcSegment {
    Sibling(alloc::string::String),
    Literal(alloc::string::String),
    Substring {
        sibling: alloc::string::String,
        start: usize,
        length: usize,
    },
    InfosetPath(
        alloc::vec::Vec<(
            Option<alloc::string::String>,
            alloc::string::String,
            Option<u32>,
            bool,
        )>,
    ),
    /// `dfdl:valueLength(../sib, 'bytes')` inside fn:concat.
    ValueLength {
        sibling: alloc::string::String,
        units: LengthUnits,
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
    Substring {
        start: usize,
        length: usize,
    },
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
    /// `{ dfdl:valueLength(../a/b, 'bytes') + N }` — encoded length of path target.
    ValueLengthInfosetPath(LengthUnits, i64),
    /// `{ dfdl:occursIndex() (+|*) ../path (+ N)? }` — path in [`DfdlProps::output_value_calc_path`].
    OccursIndexPath {
        multiply: bool,
    },
    /// `{ fn:count(../path) }` — path in [`DfdlProps::output_value_calc_path`].
    FnCountPath,
    /// `{ fn:concat(...) }` — segments in [`DfdlProps::output_value_calc_segments`].
    FnConcat,
    /// `{ fn:error(...) }` — raw call in [`DfdlProps::output_value_calc_literal`].
    FnError,
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
