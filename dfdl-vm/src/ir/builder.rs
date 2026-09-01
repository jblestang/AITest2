use super::{ChoiceBranch, IrNode, IrProgram, IrPrefixLength, IrProps, StringId, StringPool, ValueKind};
use crate::error::{Result, SchemaError};
use crate::length_validate::{
    validate_data_length_schema, validate_float_double_bit_length_schema,
    validate_signed_one_bit_length_schema, DaffodilTunables,
};
use crate::schema::{
    BuiltinType, ComplexContent, DfdlProps, LengthKind, LengthUnits, Particle, Representation,
    SchemaDocument, SimpleBase, TypeDef, TypeName,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

struct IrBuilder<'a> {
    schema: &'a SchemaDocument,
    nodes: Vec<IrNode>,
    strings: StringPool,
    defaults: IrProps,
    tunables: DaffodilTunables,
}

impl<'a> IrBuilder<'a> {
    fn new(schema: &'a SchemaDocument, tunables: DaffodilTunables) -> Self {
        let mut strings = StringPool::new();
        let defaults = overlay_dfdl_to_ir(IrProps::default(), &schema.format_defaults.props, &mut strings);
        Self {
            schema,
            nodes: Vec::new(),
            strings,
            defaults,
            tunables,
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
            let kind = value_kind_from_builtin(builtin);
            let defaults = self.defaults.clone();
            let props = finalize_element_props(
                kind,
                self.merge_props_full(&defaults, &DfdlProps::default(), &root_element.props)?,
                &self.strings,
                self.tunables,
            )?;
            validate_implicit_text_length(kind, &props)?;
            let name = self.strings.intern(root_name);
            self.push(IrNode::Element {
                name,
                kind,
                props,
                child: None,
            })
        } else {
            let type_def = self
                .schema
                .resolve_type(&root_element.type_name)
                .ok_or_else(|| SchemaError::UndefinedType {
                    name: root_element.type_name.as_str().to_string(),
                })?;

            if let TypeDef::Simple { base, props, .. } = type_def {
                let defaults = self.defaults.clone();
                let kind = value_kind_from_simple(base);
                let mut ir_props = finalize_element_props(
                    kind,
                    self.merge_props_full(&defaults, props, &root_element.props)?,
                    &self.strings,
                    self.tunables,
                )?;
                apply_restriction_facets(&mut ir_props, base);
                validate_implicit_text_length(kind, &ir_props)?;
                let name = self.strings.intern(root_name);
                self.push(IrNode::Element {
                    name,
                    kind,
                    props: ir_props,
                    child: None,
                })
            } else {
                let child = self.compile_type(&root_element.type_name, &root_element.props)?;
                let defaults = self.defaults.clone();
                let mut ir_props = self.merge_props_full(
                    &defaults,
                    &DfdlProps::default(),
                    &root_element.props,
                )?;
                if root_element.props.length_kind.is_none() {
                    ir_props.length_kind = LengthKind::Implicit;
                }
                let ir_props = finalize_element_props(
                    ValueKind::Complex,
                    ir_props,
                    &self.strings,
                    self.tunables,
                )?;
                let name = self.strings.intern(root_name);
                self.push(IrNode::Element {
                    name,
                    kind: ValueKind::Complex,
                    props: ir_props,
                    child: Some(child),
                })
            }
        };
        Ok(IrProgram {
            root_element: root_name.to_string(),
            root,
            nodes: self.nodes,
            strings: self.strings,
            tunables: self.tunables,
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
                let defaults = self.defaults.clone();
                let kind = value_kind_from_simple(base);
                let mut ir_props = finalize_element_props(
                    kind,
                    self.merge_props_full(&defaults, props, element_props)?,
                    &self.strings,
                    self.tunables,
                )?;
                apply_restriction_facets(&mut ir_props, base);
                validate_implicit_text_length(kind, &ir_props)?;
                let name = self.strings.intern("__value");
                Ok(self.push(IrNode::Element {
                    name,
                    kind,
                    props: ir_props,
                    child: None,
                }))
            }
            TypeDef::Complex { content, props, .. } => {
                let defaults = self.defaults.clone();
                let type_base = self.merge_props_full(&defaults, props, &DfdlProps::default())?;
                self.compile_complex(content, &type_base)
            }
        }
    }

