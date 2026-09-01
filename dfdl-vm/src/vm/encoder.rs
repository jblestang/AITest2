use super::runtime::{write_simple, RuntimeConfig, VmContext};
use crate::schema::encode_delimiter;
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::schema::SeparatorPosition;
use crate::value::DfdlValue;
use alloc::vec::Vec;

/// DFDL encoder VM — executes compiled IR to serialize logical values.
pub struct Encoder<'a> {
    ctx: VmContext<'a>,
}

impl<'a> Encoder<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self::with_config(program, RuntimeConfig::default())
    }

    pub fn with_config(program: &'a IrProgram, config: RuntimeConfig) -> Self {
        Self {
            ctx: VmContext { program, config },
        }
    }

    /// Encode `value` and append bytes to `output`.
    pub fn encode(&self, value: &DfdlValue, output: &mut Vec<u8>) -> Result<()> {
        let value = unwrap_root_for_encode(value, &self.ctx.program.root_element);
        self.encode_node(self.ctx.program.root, value, output)
    }

    /// Encode into a freshly allocated buffer.
    pub fn encode_to_vec(&self, value: &DfdlValue) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(value, &mut out)?;
        Ok(out)
    }

    fn encode_node(&self, node_id: u32, value: &DfdlValue, out: &mut Vec<u8>) -> Result<()> {
        match self.ctx.program.node(node_id)? {
            IrNode::Sequence { children, props } => {
                let map = value.as_sequence_fields()?;
                for (idx, &child) in children.iter().enumerate() {
                    self.write_separator(props, out, idx, children.len())?;
                    self.encode_sequence_child(child, map, out)?;
                }
                Ok(())
            }
            IrNode::Choice { branches, .. } => {
                let (discriminator, branch_value) = value.as_choice_fields()?;
                let branch = branches
                    .iter()
                    .find(|b| {
                        self.ctx
                            .strings()
                            .get(b.name)
                            .map(|name| name == discriminator)
                            .unwrap_or(false)
                    })
                    .ok_or(VmError::InvalidChoice)?;
                self.encode_node(branch.node, branch_value, out)
            }
            IrNode::Element {
                name,
                kind,
                props,
                child,
            } => {
                if let Some(child_id) = child {
                    let field = value_for_element_or_content(
                        value,
                        self.ctx.strings().get(*name)?,
                    )?;
                    let items = match field {
                        DfdlValue::Array(items) => items.as_slice(),
                        single => core::slice::from_ref(single),
                    };
                    for (idx, item) in items.iter().enumerate() {
                        self.write_separator(props, out, idx, items.len())?;
                        self.write_initiator(props, out)?;
                        self.encode_node(*child_id, item, out)?;
                        self.write_terminator(props, out)?;
                    }
                    Ok(())
                } else {
                    write_simple(out, value, *kind, props, self.ctx.strings()).map_err(Into::into)
                }
            }
        }
    }

    fn encode_element_occurrences(
        &self,
        node_id: u32,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        for (idx, item) in items.iter().enumerate() {
            self.write_separator(props, out, idx, items.len())?;
            self.encode_node(node_id, item, out)?;
        }
        Ok(())
    }

    fn encode_sequence_child(
        &self,
        node_id: u32,
        map: &alloc::collections::BTreeMap<alloc::string::String, DfdlValue>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        match self.ctx.program.node(node_id)? {
            IrNode::Element { name, props, .. } => {
                let key = self.ctx.strings().get(*name)?;
                let value = match map.get(key) {
                    Some(v) => v,
                    None if props.occurs_min == 0 => return Ok(()),
                    None => {
                        return Err(VmError::MissingField { name: key.into() }.into());
                    }
                };
                self.encode_element_occurrences(node_id, props, value, out)
            }
            IrNode::Sequence { .. } => {
                self.encode_node(node_id, &DfdlValue::Sequence(map.clone()), out)
            }
            IrNode::Choice { .. } => {
                for (discriminator, value) in map {
                    if branches_contain(self.ctx.program, node_id, discriminator) {
                        return self.encode_node(
                            node_id,
                            &DfdlValue::choice(discriminator.clone(), value.clone()),
                            out,
                        );
                    }
                }
                Err(VmError::InvalidChoice.into())
            }
        }
    }

    fn write_initiator(&self, props: &IrProps, out: &mut Vec<u8>) -> Result<()> {
        if let Some(id) = props.initiator {
            let pat = self.ctx.strings().get(id)?;
            if !pat.is_empty() {
                out.extend(encode_delimiter(pat));
            }
        }
        Ok(())
    }

    fn write_terminator(&self, props: &IrProps, out: &mut Vec<u8>) -> Result<()> {
        if let Some(id) = props.terminator {
            let pat = self.ctx.strings().get(id)?;
            if !pat.is_empty() {
                out.extend(encode_delimiter(pat));
            }
        }
        Ok(())
    }

    fn write_separator(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        index: usize,
        total: usize,
    ) -> Result<()> {
        if !should_emit_separator(props.separator_position, index, total) {
            return Ok(());
        }
        if let Some(id) = props.separator {
            out.extend(encode_delimiter(self.ctx.strings().get(id)?));
        }
        Ok(())
    }
}

