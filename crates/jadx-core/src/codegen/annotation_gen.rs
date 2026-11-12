//! `AnnotationGen`: annotations of classes, methods, fields and parameters, `throws` lists and
//! encoded values.

use jadx_input_api::{AnnotationData, ConstValue, EncodedValueData, FieldRefData};

use super::class_gen::ClassGen;
use super::code_writer::CodeWriter;
use super::insn_gen::make_static_field_access;
use super::string_utils::{
    format_double, format_float, java_double_to_string, java_float_to_string,
};
use super::type_gen::signature;
use super::CodegenError;
use crate::model::const_storage::FieldRef;
use crate::model::info::FieldInfo;
use crate::model::types::ArgType;

type Res<T = ()> = Result<T, CodegenError>;

const OVERRIDE_ANNOTATION: &str = "Ljava/lang/Override;";

/// `add(node, code)`: all annotations of a node, each on its own line.
pub fn add_annotations(
    cls_gen: &ClassGen<'_>,
    code: &mut CodeWriter,
    list: &[AnnotationData],
) -> Res {
    for a in list {
        if a.type_desc != OVERRIDE_ANNOTATION {
            code.start_line();
            format_annotation(cls_gen, code, a)?;
        }
    }
    Ok(())
}

/// `addForParameter`
pub fn add_for_parameter(
    cls_gen: &ClassGen<'_>,
    code: &mut CodeWriter,
    params: &[Vec<AnnotationData>],
    n: usize,
) -> Res {
    let Some(list) = params.get(n) else {
        return Ok(());
    };
    for a in list {
        format_annotation(cls_gen, code, a)?;
        code.add(" ");
    }
    Ok(())
}

fn desc_type(desc: &str) -> ArgType {
    ArgType::parse(desc).unwrap_or_else(|_| ArgType::object(desc))
}

fn format_annotation(cls_gen: &ClassGen<'_>, code: &mut CodeWriter, a: &AnnotationData) -> Res {
    code.add("@");
    let env = cls_gen.env();
    let ty = desc_type(&a.type_desc);
    let ann_cls = env.root.resolve_class(&ty);
    let ci = env.class_info(&ty)?;
    cls_gen.use_class_info(code, &ci)?;
    if !a.values.is_empty() {
        code.add("(");
        let n = a.values.len();
        for (i, (name, value)) in a.values.iter().enumerate() {
            let param_name = param_name(cls_gen, ann_cls, name);
            if !(param_name == "value" && n == 1) {
                code.add(&param_name);
                code.add(" = ");
            }
            let value = replace_const_value(cls_gen, value);
            encode_value(cls_gen, code, Some(&value))?;
            if i + 1 < n {
                code.add(", ");
            }
        }
        code.add(")");
    }
    Ok(())
}

/// `ModVisitor.replaceConstValue`: constant values of annotations are replaced by references to
/// constant fields.
fn replace_const_value(cls_gen: &ClassGen<'_>, v: &EncodedValueData) -> EncodedValueData {
    match v {
        EncodedValueData::Annotation(a) => EncodedValueData::Annotation(AnnotationData {
            type_desc: a.type_desc.clone(),
            visibility: a.visibility,
            values: a
                .values
                .iter()
                .map(|(k, x)| (k.clone(), replace_const_value(cls_gen, x)))
                .collect(),
        }),
        EncodedValueData::Array(items) => EncodedValueData::Array(
            items
                .iter()
                .map(|i| replace_const_value(cls_gen, i))
                .collect(),
        ),
        other => {
            let cv = match other {
                EncodedValueData::Boolean(b) => ConstValue::Boolean(*b),
                EncodedValueData::Byte(x) => ConstValue::Byte(*x),
                EncodedValueData::Short(x) => ConstValue::Short(*x),
                EncodedValueData::Char(x) => ConstValue::Char(*x),
                EncodedValueData::Int(x) => ConstValue::Int(*x),
                EncodedValueData::Long(x) => ConstValue::Long(*x),
                EncodedValueData::Float(b) => ConstValue::from_f32(f32::from_bits(*b)),
                EncodedValueData::Double(b) => ConstValue::from_f64(f64::from_bits(*b)),
                EncodedValueData::Str(u) => ConstValue::Str(u.clone()),
                _ => return other.clone(),
            };
            let root = cls_gen.env().root;
            let found = root
                .const_storage
                .get_const_field(root, cls_gen.class_index(), &cv, true);
            let info = match found {
                Some(FieldRef::Node(c, f)) => root.classes[*c].fields[*f].info.clone(),
                Some(FieldRef::Info(i)) => i.clone(),
                None => return other.clone(),
            };
            EncodedValueData::Field(FieldRefData {
                parent_class: signature(info.decl_class.arg_type()),
                name: info.name.clone(),
                ty: signature(&info.ty),
            })
        }
    }
}