    fn compile_particle(&mut self, particle: &Particle, inherited: &IrProps) -> Result<u32> {
        match particle {
            Particle::Element(element) => {
                let name = self.strings.intern(&element.name);
                if let Some(builtin) = BuiltinType::from_xsd(element.type_name.as_str()) {
                    let kind = value_kind_from_builtin(builtin);
                    let ir_props = finalize_element_props(
                        kind,
                        self.merge_props_full(inherited, &element.props, &DfdlProps::default())?,
                        &self.strings,
                        self.tunables,
                    )?;
                    validate_implicit_text_length(kind, &ir_props)?;
                    Ok(self.push(IrNode::Element {
                        name,
                        kind,
                        props: ir_props,
                        child: None,
                    }))
                } else {
                    let props = self.merge_props_full(inherited, &element.props, &DfdlProps::default())?;
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
                            let mut merged = finalize_element_props(
                                kind,
                                merge_ir_props(&child_props, &overlay),
                                &self.strings,
                                self.tunables,
                            )?;
                            if let Some(type_def) = self.schema.resolve_type(&element.type_name) {
                                if let TypeDef::Simple { base, .. } = type_def {
                                    apply_restriction_facets(&mut merged, base);
                                }
                            }
                            validate_implicit_text_length(kind, &merged)?;
                            return Ok(self.push(IrNode::Element {
                                name,
                                kind,
                                props: merged,
                                child: None,
                            }));
                        }
                    }
                    let mut ir_props = props;
                    if element.props.length_kind.is_none() {
                        ir_props.length_kind = LengthKind::Implicit;
                    }
                    let ir_props =
                        finalize_element_props(ValueKind::Complex, ir_props, &self.strings, self.tunables)?;
                    Ok(self.push(IrNode::Element {
                        name,
                        kind: ValueKind::Complex,
                        props: ir_props,
                        child: Some(child),
                    }))
                }
            }
            Particle::Sequence(sequence) => {
                let ir_props = self.merge_props_full(inherited, &sequence.props, &DfdlProps::default())?;
                let child_inherited =
                    particle_inherited_for_children(&ir_props, &sequence.props, &self.defaults);
                let mut children = Vec::new();
                for particle in &sequence.particles {
                    children.push(self.compile_particle(particle, &child_inherited)?);
                }
                Ok(self.push(IrNode::Sequence {
                    children,
                    props: ir_props,
                }))
            }
            Particle::Choice(choice) => {
                let ir_props = self.merge_props_full(inherited, &choice.props, &DfdlProps::default())?;
                let child_inherited =
                    particle_inherited_for_children(&ir_props, &choice.props, &self.defaults);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node = self.compile_particle(branch, &child_inherited)?;
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

    fn compile_complex(&mut self, content: &ComplexContent, type_base: &IrProps) -> Result<u32> {
        match content {
            ComplexContent::Sequence(sequence) => {
                let ir_props = self.merge_props_full(
                    type_base,
                    &sequence.props,
                    &DfdlProps::default(),
                )?;
                let child_inherited =
                    particle_inherited_for_children(&ir_props, &sequence.props, &self.defaults);
                let mut children = Vec::new();
                for particle in &sequence.particles {
                    children.push(self.compile_particle(particle, &child_inherited)?);
                }
                Ok(self.push(IrNode::Sequence {
                    children,
                    props: ir_props,
                }))
            }
            ComplexContent::Choice(choice) => {
                let ir_props = self.merge_props_full(
                    type_base,
                    &choice.props,
                    &DfdlProps::default(),
                )?;
                let child_inherited =
                    particle_inherited_for_children(&ir_props, &choice.props, &self.defaults);
                let mut branches = Vec::new();
                for branch in &choice.branches {
                    let node = self.compile_particle(branch, &child_inherited)?;
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
                props: type_base.clone(),
            })),
        }
    }