fn should_emit_separator(position: SeparatorPosition, index: usize, total: usize) -> bool {
    match position {
        SeparatorPosition::Prefix => index < total,
        SeparatorPosition::Infix => index > 0,
        SeparatorPosition::Postfix => index + 1 < total,
    }
}

fn unwrap_root_for_encode<'a>(value: &'a DfdlValue, root_element: &str) -> &'a DfdlValue {
    if let DfdlValue::Sequence(map) = value {
        if map.len() == 1 {
            if let Some((name, inner)) = map.iter().next() {
                if name == root_element {
                    return inner;
                }
            }
        }
    }
    value
}

fn value_for_element_or_content<'a>(value: &'a DfdlValue, name: &str) -> Result<&'a DfdlValue> {
    match value {
        DfdlValue::Sequence(map) => {
            if let Some(v) = map.get(name) {
                Ok(v)
            } else if !map.is_empty() {
                Ok(value)
            } else {
                Err(VmError::MissingField { name: name.into() }.into())
            }
        }
        DfdlValue::Choice { .. } => Ok(value),
        other => Ok(other),
    }
}

fn branches_contain(program: &IrProgram, node_id: u32, name: &str) -> bool {
    match program.node(node_id) {
        Ok(IrNode::Choice { branches, .. }) => branches.iter().any(|b| {
            program
                .strings
                .get(b.name)
                .map(|branch_name| branch_name == name)
                .unwrap_or(false)
        }),
        _ => false,
    }
}

trait ValueView {
    fn as_sequence_fields(
        &self,
    ) -> Result<&alloc::collections::BTreeMap<alloc::string::String, DfdlValue>>;
    fn as_choice_fields(&self) -> Result<(&str, &DfdlValue)>;
}

#[cfg(test)]
mod tests {
    use crate::DfdlSpec;

    #[test]
    fn decode_consecutive_empty_csv_fields() {
        use crate::schema::parse_schema;
        use crate::DfdlSpec;
        let xsd = r##"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/">
  <dfdl:format representation="text" encoding="UTF-8" lengthKind="delimited"/>
  <xs:element name="Row" dfdl:terminator=";">
    <xs:complexType>
      <xs:sequence dfdl:separator="," dfdl:separatorPosition="infix">
        <xs:element name="field" type="xs:string" minOccurs="0" maxOccurs="unbounded"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"##;
        parse_schema(xsd).expect("parse");
        let spec = DfdlSpec::from_xsd(xsd).expect("spec");
        let decoded = spec.decode(b"a,,;").expect("decode");
        match decoded.field("field").expect("fields") {
            crate::DfdlValue::Array(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], crate::DfdlValue::String("a".into()));
                assert_eq!(items[1], crate::DfdlValue::String("".into()));
                assert_eq!(items[2], crate::DfdlValue::String("".into()));
            }
            other => panic!("expected array, got {other:?}"),
        }
    }

    #[test]
    fn encodes_nmea_root_line_ending() {
        let input = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A\r\n";
        let spec = DfdlSpec::from_xsd(include_str!(
            "../../tests/fixtures/nmea_sentence.xsd"
        ))
        .expect("spec");
        let decoded = spec.decode(input).expect("decode");
        let encoded = spec.encode(&decoded).expect("encode");
        assert_eq!(encoded, input);
    }
}

impl ValueView for DfdlValue {
    fn as_sequence_fields(
        &self,
    ) -> Result<&alloc::collections::BTreeMap<alloc::string::String, DfdlValue>> {
        match self {
            DfdlValue::Sequence(map) => Ok(map),
            _ => Err(VmError::TypeMismatch {
                expected: "sequence".into(),
            }
            .into()),
        }
    }

    fn as_choice_fields(&self) -> Result<(&str, &DfdlValue)> {
        match self {
            DfdlValue::Choice { discriminator, value } => Ok((discriminator, value)),
            _ => Err(VmError::TypeMismatch {
                expected: "choice".into(),
            }
            .into()),
        }
    }
}
