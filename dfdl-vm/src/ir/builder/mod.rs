pub(crate) mod element;
pub(crate) mod particle;
pub(crate) mod props;
pub(crate) mod validate;

#[cfg(test)]
mod tests;

use super::{IrNode, IrPrefixLength, IrProgram, IrProps, StringId, StringPool, ValueKind};
use crate::error::{Result, SchemaError};
use crate::length_validate::{validate_data_length_schema, DaffodilTunables};
use crate::schema::{
    DfdlProps, LengthKind, LengthUnits, SchemaDocument, SimpleBase, TypeDef, TypeName,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use props::intern_input_value_calc_expression;
use props::{merge_dfdl_props, overlay_dfdl_to_ir, resolve_escape_scheme};
use validate::{
    validate_delimiter_props, validate_format_has_no_input_value_calc, validate_prefix_length_type,
    validate_text_string_pad_props,
};

pub(crate) struct IrBuilder<'a> {
    pub(crate) schema: &'a SchemaDocument,
    pub(crate) nodes: Vec<IrNode>,
    pub(crate) strings: StringPool,
    pub(crate) defaults: IrProps,
    pub(crate) tunables: DaffodilTunables,
    /// Hidden group refs currently being expanded (cycle detection).
    pub(crate) hidden_group_expand_stack: Vec<String>,
}

impl<'a> IrBuilder<'a> {
    pub(crate) fn new(schema: &'a SchemaDocument, tunables: DaffodilTunables) -> Result<Self> {
        let mut strings = StringPool::new();
        let mut defaults = overlay_dfdl_to_ir(
            IrProps::default(),
            &schema.format_defaults.props,
            &mut strings,
        )?;
        defaults.binary_packed_sign_codes = strings.intern(
            schema
                .format_defaults
                .props
                .binary_packed_sign_codes
                .as_deref()
                .unwrap_or("C D F C"),
        );
        if strings.get(defaults.text_standard_exponent_rep).is_err()
            || strings
                .get(defaults.text_standard_exponent_rep)
                .ok()
                .is_some_and(|s| s.is_empty())
        {
            defaults.text_standard_exponent_rep = strings.intern("E");
        }
        if schema
            .format_defaults
            .props
            .text_standard_exponent_rep
            .is_some()
        {
            defaults.text_standard_exponent_rep_defined = true;
        }
        if schema.format_defaults.props.length_kind_defined {
            defaults.length_kind_defined = true;
        }
        if !defaults.representation_defined {
            let mut ref_name = schema.format_defaults.props.format_ref.as_deref();
            let mut visited = 0u8;
            while let Some(name) = ref_name {
                if visited >= 8 {
                    break;
                }
                visited += 1;
                let key = if let Some((prefix, local)) = name.split_once(':') {
                    let ns = schema.namespace_prefixes.get(prefix).map(String::as_str);
                    crate::schema::format_storage_key(local, ns)
                } else {
                    crate::schema::format_storage_key(name, schema.target_namespace.as_deref())
                };
                if let Some(fmt) = schema.named_formats.get(&key) {
                    if fmt.representation.is_some() {
                        defaults.representation_defined = true;
                        break;
                    }
                    ref_name = fmt.format_ref.as_deref();
                } else {
                    break;
                }
            }
        }
        if defaults.text_standard_infinity_rep == StringId(0) {
            defaults.text_standard_infinity_rep = strings.intern("Inf");
        }
        if defaults.text_standard_nan_rep == StringId(0) {
            defaults.text_standard_nan_rep = strings.intern("NaN");
        }
        if defaults.text_standard_zero_rep_defined
            && strings.get(defaults.text_standard_zero_rep).ok() == Some("")
        {
            defaults.text_standard_zero_rep_defined = false;
        }
        Ok(Self {
            schema,
            nodes: Vec::new(),
            strings,
            defaults,
            tunables,
            hidden_group_expand_stack: Vec::new(),
        })
    }