    fn push(&mut self, node: IrNode) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        id
    }

    fn merge_props_full(
        &mut self,
        base: &IrProps,
        type_props: &DfdlProps,
        element_props: &DfdlProps,
    ) -> Result<IrProps> {
        let mut ir = merge_dfdl_props(base, type_props, element_props, &mut self.strings);
        self.attach_prefix_length(type_props, element_props, &mut ir, 0)?;
        Ok(ir)
    }

    fn attach_prefix_length(
        &mut self,
        type_props: &DfdlProps,
        element_props: &DfdlProps,
        ir: &mut IrProps,
        depth: usize,
    ) -> Result<()> {
        if ir.length_kind != LengthKind::Prefixed {
            return Ok(());
        }
        let prefix_type = element_props
            .prefix_length_type
            .as_ref()
            .or(type_props.prefix_length_type.as_ref());
        let Some(type_name) = prefix_type else {
            return Err(SchemaError::InvalidProperty {
                message: "lengthKind=prefixed requires prefixLengthType".into(),
            }
            .into());
        };
        ir.prefix_length = Some(alloc::boxed::Box::new(
            self.resolve_prefix_length_type(type_name, depth)?,
        ));
        ir.prefix_includes_prefix_length = element_props
            .prefix_includes_prefix_length
            .or(type_props.prefix_includes_prefix_length)
            .unwrap_or(ir.prefix_includes_prefix_length);
        if ir.prefix_includes_prefix_length {
            if let Some(ref prefix) = ir.prefix_length {
                if prefix.props.length_units == LengthUnits::Bits
                    && ir.length_units == LengthUnits::Bytes
                {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error. ex:{} dfdl:prefixIncludesPrefixLength=\"yes\" dfdl:prefixLengthType dfdl:lengthUnits",
                            type_name.as_str()
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(())
    }

    fn resolve_prefix_length_type(
        &mut self,
        type_name: &TypeName,
        depth: usize,
    ) -> Result<IrPrefixLength> {
        let type_def = self.schema.resolve_type(type_name).ok_or_else(|| {
            SchemaError::UndefinedType {
                name: type_name.as_str().to_string(),
            }
        })?;
        let TypeDef::Simple { base, props, .. } = type_def else {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error. dfdl:prefixLengthType ex:{} must be simpleType",
                    type_name.as_str()
                ),
            }
            .into());
        };
        if props.has_statement_annotation {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "prefixLengthType `{}` specifies one or more statement annotations such as dfdl:assert",
                    type_name.as_str()
                ),
            }
            .into());
        }
        let (min_inclusive, max_inclusive) = match base {
            SimpleBase::Restriction {
                min_inclusive,
                max_inclusive,
                ..
            } => (*min_inclusive, *max_inclusive),
            SimpleBase::Builtin(_) => (None, None),
        };
        let mut prefix_props =
            merge_dfdl_props(&self.defaults.clone(), props, &DfdlProps::default(), &mut self.strings);
        validate_prefix_length_type(type_name, props, &prefix_props)?;
        if prefix_props.length_kind == LengthKind::Prefixed && depth >= 1 {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error. Nested dfdl:lengthKind=\"prefixed\" not supported"
                    .into(),
            }
            .into());
        }
        self.attach_prefix_length(props, &DfdlProps::default(), &mut prefix_props, depth + 1)?;
        let kind = value_kind_from_simple(base);
        if kind == ValueKind::Decimal {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error. dfdl:prefixLengthType ex:{} xs:decimal subtype xs:integer",
                    type_name.as_str()
                ),
            }
            .into());
        }
        if let Some(len) = prefix_props.length {
            validate_data_length_schema(kind, len, prefix_props.length_units)?;
        }
        Ok(IrPrefixLength {
            kind,
            props: prefix_props,
            min_inclusive,
            max_inclusive,
        })
    }
}

fn validate_prefixed_character_encoding(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<()> {
    if kind != ValueKind::Complex {
        return Ok(());
    }
    if props.length_kind != LengthKind::Prefixed {
        return Ok(());
    }
    if props.length_units != LengthUnits::Characters {
        return Ok(());
    }
    let encoding = strings
        .get(props.encoding)
        .map_err(|e| SchemaError::InvalidProperty {
            message: alloc::format!("invalid encoding reference: {e}"),
        })?;
    if encoding.eq_ignore_ascii_case("utf-8") {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error. Unparsing dfdl:lengthKind='prefixed' with dfdl:lengthUnits='characters' cannot be used with variable-width encoding".into(),
        }
        .into());
    }
    Ok(())
}

fn validate_float_double_bit_length(kind: ValueKind, length: u64, units: LengthUnits) -> Result<()> {
    validate_float_double_bit_length_schema(kind, length, units).map_err(Into::into)
}