fn param_name(cls_gen: &ClassGen<'_>, ann_cls: Option<usize>, name: &str) -> String {
    if let Some(c) = ann_cls {
        let cls = &cls_gen.env().root.classes[c];
        if let Some(m) = cls.methods.iter().find(|m| m.info.name == name) {
            return m.alias.clone().unwrap_or_else(|| m.info.name.clone());
        }
    }
    name.to_string()
}

/// `addThrows`
pub fn add_throws(cls_gen: &ClassGen<'_>, code: &mut CodeWriter, throws: &[ArgType]) -> Res {
    if throws.is_empty() {
        return Ok(());
    }
    code.add(" throws ");
    for (i, ex) in throws.iter().enumerate() {
        cls_gen.use_type(code, ex)?;
        if i + 1 < throws.len() {
            code.add(", ");
        }
    }
    Ok(())
}

/// `encodeValue`
pub fn encode_value(
    cls_gen: &ClassGen<'_>,
    code: &mut CodeWriter,
    value: Option<&EncodedValueData>,
) -> Res {
    let Some(value) = value else {
        code.add("null");
        return Ok(());
    };
    let su = &cls_gen.env().string_utils;
    match value {
        EncodedValueData::Null => {
            code.add("null");
        }
        EncodedValueData::Boolean(b) => {
            code.add(if *b { "true" } else { "false" });
        }
        EncodedValueData::Byte(v) => {
            code.add(&su.format_byte(i64::from(*v), false));
        }
        EncodedValueData::Short(v) => {
            code.add(&su.format_short(i64::from(*v), false));
        }
        EncodedValueData::Char(c) => {
            code.add(&su.unescape_char(*c, false));
        }
        EncodedValueData::Int(v) => {
            code.add(&su.format_integer(i64::from(*v), false));
        }
        EncodedValueData::Long(v) => {
            code.add(&su.format_long(*v, false));
        }
        EncodedValueData::Float(bits) => {
            code.add(&format_float(f32::from_bits(*bits)));
        }
        EncodedValueData::Double(bits) => {
            code.add(&format_double(f64::from_bits(*bits)));
        }
        EncodedValueData::Str(units) => {
            code.add(&su.unescape_string(units));
        }
        EncodedValueData::Type(desc) => {
            cls_gen.use_type(code, &desc_type(desc))?;
            code.add(".class");
        }
        EncodedValueData::Field(f) | EncodedValueData::Enum(f) => {
            let env = cls_gen.env();
            let decl = env
                .root
                .ctx
                .class_info(&desc_type(&f.parent_class))
                .map_err(|e| CodegenError::new(format!("Not class type: {e}")))?;
            let info = FieldInfo {
                decl_class: decl,
                name: f.name.clone(),
                ty: desc_type(&f.ty),
            };
            make_static_field_access(code, &info, cls_gen)?;
        }
        EncodedValueData::Method => {}
        EncodedValueData::Array(items) => {
            code.add("{");
            for (i, v) in items.iter().enumerate() {
                encode_value(cls_gen, code, Some(v))?;
                if i + 1 < items.len() {
                    code.add(", ");
                }
            }
            code.add("}");
        }
        EncodedValueData::Annotation(a) => format_annotation(cls_gen, code, a)?,
        EncodedValueData::Unsupported(k) => {
            return Err(CodegenError::new(format!("Can't decode value: {k}")));
        }
    }
    Ok(())
}

/// `toString` of the value object of a constant (as printed in the switch key comment).
pub fn const_value_to_string(v: &ConstValue) -> String {
    match v {
        ConstValue::Boolean(b) => b.to_string(),
        ConstValue::Byte(x) => x.to_string(),
        ConstValue::Short(x) => x.to_string(),
        ConstValue::Char(c) => char::decode_utf16([*c])
            .map(|r| r.unwrap_or('\u{fffd}'))
            .collect(),
        ConstValue::Int(x) => x.to_string(),
        ConstValue::Long(x) => x.to_string(),
        ConstValue::Float(bits) => java_float_to_string(f32::from_bits(*bits)),
        ConstValue::Double(bits) => java_double_to_string(f64::from_bits(*bits)),
        ConstValue::Str(u) => String::from_utf16_lossy(u),
        ConstValue::Null => "null".to_string(),
    }
}