    pub(crate) fn build(mut self, root_name: &str) -> Result<IrProgram> {
        let root =
            if let Some(root_element) = crate::schema::get_global_element(self.schema, root_name) {
                self.build_root_element_node(root_name, root_element)?
            } else if let Some(type_def) = self.schema.resolve_type(&TypeName::new(root_name)) {
                match type_def {
                    TypeDef::Complex { .. } => {
                        let type_name = TypeName::new(root_name);
                        let child = self.compile_type(
                            &type_name,
                            &DfdlProps::default(),
                            Some(root_name),
                            false,
                        )?;
                        let defaults = self.defaults.clone();
                        let mut ir_props = self.merge_props_full(
                            &defaults,
                            &DfdlProps::default(),
                            &DfdlProps::default(),
                        )?;
                        if self.defaults.length_kind_defined {
                            ir_props.length_kind = self.defaults.length_kind;
                        }
                        let ir_props = props::finalize_element_props(
                            ValueKind::Complex,
                            ir_props,
                            &mut self.strings,
                            self.tunables,
                            Some(self.schema),
                            Some(root_name),
                        )?;
                        let name = self.strings.intern(root_name);
                        self.push(IrNode::Element {
                            name,
                            kind: ValueKind::Complex,
                            props: ir_props,
                            child: Some(child),
                        })
                    }
                    _ => {
                        return Err(SchemaError::UndefinedType {
                            name: root_name.to_string(),
                        }
                        .into());
                    }
                }
            } else {
                return Err(SchemaError::UndefinedType {
                    name: root_name.to_string(),
                }
                .into());
            };

        let program = IrProgram {
            root_element: root_name.to_string(),
            root,
            nodes: self.nodes,
            strings: self.strings,
            tunables: self.tunables,
            variables: self.schema.variables.clone(),
        };
        Ok(program)
    }

    pub(crate) fn push(&mut self, node: IrNode) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        id
    }

    pub(crate) fn merge_props_full(
        &mut self,
        base: &IrProps,
        type_props: &DfdlProps,
        element_props: &DfdlProps,
    ) -> Result<IrProps> {
        validate_delimiter_props(type_props, &DfdlProps::default())?;
        validate_delimiter_props(element_props, element_props)?;
        validate_text_string_pad_props(type_props)?;
        validate_text_string_pad_props(element_props)?;
        let mut ir = merge_dfdl_props(base, type_props, element_props, &mut self.strings)?;
        resolve_escape_scheme(self.schema, type_props, element_props, &mut ir);
        self.attach_prefix_length(type_props, element_props, &mut ir, 0)?;
        Ok(ir)
    }

    pub(crate) fn attach_prefix_length(
        &mut self,
        type_props: &DfdlProps,
        element_props: &DfdlProps,
        ir: &mut IrProps,
        depth: usize,
    ) -> Result<()> {
        if ir.length_kind != LengthKind::Prefixed {
            return Ok(());
        }
        if depth >= 2 {
            return Err(SchemaError::InvalidProperty {
                message:
                    "Schema Definition Error. Nested dfdl:lengthKind=\"prefixed\" is not supported"
                        .into(),
            }
            .into());
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
        let includes_prefix = element_props
            .prefix_includes_prefix_length
            .or(type_props.prefix_includes_prefix_length)
            .unwrap_or(false);
        if includes_prefix {
            if let Some(TypeDef::Simple { props: pprops, .. }) = self.schema.resolve_type(type_name)
            {
                if pprops.length_units == Some(LengthUnits::Bits)
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

    pub(crate) fn resolve_prefix_length_type(
        &mut self,
        type_name: &TypeName,
        depth: usize,
    ) -> Result<IrPrefixLength> {
        let type_def =
            self.schema
                .resolve_type(type_name)
                .ok_or_else(|| SchemaError::UndefinedType {
                    name: type_name.as_str().to_string(),
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
            SimpleBase::Builtin(_) | SimpleBase::Union { .. } => (None, None),
        };
        let mut prefix_props = merge_dfdl_props(
            &self.defaults.clone(),
            props,
            &DfdlProps::default(),
            &mut self.strings,
        )?;
        let kind = element::value_kind_from_simple(self.schema, base);
        validate_prefix_length_type(type_name, props, &prefix_props, kind, &self.strings)?;
        self.attach_prefix_length(props, &DfdlProps::default(), &mut prefix_props, depth + 1)?;
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
            validate_data_length_schema(
                kind,
                len,
                prefix_props.length_units,
                prefix_props.binary_number_rep,
            )?;
        }
        Ok(IrPrefixLength {
            kind,
            props: prefix_props,
            min_inclusive,
            max_inclusive,
        })
    }
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
            let key = schema
                .global_elements
                .keys()
                .next()
                .cloned()
                .ok_or(SchemaError::NoRootElement)?;
            crate::schema::format_local_from_storage_key(&key).to_string()
        }
    };

    if let Some(err) = crate::schema::get_global_element_error(schema, &root_name) {
        return Err(err.clone());
    }

    crate::parse_unparse_policy::validate_parse_unparse_policy(schema, &root_name)?;
    crate::tunable_validate::validate_tunable_schema_requirements(schema, &root_name, &tunables)?;
    crate::schema_validate::validate_compiled_schema(schema, &root_name, &tunables)?;
    validate_format_has_no_input_value_calc(schema)?;
    IrBuilder::new(schema, tunables)?.build(&root_name)
}
