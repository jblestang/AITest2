use super::{ChoiceBranch, IrNode, IrProgram, IrProps, StringPool, ValueKind};
use crate::error::{Result, SchemaError};
use crate::schema::{
    BinaryFloatRep, BinaryNumberRep, BitOrder, BuiltinType, ByteOrder, ComplexContent, DfdlProps,
    FormatDefaults, LengthKind, LengthUnits, Particle, Representation, SchemaDocument, SimpleBase,
    TextTrimKind, TypeDef, TypeName,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

struct IrBuilder<'a> {
    schema: &'a SchemaDocument,
    nodes: Vec<IrNode>,
    strings: StringPool,
    defaults: IrProps,
}

impl<'a> IrBuilder<'a> {
    fn new(schema: &'a SchemaDocument) -> Self {
        let defaults = props_from_format(&schema.format_defaults, &mut StringPool::new());
        Self {
            schema,
            nodes: Vec::new(),
            strings: StringPool::new(),
            defaults,
        }
    }

    fn build(mut self, root_name: &str) -> Result<IrProgram> {
        let root_element = self
            .schema
            .global_elements
            .get(root_name)
            .ok_or_else(|| SchemaError::UndefinedType {
                name: root_name.to_string(),
            })?;

        let root = self.compile_type(&root_element.type_name, &root_element.props)?;
        Ok(IrProgram {
            root_element: root_name.to_string(),
            root,
            nodes: self.nodes,
            strings: self.strings,
        })
    }

    fn compile_type(&mut self, type_name: &TypeName, element_props: &DfdlProps) -> Result<u32> {
        if let Some(BuiltinType::String | BuiltinType::HexBinary) = BuiltinType::from_xsd(type_name.as_str()) {
            return Err(SchemaError::UnsupportedFeature {
                feature: alloc::format!("simple type used as root `{type_name:?}`"),
            }
            .into());
        }

        if let Some(builtin) = BuiltinType::from_xsd(type_name.as_str()) {
            return Err(SchemaError::UnsupportedFeature {
                feature: alloc::format!(
                    "scalar root type `{}` requires an element wrapper",
                    builtin.xsd_name()
                ),
            }
            .into());
        }

        let type_def = self
            .schema
            .resolve_type(type_name)
            .ok_or_else(|| SchemaError::UndefinedType {
                name: type_name.as_str().to_string(),
            })?;

        match type_def {
            TypeDef::Simple { base, props, .. } => {
                let merged = merge_dfdl_props(&self.defaults, props, element_props);
                let ir_props = dfdl_to_ir(&merged, &mut self.strings);
                let kind = value_kind_from_simple(base);
                let name = self.strings.intern("__value");
                Ok(self.push(IrNode::Element {
                    name,
                    kind,
                    props: ir_props,
                    child: None,
                }))
            }
            TypeDef::Complex { content, props, .. } => {
                let merged = merge_dfdl_props(&self.defaults, props, element_props);
                self.compile_complex(content, &merged)
            }
        }
    }

    fn compile_particle(&mut self, particle: &Particle, inherited: &IrProps) -> Result<u32> {
        match particle {
            Particle::Element(element) => {
                let props = merge_dfdl_props(inherited, &element.props, &DfdlProps::default());
                let name = self.strings.intern(&element.name);
                if let Some(builtin) = BuiltinType::from_xsd(element.type_name.as_str()) {
                    let ir_props = dfdl_to_ir(&props, &mut self.strings);
                    Ok(self.push(IrNode::Element {
                        name,
                        kind: value_kind_from_builtin(builtin),
                        props: ir_props,
                        child: None,
                    }))
                } else {
                    let child = self.compile_type(&element.type_name, &element.props)?;
                    if let IrNode::Element {
                        kind,
                        props: child_props,
                        child: nested,
                        ..
                    } = self.nodes[child as usize].clone()
                    {
                        if nested.is_none() && kind != ValueKind::Complex {
                            let overlay = dfd_to_ir(&props, &mut self.strings);
                            return Ok(self.push(IrNode::Element {
                                name,
                                kind,
                                props: merge_ir_props(&child_props, &overlay),
                                child: None,
                            }));
                        }
                    }
                    let ir_props = dfdl_to_ir(&props, &mut self.strings);
                    Ok(self.push(IrNode::Element {
                        name,
                        kind: ValueKind::Complex,
                        props: ir_props,
                        child: Some(child),
                    }))
                }
            }
            Particle::Sequence(sequence) => {
                let merged = merge_dfdl_props(inherited, &sequence.props, &DfdlProps::default());
                let ir_props = dfdl_to_ir(&merged, &mut self.strings);
                let mut children = Vec::new();
                for particle in &sequence.particles {
                    children.push(self.compile_particle(particle, &ir_props)?);
                }
                Ok(self.push(IrNode::Sequence {
                    children,
                    props: ir_props,
                }))
            }
            Particle::Choice(choice) => {
                let merged = merge_dfdl_props(inherited, &choice.props, &DfdlProps::default());
                let ir_props = dfdl_to_ir(&merged, &mut self.strings);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node = self.compile_particle(branch, &ir_props)?;
                    let name = branch_name(branch);
                    let initiator = branch_initiator(branch);
                    branches.push(ChoiceBranch {
                        name: self.strings.intern(&name),
                        initiator,
                        node,
                    });
                }
                Ok(self.push(IrNode::Choice {
                    branches,
                    props: ir_props,
                }))
            }
        }
    }

    fn compile_complex(&mut self, content: &ComplexContent, props: &DfdlProps) -> Result<u32> {
        let ir_props = dfdl_to_ir(props, &mut self.strings);
        match content {
            ComplexContent::Sequence(sequence) => {
                let mut children = Vec::new();
                for particle in &sequence.particles {
                    children.push(self.compile_particle(particle, &ir_props)?);
                }
                Ok(self.push(IrNode::Sequence {
                    children,
                    props: ir_props,
                }))
            }
            ComplexContent::Choice(choice) => {
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node = self.compile_particle(branch, &ir_props)?;
                    branches.push(ChoiceBranch {
                        name: self.strings.intern(&branch_name(branch)),
                        initiator: branch_initiator(branch),
                        node,
                    });
                }
                Ok(self.push(IrNode::Choice {
                    branches,
                    props: ir_props,
                }))
            }
            ComplexContent::Empty => Ok(self.push(IrNode::Sequence {
                children: Vec::new(),
                props: ir_props,
            })),
        }
    }

    fn push(&mut self, node: IrNode) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        id
    }
}

