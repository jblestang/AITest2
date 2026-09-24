use crate::error::Result;
use crate::ir::{compile, compile_named, compile_named_with_tunables, IrProgram};
use crate::schema::{parse_schema, SchemaDocument};
use crate::value::DfdlValue;
use crate::vm::{Decoder, Encoder, RuntimeConfig};
use alloc::vec::Vec;

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
        if let Some(msg) = namespace_entity_limit_error(&self.schema, &self.program.root_element) {
            return Err(crate::error::VmError::InvalidValue { message: msg }.into());
        }
        self.decoder().decode(input)
    }

    /// Decode with a TDML-style significant bit length (partial last byte).
    pub fn decode_with_bit_limit(
        &self,
        input: &[u8],
        frame_bits: Option<usize>,
    ) -> Result<DfdlValue> {
        if let Some(msg) = namespace_entity_limit_error(&self.schema, &self.program.root_element) {
            return Err(crate::error::VmError::InvalidValue { message: msg }.into());
        }
        self.decoder()
            .decode_with_bit_limit(input, frame_bits, None)
    }

    pub fn decode_with_bit_limit_and_transmission(
        &self,
        input: &[u8],
        frame_bits: Option<usize>,
        transmission_bit_order: Option<crate::schema::BitOrder>,
    ) -> Result<DfdlValue> {
        if let Some(msg) = namespace_entity_limit_error(&self.schema, &self.program.root_element) {
            return Err(crate::error::VmError::InvalidValue { message: msg }.into());
        }
        self.decoder()
            .decode_with_bit_limit(input, frame_bits, transmission_bit_order)
    }

    /// Convenience: encode a value using a fresh encoder instance.
    pub fn encode(&self, value: &DfdlValue) -> Result<Vec<u8>> {
        if let Some(msg) = namespace_entity_limit_error(&self.schema, &self.program.root_element) {
            return Err(crate::error::VmError::InvalidValue { message: msg }.into());
        }
        self.encoder().encode_to_vec(value)
    }

    /// Encode a value and return trailing bit count in the last byte (0 if byte-aligned).
    pub fn encode_with_bit_count(&self, value: &DfdlValue) -> Result<(Vec<u8>, u8)> {
        self.encode_with_bit_count_config(value, RuntimeConfig::default())
    }

    pub fn encode_with_bit_count_config(
        &self,
        value: &DfdlValue,
        config: RuntimeConfig,
    ) -> Result<(Vec<u8>, u8)> {
        if let Some(msg) = namespace_entity_limit_error(&self.schema, &self.program.root_element) {
            return Err(crate::error::VmError::InvalidValue { message: msg }.into());
        }
        let mut out = Vec::new();
        let bit_count = self
            .encoder_with_config(config)
            .encode_with_bit_count(value, &mut out)?;
        Ok((out, bit_count))
    }
}

pub(crate) fn namespace_entity_limit_error(schema: &SchemaDocument, root: &str) -> Option<String> {
    const MAX_ENTITY_PREFIX_LEN: usize = 5248;
    let ge = crate::schema::get_global_element(schema, root)?;
    let q = ge.type_xsd_qname.as_ref()?;
    let prefix = q.split_once(':')?.0;
    if prefix.len() >= MAX_ENTITY_PREFIX_LEN {
        Some(alloc::format!(
            "length of entity {len}, 5,248 exceeds limit",
            len = prefix.len()
        ))
    } else {
        None
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