fn finalize_element_props(
    kind: ValueKind,
    mut ir: IrProps,
    strings: &StringPool,
    tunables: DaffodilTunables,
) -> Result<IrProps> {
    validate_binary_delimited(kind, &ir)?;
    validate_prefixed_character_encoding(kind, &ir, strings)?;
    validate_end_of_parent(kind, &ir)?;
    if matches!(ir.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        && ir.length.is_none()
        && ir.length_sibling.is_none()
        && ir.length_pattern.is_none()
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property length is not defined".into(),
        }
        .into());
    }
    if matches!(ir.length_kind, LengthKind::Explicit | LengthKind::Fixed) {
        if let Some(len) = ir.length {
            validate_float_double_bit_length(kind, len, ir.length_units)?;
            if kind != ValueKind::Decimal {
                validate_data_length_schema(kind, len, ir.length_units)?;
                validate_signed_one_bit_length_schema(kind, len, ir.length_units, &tunables)?;
            }
        }
    }
    if matches!(kind, ValueKind::Float | ValueKind::Double)
        && matches!(ir.length_kind, LengthKind::Explicit)
        && ir.length_sibling.is_some()
    {
        return Err(SchemaError::InvalidProperty {
            message: "floating point binary numbers may not have runtime-specified lengths".into(),
        }
        .into());
    }
    // Binary schemas default to binary representation, but length-delimited string
    // payloads are still textual. HexBinary keeps binary bytes even with a text prefix.
    if kind == ValueKind::String
        && ir.representation == Representation::Binary
        && matches!(
            ir.length_kind,
            LengthKind::Delimited | LengthKind::Prefixed | LengthKind::Explicit
        )
    {
        ir.representation = Representation::Text;
    }
    Ok(ir)
}

fn validate_binary_delimited(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.representation == Representation::Binary
        && props.length_kind == LengthKind::Delimited
        && !matches!(kind, ValueKind::String | ValueKind::HexBinary)
    {
        return Err(SchemaError::InvalidProperty {
            message: "binary data elements cannot have lengthKind=delimited".into(),
        }
        .into());
    }
    Ok(())
}

fn validate_implicit_text_length(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.length_kind != LengthKind::Implicit {
        return Ok(());
    }
    if props.representation != Representation::Text {
        return Ok(());
    }
    if matches!(kind, ValueKind::String | ValueKind::HexBinary | ValueKind::Complex) {
        return Ok(());
    }
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error. type {} representation text lengthKind implicit is not allowed",
            value_kind_type_name(kind)
        ),
    }
    .into())
}

fn validate_end_of_parent(kind: ValueKind, props: &IrProps) -> Result<()> {
    if props.length_kind != LengthKind::EndOfParent {
        return Ok(());
    }
    if kind == ValueKind::Complex {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error. not implemented endOfParent complex type".into(),
        }
        .into());
    }
    Err(SchemaError::InvalidProperty {
        message: "Schema Definition Error. not implemented endOfParent simple type".into(),
    }
    .into())
}

fn value_kind_type_name(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Boolean => "xs:boolean",
        ValueKind::Byte => "xs:byte",
        ValueKind::Short => "xs:short",
        ValueKind::Int => "xs:int",
        ValueKind::Long => "xs:long",
        ValueKind::UnsignedByte => "xs:unsignedByte",
        ValueKind::UnsignedShort => "xs:unsignedShort",
        ValueKind::UnsignedInt => "xs:unsignedInt",
        ValueKind::Float => "xs:float",
        ValueKind::Double => "xs:double",
        ValueKind::Decimal => "xs:decimal",
        ValueKind::DateTime => "xs:dateTime",
        ValueKind::String => "xs:string",
        ValueKind::HexBinary => "xs:hexBinary",
        ValueKind::Complex => "complex",
    }
}

fn validate_prefix_length_type(
    type_name: &TypeName,
    raw_props: &DfdlProps,
    prefix_props: &IrProps,
) -> Result<()> {
    let qname = alloc::format!("ex:{}", type_name.as_str());
    let prefix_label = alloc::format!("dfdl:prefixLengthType {qname}");

    match prefix_props.length_kind {
        LengthKind::Explicit | LengthKind::Fixed | LengthKind::Implicit | LengthKind::Prefixed => {}
        other => {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error. {qname} {prefix_label} lengthKind {}",
                    length_kind_label(other)
                ),
            }
            .into());
        }
    }

    if matches!(
        prefix_props.length_kind,
        LengthKind::Explicit | LengthKind::Fixed
    ) && prefix_props.length.is_none()
    {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} expression"
            ),
        }
        .into());
    }

    if raw_props.output_value_calc.is_some() {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:outputValueCalc"
            ),
        }
        .into());
    }
    if raw_props.initiator.as_ref().is_some_and(|s| !s.is_empty()) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:initiator"
            ),
        }
        .into());
    }
    if raw_props.terminator.as_ref().is_some_and(|s| !s.is_empty()) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:terminator"
            ),
        }
        .into());
    }
    if raw_props.alignment.is_some() && raw_props.alignment != Some(0) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:alignment"
            ),
        }
        .into());
    }
    if raw_props.leading_skip.is_some() && raw_props.leading_skip != Some(0) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:leadingSkip"
            ),
        }
        .into());
    }
    if raw_props.trailing_skip.is_some() && raw_props.trailing_skip != Some(0) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. {qname} {prefix_label} dfdl:trailingSkip"
            ),
        }
        .into());
    }

    Ok(())
}