fn branch_name(particle: &Particle) -> String {
    match particle {
        Particle::Element(e) => e.name.clone(),
        Particle::Sequence(_) => "sequence".to_string(),
        Particle::Choice(_) => "choice".to_string(),
    }
}

fn branch_initiator(particle: &Particle) -> Option<Vec<u8>> {
    match particle {
        Particle::Element(e) => e.props.initiator.clone(),
        Particle::Sequence(s) => s.props.initiator.clone(),
        Particle::Choice(c) => c.props.initiator.clone(),
    }
}

fn value_kind_from_simple(base: &SimpleBase) -> ValueKind {
    match base {
        SimpleBase::Builtin(b) => value_kind_from_builtin(*b),
        SimpleBase::Restriction { base, .. } => value_kind_from_builtin(*base),
    }
}

fn value_kind_from_builtin(builtin: BuiltinType) -> ValueKind {
    match builtin {
        BuiltinType::Boolean => ValueKind::Boolean,
        BuiltinType::Int => ValueKind::Int,
        BuiltinType::Long => ValueKind::Long,
        BuiltinType::Short => ValueKind::Short,
        BuiltinType::Byte => ValueKind::Byte,
        BuiltinType::UnsignedInt => ValueKind::UnsignedInt,
        BuiltinType::UnsignedShort => ValueKind::UnsignedShort,
        BuiltinType::UnsignedByte => ValueKind::UnsignedByte,
        BuiltinType::Float => ValueKind::Float,
        BuiltinType::Double => ValueKind::Double,
        BuiltinType::String => ValueKind::String,
        BuiltinType::HexBinary => ValueKind::HexBinary,
    }
}

fn props_from_format(format: &FormatDefaults, strings: &mut StringPool) -> IrProps {
    dfdl_to_ir(&format.props, strings)
}

fn merge_dfdl_props(base: &IrProps, type_props: &DfdlProps, element_props: &DfdlProps) -> DfdlProps {
    let mut merged = ir_to_dfdl(base);
    merged = overlay_dfdl(merged, type_props);
    overlay_dfdl(merged, element_props)
}

fn overlay_dfdl(mut base: DfdlProps, overlay: &DfdlProps) -> DfdlProps {
    if overlay.representation.is_some() {
        base.representation = overlay.representation;
    }
    if overlay.byte_order.is_some() {
        base.byte_order = overlay.byte_order;
    }
    if overlay.bit_order.is_some() {
        base.bit_order = overlay.bit_order;
    }
    if overlay.length_kind.is_some() {
        base.length_kind = overlay.length_kind;
    }
    if overlay.length.is_some() {
        base.length = overlay.length;
    }
    if overlay.length_units.is_some() {
        base.length_units = overlay.length_units;
    }
    if overlay.encoding.is_some() {
        base.encoding = overlay.encoding.clone();
    }
    if overlay.text_trim_kind.is_some() {
        base.text_trim_kind = overlay.text_trim_kind;
    }
    if overlay.binary_number_rep.is_some() {
        base.binary_number_rep = overlay.binary_number_rep;
    }
    if overlay.binary_float_rep.is_some() {
        base.binary_float_rep = overlay.binary_float_rep;
    }
    if overlay.initiator.is_some() {
        base.initiator = overlay.initiator.clone();
    }
    if overlay.terminator.is_some() {
        base.terminator = overlay.terminator.clone();
    }
    if overlay.separator.is_some() {
        base.separator = overlay.separator.clone();
    }
    if overlay.occurs_min.is_some() {
        base.occurs_min = overlay.occurs_min;
    }
    if overlay.occurs_max.is_some() {
        base.occurs_max = overlay.occurs_max;
    }
    if overlay.choice_dispatch_key.is_some() {
        base.choice_dispatch_key = overlay.choice_dispatch_key.clone();
    }
    base
}

