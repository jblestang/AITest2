use alloc::vec::Vec;
use crate::error::Result;
use crate::ir::{compile, compile_named, compile_named_with_tunables, IrProgram};
use crate::schema::{parse_schema, SchemaDocument};
use crate::value::DfdlValue;
use crate::vm::{Decoder, Encoder, RuntimeConfig};

/// Compiled DFDL specification: XSD parsed, IR built, ready for VM encode/decode.
#[derive(Debug, Clone)]
pub struct DfdlSpec {
    schema: SchemaDocument,
    program: IrProgram,
}

impl DfdlSpec {
    /// Parse XSD + DFDL annotations and compile to IR.
    pub fn from_xsd(xsd: &str) -> Result<Self> {
        Self::from_xsd_root(xsd, None)
    }

    /// Parse and compile, selecting a specific global root element.
    pub fn from_xsd_root(xsd: &str, root_element: Option<&str>) -> Result<Self> {
        let schema = parse_schema(xsd)?;
        let program = compile_named(&schema, root_element)?;
        Ok(Self { schema, program })
    }

    /// Build from an already parsed schema document.
    pub fn from_schema(schema: SchemaDocument) -> Result<Self> {
        let program = compile(&schema)?;
        Ok(Self { schema, program })
    }

    /// Build from a parsed schema with tunables and optional root element.
    pub fn from_schema_root_with_tunables(
        schema: SchemaDocument,
        root_element: Option<&str>,
        tunables: crate::length_validate::DaffodilTunables,
    ) -> Result<Self> {
        let program = compile_named_with_tunables(&schema, root_element, tunables)?;
        Ok(Self { schema, program })
    }

    pub fn schema(&self) -> &SchemaDocument {
        &self.schema
    }

    pub fn program(&self) -> &IrProgram {
        &self.program
    }

    pub fn root_element(&self) -> &str {
        &self.program.root_element
    }

    /// Create a decoder VM bound to this specification.
    pub fn decoder(&self) -> Decoder<'_> {
        Decoder::new(&self.program)
    }

    pub fn decoder_with_config(&self, config: RuntimeConfig) -> Decoder<'_> {
        Decoder::with_config(&self.program, config)
    }

    /// Create an encoder VM bound to this specification.
    pub fn encoder(&self) -> Encoder<'_> {
        Encoder::new(&self.program)
    }

    pub fn encoder_with_config(&self, config: RuntimeConfig) -> Encoder<'_> {
        Encoder::with_config(&self.program, config)
    }

    /// Convenience: decode bytes using a fresh decoder instance.
    pub fn decode(&self, input: &[u8]) -> Result<DfdlValue> {
        self.decoder().decode(input)
    }

    /// Convenience: encode a value using a fresh encoder instance.
    pub fn encode(&self, value: &DfdlValue) -> Result<Vec<u8>> {
        self.encoder().encode_to_vec(value)
    }
}

/// Alias for the primary entry type.
pub type DfdlSchema = DfdlSpec;

/// Owned codec holding spec + reusable encoder/decoder facades.
pub struct DfdlCodec {
    spec: DfdlSpec,
}

impl DfdlCodec {
    pub fn from_xsd(xsd: &str) -> Result<Self> {
        Ok(Self {
            spec: DfdlSpec::from_xsd(xsd)?,
        })
    }

    pub fn spec(&self) -> &DfdlSpec {
        &self.spec
    }

    pub fn decode(&self, input: &[u8]) -> Result<DfdlValue> {
        self.spec.decode(input)
    }

    pub fn encode(&self, value: &DfdlValue) -> Result<Vec<u8>> {
        self.spec.encode(value)
    }

    pub fn decoder(&self) -> Decoder<'_> {
        self.spec.decoder()
    }

    pub fn encoder(&self) -> Encoder<'_> {
        self.spec.encoder()
    }
}