fn length_kind_label(kind: LengthKind) -> &'static str {
    match kind {
        LengthKind::Implicit => "implicit",
        LengthKind::Explicit => "explicit",
        LengthKind::Fixed => "fixed",
        LengthKind::Delimited => "delimited",
        LengthKind::Prefixed => "prefixed",
        LengthKind::Pattern => "pattern",
        LengthKind::EndOfParent => "endOfParent",
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

fn restriction_min_length(base: &SimpleBase) -> Option<u64> {
    match base {
        SimpleBase::Restriction { min_length, .. } => *min_length,
        _ => None,
    }
}

fn apply_restriction_facets(props: &mut IrProps, base: &SimpleBase) {
    if let Some(min) = restriction_min_length(base) {
        props.min_length = Some(min);
    }
}

fn value_kind_from_builtin(builtin: BuiltinType) -> ValueKind {
    match builtin {
        BuiltinType::Boolean => ValueKind::Boolean,
        BuiltinType::Int => ValueKind::Int,
        BuiltinType::Long => ValueKind::Long,
        BuiltinType::Short => ValueKind::Short,
        BuiltinType::Byte => ValueKind::Byte,
        BuiltinType::UnsignedInt | BuiltinType::NonNegativeInteger => ValueKind::UnsignedInt,
        BuiltinType::UnsignedShort => ValueKind::UnsignedShort,
        BuiltinType::UnsignedByte => ValueKind::UnsignedByte,
        BuiltinType::Float => ValueKind::Float,
        BuiltinType::Double => ValueKind::Double,
        BuiltinType::Decimal => ValueKind::Decimal,
        BuiltinType::DateTime => ValueKind::DateTime,
        BuiltinType::String => ValueKind::String,
        BuiltinType::HexBinary => ValueKind::HexBinary,
    }
}

/// Inherited props for particles inside a sequence/choice group.
///
/// `lengthKind` on a complex element applies to that element's span in its parent,
/// not to descendants — reset to schema format defaults unless the group sets it.
fn particle_inherited_for_children(
    merged: &IrProps,
    group_props: &DfdlProps,
    defaults: &IrProps,
) -> IrProps {
    let mut inherited = merged.clone();
    if group_props.length_kind.is_none() {
        inherited.length_kind = defaults.length_kind;
    }
    // Element occurrence limits apply to the particle, not descendants.
    if group_props.occurs_min.is_none() {
        inherited.occurs_min = defaults.occurs_min;
    }
    if !group_props.max_occurs_specified {
        inherited.occurs_max = defaults.occurs_max;
    }
    // Initiator/terminator on a group apply to the group node itself, not its children.
    inherited.initiator = None;
    inherited.terminator = None;
    inherited
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
    if props.length_sibling.is_some() {
        base.length_sibling = props
            .length_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
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
    if let Some(v) = props.truncate_specified_length_string {
        base.truncate_specified_length_string = v;
    }
    if props.text_number_pad_character.is_some() {
        base.text_number_pad_character = props
            .text_number_pad_character
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if props.text_string_pad_character.is_some() {
        base.text_string_pad_character = props
            .text_string_pad_character
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.binary_number_rep {
        base.binary_number_rep = v;
    }
    if let Some(v) = props.binary_calendar_rep {
        base.binary_calendar_rep = v;
    }
    if let Some(v) = props.binary_float_rep {
        base.binary_float_rep = v;
    }
    if props.binary_decimal_virtual_point.is_some() {
        base.binary_decimal_virtual_point = props.binary_decimal_virtual_point.unwrap_or(0);
    }
    if let Some(signed) = props.decimal_signed {
        base.decimal_signed = signed;
    }
    if props.calendar_pattern.is_some() {
        base.calendar_pattern = props
            .calendar_pattern
            .as_ref()
            .map(|s| strings.intern(s.clone()));
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
    if let Some(v) = props.input_value_calc {
        base.input_value_calc = Some(v);
    }
    if props.input_value_calc_sibling.is_some() {
        base.input_value_calc_sibling = props
            .input_value_calc_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.output_value_calc {
        base.output_value_calc = Some(v);
    }
    if props.output_value_calc_sibling.is_some() {
        base.output_value_calc_sibling = props
            .output_value_calc_sibling
            .as_ref()
            .map(|s| strings.intern(s.clone()));
    }
    if let Some(v) = props.text_string_justification {
        base.text_string_justification = v;
    }
    if let Some(v) = props.text_number_justification {
        base.text_number_justification = v;
    }
    if props.alignment.is_some() {
        base.alignment = props.alignment.unwrap_or(0);
    }
    if let Some(v) = props.alignment_units {
        base.alignment_units = v;
    }
    if let Some(ref bytes) = props.fill_byte {
        base.fill_byte = bytes.first().copied().unwrap_or(0);
    }
    if let Some(v) = props.prefix_includes_prefix_length {
        base.prefix_includes_prefix_length = v;
    }
    base
}

fn merge_ir_props(base: &IrProps, overlay: &IrProps) -> IrProps {
    let mut out = base.clone();
    out.representation = overlay.representation;
    out.byte_order = overlay.byte_order;
    out.bit_order = overlay.bit_order;
    if matches!(
        base.length_kind,
        LengthKind::Explicit
            | LengthKind::Fixed
            | LengthKind::Prefixed
            | LengthKind::Delimited
            | LengthKind::Pattern
    ) && overlay.length_kind == LengthKind::Implicit
        && overlay.length.is_none()
        && overlay.length_pattern.is_none()
    {
        // Keep type-derived lengthKind when overlay only carries inherited implicit defaults
        // (including when the overlay adds a runtime length expression via length_sibling).
    } else {
        out.length_kind = overlay.length_kind;
    }
    if overlay.length.is_some() {
        out.length = overlay.length;
    }
    if overlay.length_sibling.is_some() {
        out.length_sibling = overlay.length_sibling;
    }
    out.length_units = overlay.length_units;
    out.encoding = overlay.encoding;
    out.text_trim_kind = overlay.text_trim_kind;
    out.text_number_pad_character = overlay.text_number_pad_character;
    out.text_string_pad_character = overlay.text_string_pad_character;
    out.binary_number_rep = overlay.binary_number_rep;
    out.binary_calendar_rep = overlay.binary_calendar_rep;
    out.binary_float_rep = overlay.binary_float_rep;
    out.binary_decimal_virtual_point = overlay.binary_decimal_virtual_point;
    // decimal_signed comes from the resolved simple type; the element wrapper overlay
    // carries schema defaults and must not clobber type-derived decimalSigned.
    out.calendar_pattern = overlay.calendar_pattern;
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
    if overlay.alignment != 0 {
        out.alignment = overlay.alignment;
    }
    out.alignment_units = overlay.alignment_units;
    if overlay.fill_byte != 0 {
        out.fill_byte = overlay.fill_byte;
    }
    out.input_value_calc = overlay.input_value_calc;
    out.input_value_calc_sibling = overlay.input_value_calc_sibling;
    out.output_value_calc = overlay.output_value_calc;
    out.output_value_calc_sibling = overlay.output_value_calc_sibling;
    out.text_string_justification = overlay.text_string_justification;
    out.text_number_justification = overlay.text_number_justification;
    out.truncate_specified_length_string = overlay.truncate_specified_length_string;
    if overlay.min_length.is_some() {
        out.min_length = overlay.min_length;
    }
    if overlay.prefix_length.is_some() {
        out.prefix_length = overlay.prefix_length.clone();
    }
    out.prefix_includes_prefix_length = overlay.prefix_includes_prefix_length;
    out
}

/// Compile a parsed XSD/DFDL schema document into an [`IrProgram`].
pub fn compile(schema: &SchemaDocument) -> Result<IrProgram> {
    compile_named(schema, None)
}

/// Compile using an explicit root element name, or the sole global element.
pub fn compile_named(schema: &SchemaDocument, root: Option<&str>) -> Result<IrProgram> {
    compile_named_with_tunables(schema, root, DaffodilTunables::default())
}

/// Compile with Daffodil tunables from TDML test configuration.
pub fn compile_named_with_tunables(
    schema: &SchemaDocument,
    root: Option<&str>,
    tunables: DaffodilTunables,
) -> Result<IrProgram> {
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

    IrBuilder::new(schema, tunables).build(&root_name)
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