fn ir_to_dfdl(props: &IrProps) -> DfdlProps {
    DfdlProps {
        representation: Some(props.representation),
        byte_order: Some(props.byte_order),
        bit_order: Some(props.bit_order),
        length_kind: Some(props.length_kind),
        length: props.length,
        length_units: Some(props.length_units),
        encoding: Some(props.encoding.0.to_string()),
        text_trim_kind: Some(props.text_trim_kind),
        binary_number_rep: Some(props.binary_number_rep),
        binary_float_rep: Some(props.binary_float_rep),
        initiator: props.initiator.clone(),
        terminator: props.terminator.clone(),
        separator: props.separator.clone(),
        occurs_min: Some(props.occurs_min),
        occurs_max: props.occurs_max,
        choice_dispatch_key: None,
    }
}

fn dfdl_to_ir(props: &DfdlProps, strings: &mut StringPool) -> IrProps {
    IrProps {
        representation: props.representation.unwrap_or(Representation::Binary),
        byte_order: props.byte_order.unwrap_or(ByteOrder::BigEndian),
        bit_order: props
            .bit_order
            .unwrap_or(BitOrder::MostSignificantBitFirst),
        length_kind: props.length_kind.unwrap_or(LengthKind::Implicit),
        length: props.length,
        length_units: props.length_units.unwrap_or(LengthUnits::Bytes),
        encoding: strings.intern(props.encoding.as_deref().unwrap_or("UTF-8")),
        text_trim_kind: props.text_trim_kind.unwrap_or(TextTrimKind::None),
        binary_number_rep: props
            .binary_number_rep
            .unwrap_or(BinaryNumberRep::Binary),
        binary_float_rep: props.binary_float_rep.unwrap_or(BinaryFloatRep::Ieee),
        initiator: props.initiator.clone(),
        terminator: props.terminator.clone(),
        separator: props.separator.clone(),
        occurs_min: props.occurs_min.unwrap_or(1),
        occurs_max: props.occurs_max.or(Some(1)),
    }
}

fn dfd_to_ir(props: &DfdlProps, strings: &mut StringPool) -> IrProps {
    dfdl_to_ir(props, strings)
}

fn merge_ir_props(base: &IrProps, overlay: &IrProps) -> IrProps {
    let mut out = base.clone();
    out.representation = overlay.representation;
    out.byte_order = overlay.byte_order;
    out.bit_order = overlay.bit_order;
    out.length_kind = overlay.length_kind;
    if overlay.length.is_some() {
        out.length = overlay.length;
    }
    out.length_units = overlay.length_units;
    out.encoding = overlay.encoding;
    out.text_trim_kind = overlay.text_trim_kind;
    out.binary_number_rep = overlay.binary_number_rep;
    out.binary_float_rep = overlay.binary_float_rep;
    if overlay.initiator.is_some() {
        out.initiator = overlay.initiator.clone();
    }
    if overlay.terminator.is_some() {
        out.terminator = overlay.terminator.clone();
    }
    if overlay.separator.is_some() {
        out.separator = overlay.separator.clone();
    }
    out.occurs_min = overlay.occurs_min;
    out.occurs_max = overlay.occurs_max;
    out
}

/// Compile a parsed XSD/DFDL schema document into an [`IrProgram`].
pub fn compile(schema: &SchemaDocument) -> Result<IrProgram> {
    compile_named(schema, None)
}

/// Compile using an explicit root element name, or the sole global element.
pub fn compile_named(schema: &SchemaDocument, root: Option<&str>) -> Result<IrProgram> {
    let root_name = match root {
        Some(name) => name.to_string(),
        None => {
            if schema.global_elements.is_empty() {
                return Err(SchemaError::NoRootElement.into());
            }
            if schema.global_elements.len() > 1 {
                return Err(SchemaError::AmbiguousRootElement.into());
            }
            schema.global_elements.keys().next().unwrap().clone()
        }
    };

    IrBuilder::new(schema).build(&root_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::parse_schema;

    #[test]
    fn compile_record_schema() {
        let xsd = include_str!("../../tests/fixtures/record.xsd");
        let schema = parse_schema(xsd).expect("parse");
        let program = compile(&schema).expect("compile");
        assert_eq!(program.root_element, "Record");
        assert!(!program.nodes.is_empty());
    }
}
