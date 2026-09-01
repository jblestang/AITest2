use super::{ChoiceBranch, IrNode, IrProgram, IrProps, StringId, StringPool, ValueKind};
use crate::error::{Result, SchemaError};
use crate::schema::{
    BuiltinType, ComplexContent, DfdlProps, Particle, SchemaDocument, SimpleBase, TypeDef, TypeName,
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
        let mut strings = StringPool::new();
        let defaults = overlay_dfdl_to_ir(IrProps::default(), &schema.format_defaults.props, &mut strings);
        Self {
            schema,
            nodes: Vec::new(),
            strings,
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

        let root = if let Some(builtin) = BuiltinType::from_xsd(root_element.type_name.as_str()) {
            let props = merge_dfdl_props(
                &self.defaults,
                &DfdlProps::default(),
                &root_element.props,
                &mut self.strings,
            );
            let name = self.strings.intern(root_name);
            self.push(IrNode::Element {
                name,
                kind: value_kind_from_builtin(builtin),
                props,
                child: None,
            })
        } else {
            self.compile_type(&root_element.type_name, &root_element.props)?
        };
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
                let merged = merge_dfdl_props(&self.defaults, props, element_props, &mut self.strings);
                let ir_props = merged;
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
                let merged = merge_dfdl_props(&self.defaults, props, element_props, &mut self.strings);
                self.compile_complex(content, &merged)
            }
        }
    }

    fn compile_particle(&mut self, particle: &Particle, inherited: &IrProps) -> Result<u32> {
        match particle {
            Particle::Element(element) => {
                let props = merge_dfdl_props(inherited, &element.props, &DfdlProps::default(), &mut self.strings);
                let name = self.strings.intern(&element.name);
                if let Some(builtin) = BuiltinType::from_xsd(element.type_name.as_str()) {
                    let ir_props = props;
                    Ok(self.push(IrNode::Element {
                        name,
                        kind: value_kind_from_builtin(builtin),
                        props: ir_props,
                        child: None,
                    }))
                } else {
                    let child = self.compile_type(&element.type_name, &element.props)?;
                    let child_node = self.nodes.get(child as usize).ok_or_else(|| {
                        SchemaError::InvalidProperty {
                            message: alloc::format!("invalid child node id {child}"),
                        }
                    })?;
                    if let IrNode::Element {
                        kind,
                        props: child_props,
                        child: nested,
                        ..
                    } = child_node.clone()
                    {
                        if nested.is_none() && kind != ValueKind::Complex {
                            let overlay = props;
                            return Ok(self.push(IrNode::Element {
                                name,
                                kind,
                                props: merge_ir_props(&child_props, &overlay),
                                child: None,
                            }));
                        }
                    }
                    let ir_props = props;
                    Ok(self.push(IrNode::Element {
                        name,
                        kind: ValueKind::Complex,
                        props: ir_props,
                        child: Some(child),
                    }))
                }
            }
            Particle::Sequence(sequence) => {
                let ir_props = merge_dfdl_props(inherited, &sequence.props, &DfdlProps::default(), &mut self.strings);
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
                let ir_props = merge_dfdl_props(inherited, &choice.props, &DfdlProps::default(), &mut self.strings);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node = self.compile_particle(branch, &ir_props)?;
                    let name = branch_name(branch);
                    let initiator = branch_initiator(branch, &mut self.strings);
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

    fn compile_complex(&mut self, content: &ComplexContent, props: &IrProps) -> Result<u32> {
        match content {
            ComplexContent::Sequence(sequence) => {
                let ir_props = merge_dfdl_props(props, &sequence.props, &DfdlProps::default(), &mut self.strings);
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
                let ir_props = merge_dfdl_props(props, &choice.props, &DfdlProps::default(), &mut self.strings);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node = self.compile_particle(branch, &ir_props)?;
                    branches.push(ChoiceBranch {
                        name: self.strings.intern(branch_name(branch)),
                        initiator: branch_initiator(branch, &mut self.strings),
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
                props: props.clone(),
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

fn branch_initiator(particle: &Particle, strings: &mut StringPool) -> Option<StringId> {
    let raw = match particle {
        Particle::Element(e) => e.props.initiator.as_deref(),
        Particle::Sequence(s) => s.props.initiator.as_deref(),
        Particle::Choice(c) => c.props.initiator.as_deref(),
    };
    raw.map(|s| strings.intern(s))
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

fn merge_dfdl_props(
    base: &IrProps,
    type_props: &DfdlProps,
    element_props: &DfdlProps,
    strings: &mut StringPool,
) -> IrProps {
    let mut out = base.clone();
    out = overlay_dfdl_to_ir(out, type_props, strings);
    overlay_dfdl_to_ir(out, element_props, strings)
}

fn overlay_dfdl_to_ir(mut base: IrProps, props: &DfdlProps, strings: &mut StringPool) -> IrProps {
    if let Some(v) = props.representation {
        base.representation = v;
    }
    if let Some(v) = props.byte_order {
        base.byte_order = v;
    }
    if let Some(v) = props.bit_order {
        base.bit_order = v;
    }
    if let Some(v) = props.length_kind {
        base.length_kind = v;
    }
    if props.length.is_some() {
        base.length = props.length;
    }
    if let Some(v) = props.length_units {
        base.length_units = v;
    }
    if props.encoding.is_some() {
        base.encoding = strings.intern(props.encoding.as_deref().unwrap_or("UTF-8"));
    }
    if let Some(v) = props.text_trim_kind {
        base.text_trim_kind = v;
    }
    if let Some(v) = props.binary_number_rep {
        base.binary_number_rep = v;
    }
    if let Some(v) = props.binary_float_rep {
        base.binary_float_rep = v;
    }
    if let Some(ref s) = props.initiator {
        if !s.is_empty() {
            base.initiator = Some(strings.intern(s.clone()));
        }
    }
    if let Some(ref s) = props.terminator {
        if !s.is_empty() {
            base.terminator = Some(strings.intern(s.clone()));
        }
    }
    if let Some(ref s) = props.separator {
        if !s.is_empty() {
            base.separator = Some(strings.intern(s.clone()));
        }
    }
    if props.occurs_min.is_some() {
        base.occurs_min = props.occurs_min.unwrap_or(1);
    }
    if props.max_occurs_specified {
        base.occurs_max = props.occurs_max;
    }
    if props.length_pattern.is_some() {
        base.length_pattern = props
            .length_pattern
            .as_ref()
            .map(|p| strings.intern(p.clone()));
    }
    if let Some(v) = props.separator_position {
        base.separator_position = v;
    }
    if props.text_boolean_true_rep.is_some() {
        base.text_boolean_true_rep = props
            .text_boolean_true_rep
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.text_boolean_false_rep.is_some() {
        base.text_boolean_false_rep = props
            .text_boolean_false_rep
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.default_value.is_some() {
        base.default_value = props
            .default_value
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.sequence_kind {
        base.sequence_kind = v;
    }
    base
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
        out.initiator = overlay.initiator;
    }
    if overlay.terminator.is_some() {
        out.terminator = overlay.terminator;
    }
    if overlay.separator.is_some() {
        out.separator = overlay.separator;
    }
    if overlay.length_pattern.is_some() {
        out.length_pattern = overlay.length_pattern;
    }
    out.separator_position = overlay.separator_position;
    if overlay.text_boolean_true_rep.is_some() {
        out.text_boolean_true_rep = overlay.text_boolean_true_rep;
    }
    if overlay.text_boolean_false_rep.is_some() {
        out.text_boolean_false_rep = overlay.text_boolean_false_rep;
    }
    if overlay.default_value.is_some() {
        out.default_value = overlay.default_value;
    }
    out.sequence_kind = overlay.sequence_kind;
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
            schema
                .global_elements
                .keys()
                .next()
                .cloned()
                .ok_or(SchemaError::NoRootElement)?
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

    #[test]
    fn text_message_tag_has_fixed_length() {
        use crate::schema::{LengthKind, Representation};
        let xsd = include_str!("../../tests/fixtures/text_message.xsd");
        let schema = parse_schema(xsd).expect("parse");
        let ty = schema.resolve_type(&crate::schema::TypeName::new("MessageType")).unwrap();
        if let crate::schema::TypeDef::Complex { content, .. } = ty {
            if let crate::schema::ComplexContent::Sequence(seq) = content {
                let tag = &seq.particles[0];
                if let crate::schema::Particle::Element(el) = tag {
                    assert_eq!(el.props.length_kind, Some(LengthKind::Fixed));
                    assert_eq!(el.props.length, Some(3));
                    assert_eq!(el.props.representation, Some(Representation::Text));
                }
            }
        }
        let program = compile(&schema).expect("compile");
        let tag_node = program.nodes.iter().find_map(|n| {
            if let IrNode::Element { name, props, .. } = n {
                if program.strings.get(*name).ok() == Some("tag") {
                    return Some(props.clone());
                }
            }
            None
        }).expect("tag node");
        assert_eq!(tag_node.length_kind, LengthKind::Fixed);
        assert_eq!(tag_node.length, Some(3));
    }
}
