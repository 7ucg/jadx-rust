//! Conversion of a parsed class into the input API model.

use jadx_input_api::{
    AnnotationData, AnnotationRef, AnnotationVisibility as ApiVisibility, CallSiteValue,
    ClassAttrs, ClassData, CodeRef, CodeRefProvider, ConstValue, EncodedValueData, FieldAttrs,
    FieldData, FieldRefData, InnerClassData, InsnData, InsnIndexType, InsnResolver, MethodAttrs,
    MethodData, MethodProtoData, MethodRefData,
};

use crate::class::{parse_method_descriptor, JavaClass};
use crate::error::{JavaError, Result};
use crate::types::{Annotation, AnnotationVisibility, EncodedValue, MethodHandleRef};

/// Marker bit of a `newarray` operand in the index of a decoded instruction used with
/// [`InsnResolver::type_desc`]: the low bits hold the primitive element type code.
pub const NEW_ARRAY_PRIMITIVE_TAG: u32 = 0x8000_0000;

/// Annotation value as handed to the decompiler core.
pub fn value_data(v: &EncodedValue) -> EncodedValueData {
    match v {
        EncodedValue::Null => EncodedValueData::Null,
        EncodedValue::Boolean(b) => EncodedValueData::Boolean(*b),
        EncodedValue::Byte(x) => EncodedValueData::Byte(*x),
        EncodedValue::Short(x) => EncodedValueData::Short(*x),
        EncodedValue::Char(x) => EncodedValueData::Char(*x),
        EncodedValue::Int(x) => EncodedValueData::Int(*x),
        EncodedValue::Long(x) => EncodedValueData::Long(*x),
        EncodedValue::Float(x) => EncodedValueData::Float(x.to_bits()),
        EncodedValue::Double(x) => EncodedValueData::Double(x.to_bits()),
        EncodedValue::String(s) => EncodedValueData::Str(s.clone()),
        EncodedValue::Type(t) => EncodedValueData::Type(t.clone()),
        EncodedValue::Enum(f) => EncodedValueData::Enum(f.clone()),
        EncodedValue::MethodType(_) => EncodedValueData::Unsupported("METHOD_TYPE".into()),
        EncodedValue::MethodHandle(_) => EncodedValueData::Unsupported("METHOD_HANDLE".into()),
        EncodedValue::Array(items) => {
            EncodedValueData::Array(items.iter().map(value_data).collect())
        }
        EncodedValue::Annotation(a) => EncodedValueData::Annotation(annotation_data(a)),
    }
}

/// Annotation with its values as handed to the decompiler core.
pub fn annotation_data(a: &Annotation) -> AnnotationData {
    AnnotationData {
        type_desc: a.class.clone(),
        visibility: match a.visibility {
            AnnotationVisibility::Runtime => ApiVisibility::Runtime,
            AnnotationVisibility::Build => ApiVisibility::Build,
        },
        values: a
            .values
            .iter()
            .map(|(k, v)| (k.clone(), value_data(v)))
            .collect(),
    }
}

fn annotations_attr(list: &[Annotation]) -> Vec<AnnotationData> {
    jadx_input_api::pack_annotations(list.iter().map(annotation_data).collect())
}

/// Annotations as seen by the decompiler: one annotation per class (a later one replaces an
/// earlier one of the same class).
pub fn pack_annotations(list: &[Annotation]) -> Vec<&Annotation> {
    let mut out: Vec<&Annotation> = Vec::new();
    for a in list {
        match out.iter_mut().find(|o| o.class == a.class) {
            Some(slot) => *slot = a,
            None => out.push(a),
        }
    }
    out
}

fn collect_annotation_refs(ann: &Annotation, out: &mut Vec<AnnotationRef>) {
    out.push(AnnotationRef::Class(ann.class.clone()));
    for (_, v) in &ann.values {
        collect_value_refs(v, out);
    }
}

fn collect_value_refs(v: &EncodedValue, out: &mut Vec<AnnotationRef>) {
    match v {
        EncodedValue::Type(t) => out.push(AnnotationRef::Class(t.clone())),
        EncodedValue::Enum(f) => out.push(AnnotationRef::Field(f.clone())),
        EncodedValue::Array(items) => items.iter().for_each(|i| collect_value_refs(i, out)),
        EncodedValue::Annotation(a) => collect_annotation_refs(a, out),
        _ => {}
    }
}

/// Classes and fields referenced by a list of annotations.
pub fn annotation_refs(list: &[Annotation]) -> Vec<AnnotationRef> {
    let mut out = Vec::new();
    for a in pack_annotations(list) {
        collect_annotation_refs(a, &mut out);
    }
    out
}

/// Constant field value (`None` for values that are no compile time constants). The value kind
/// is the kind stored in the class file: boolean, byte, short and char constants are `Int`.
pub fn const_value(v: &EncodedValue) -> Option<ConstValue> {
    Some(match v {
        EncodedValue::Boolean(b) => ConstValue::Boolean(*b),
        EncodedValue::Byte(x) => ConstValue::Byte(*x),
        EncodedValue::Short(x) => ConstValue::Short(*x),
        EncodedValue::Char(x) => ConstValue::Char(*x),
        EncodedValue::Int(x) => ConstValue::Int(*x),
        EncodedValue::Long(x) => ConstValue::Long(*x),
        EncodedValue::Float(x) => ConstValue::from_f32(*x),
        EncodedValue::Double(x) => ConstValue::from_f64(*x),
        EncodedValue::String(s) => ConstValue::Str(s.clone()),
        EncodedValue::Type(t) => ConstValue::Str(t.encode_utf16().collect()),
        _ => return None,
    })
}

impl JavaClass {
    /// Class members in the form handed to the decompiler core (no code, only locations).
    pub fn class_api(&self) -> Result<ClassData> {
        let attrs = self.class_attrs()?;
        let type_desc = self.type_desc()?;
        let inner_class = attrs
            .inner_classes
            .as_ref()
            .and_then(|l| l.iter().find(|i| i.inner_class == type_desc))
            .map(|i| InnerClassData {
                outer_class: i.outer_class.clone(),
                name: i.name.clone(),
                access_flags: i.access_flags,
            });
        let fields = self
            .fields()?
            .into_iter()
            .map(|f| FieldData {
                annotation_refs: annotation_refs(&f.annotations),
                const_value: f.const_value.as_ref().and_then(const_value),
                name: f.name,
                ty: f.descriptor,
                access_flags: f.access_flags,
                signature: f.signature,
                attrs: FieldAttrs {
                    annotations: annotations_attr(&f.annotations),
                },
            })
            .collect();
        let mut methods = Vec::new();
        for m in self.methods()? {
            let proto = parse_method_descriptor(&m.descriptor)?;
            let mut refs = annotation_refs(&m.annotations);
            for p in &m.param_annotations {
                refs.extend(annotation_refs(p));
            }
            let attrs = MethodAttrs {
                annotations: annotations_attr(&m.annotations),
                param_annotations: m
                    .param_annotations
                    .iter()
                    .map(|p| annotations_attr(p))
                    .collect(),
                throws: m.exceptions.clone().unwrap_or_default(),
                annotation_default: m.annotation_default.as_ref().map(value_data),
                method_parameters: m
                    .parameters
                    .iter()
                    .flatten()
                    .map(|p| (p.name.clone(), p.access_flags))
                    .collect(),
            };
            methods.push(MethodData {
                name: m.name,
                arg_types: proto.arg_types,
                return_type: proto.return_type,
                access_flags: m.access_flags,
                signature: m.signature,
                code_offset: m.code_offset,
                annotation_refs: refs,
                attrs,
            });
        }
        Ok(ClassData {
            access_flags: self.access_flags()?,
            super_type: self.super_type()?,
            interfaces: self.interfaces()?,
            signature: attrs.signature.clone(),
            inner_class,
            annotation_refs: annotation_refs(&attrs.annotations()),
            fields,
            methods,
            type_desc,
            attrs: ClassAttrs {
                source_file: attrs.source_file.clone().filter(|s| !s.is_empty()),
                input_file_name: Some(self.source().to_string()),
                annotations: annotations_attr(&attrs.annotations()),
            },
        })
    }

    /// Type operand of a decoded instruction (`newarray` carries a primitive element type
    /// code instead of a constant pool index).
    pub fn insn_type(&self, insn: &InsnData) -> Result<String> {
        if insn.raw_opcode_unit == 0xbc {
            return array_elem_type(insn.index);
        }
        self.class_name(insn.index)
    }

    /// Instructions for the decompiler core: like [`decode_insns`](Self::decode_insns), but the
    /// operand of `newarray` is tagged with [`NEW_ARRAY_PRIMITIVE_TAG`] so that
    /// [`InsnResolver::type_desc`] resolves it to the primitive element type.
    pub fn decode_core_insns(&self, code_offset: u32) -> Result<Vec<InsnData>> {
        let mut insns = self.decode_insns(code_offset)?;
        for insn in &mut insns {
            if insn.raw_opcode_unit == 0xbc {
                insn.index |= NEW_ARRAY_PRIMITIVE_TAG;
            }
        }
        Ok(insns)
    }

    /// Type, field and method operands of all instructions of the code at `code_offset`.
    pub fn code_refs(&self, code_offset: u32) -> Result<Vec<CodeRef>> {
        let mut out = Vec::new();
        for insn in self.decode_insns(code_offset)? {
            match insn.index_type {
                InsnIndexType::TypeRef => {
                    if insn.raw_opcode_unit != 0xbc {
                        out.push(CodeRef::Type(self.class_name(insn.index)?));
                    }
                }
                InsnIndexType::FieldRef => out.push(CodeRef::Field(self.field_ref(insn.index)?)),
                InsnIndexType::MethodRef => out.push(CodeRef::Method(self.method_ref(insn.index)?)),
                InsnIndexType::CallSite => {
                    let cs = self.call_site(insn.index)?;
                    if let Some(EncodedValue::MethodHandle(h)) = cs.values.get(4) {
                        if let MethodHandleRef::Method(m) = &h.target {
                            out.push(CodeRef::Method(m.clone()));
                        }
                    }
                }
                InsnIndexType::None | InsnIndexType::StringRef => {}
            }
        }
        Ok(out)
    }
}

/// Element type of a `newarray` instruction.
pub fn array_elem_type(code: u32) -> Result<String> {
    Ok(match code {
        4 => "Z",
        5 => "C",
        6 => "F",
        7 => "D",
        8 => "B",
        9 => "S",
        10 => "I",
        11 => "J",
        v => return Err(JavaError::format(format!("Unknown array type value: {v}"))),
    }
    .to_string())
}

/// Indices of instructions are constant pool indices of the class they belong to. Note that
/// the type operand of `newarray` is a primitive type code: use [`JavaClass::insn_type`] for
/// type operands.
impl InsnResolver for JavaClass {
    fn string(&self, idx: u32) -> std::result::Result<String, String> {
        self.utf8(idx).map_err(|e| e.to_string())
    }

    fn string_utf16(&self, idx: u32) -> std::result::Result<Vec<u16>, String> {
        self.utf8_units(idx).map_err(|e| e.to_string())
    }

    fn type_desc(&self, idx: u32) -> std::result::Result<String, String> {
        if idx & NEW_ARRAY_PRIMITIVE_TAG != 0 {
            return array_elem_type(idx & !NEW_ARRAY_PRIMITIVE_TAG).map_err(|e| e.to_string());
        }
        self.class_name(idx).map_err(|e| e.to_string())
    }

    fn field(&self, idx: u32) -> std::result::Result<FieldRefData, String> {
        self.field_ref(idx).map_err(|e| e.to_string())
    }

    fn method(&self, idx: u32) -> std::result::Result<MethodRefData, String> {
        self.method_ref(idx).map_err(|e| e.to_string())
    }

    fn proto(&self, _idx: u32) -> std::result::Result<MethodProtoData, String> {
        Err("class files have no prototype table".to_string())
    }

    fn call_site(&self, idx: u32) -> std::result::Result<Vec<CallSiteValue>, String> {
        let site = JavaClass::call_site(self, idx).map_err(|e| e.to_string())?;
        site.values.iter().map(call_site_value).collect()
    }
}

/// [`CodeRefProvider`] over loaded classes; `class_idx` is the position in `classes`.
pub struct JavaCodeRefs<'a> {
    pub classes: &'a [JavaClass],
}

impl CodeRefProvider for JavaCodeRefs<'_> {
    fn code_refs(
        &self,
        class_idx: usize,
        code_offset: u32,
    ) -> std::result::Result<Vec<CodeRef>, String> {
        let cls = self
            .classes
            .get(class_idx)
            .ok_or_else(|| format!("no class #{class_idx}"))?;
        cls.code_refs(code_offset).map_err(|e| e.to_string())
    }
}

fn call_site_value(v: &EncodedValue) -> std::result::Result<CallSiteValue, String> {
    use crate::types::MethodHandleKind as J;
    use jadx_input_api::{MethodHandleData, MethodHandleKind as K, MethodHandleTarget as T};
    Ok(match v {
        EncodedValue::Null => CallSiteValue::Null,
        EncodedValue::Boolean(b) => CallSiteValue::Boolean(*b),
        EncodedValue::Byte(x) => CallSiteValue::Byte(*x),
        EncodedValue::Short(x) => CallSiteValue::Short(*x),
        EncodedValue::Char(x) => CallSiteValue::Char(*x),
        EncodedValue::Int(x) => CallSiteValue::Int(*x),
        EncodedValue::Long(x) => CallSiteValue::Long(*x),
        EncodedValue::Float(x) => CallSiteValue::Float(x.to_bits()),
        EncodedValue::Double(x) => CallSiteValue::Double(x.to_bits()),
        EncodedValue::String(s) => CallSiteValue::Str(s.clone()),
        EncodedValue::Type(t) => CallSiteValue::Type(t.clone()),
        EncodedValue::MethodType(p) => CallSiteValue::MethodType(p.clone()),
        EncodedValue::MethodHandle(h) => {
            let kind = match h.kind {
                J::StaticPut => K::StaticPut,
                J::StaticGet => K::StaticGet,
                J::InstancePut => K::InstancePut,
                J::InstanceGet => K::InstanceGet,
                J::InvokeStatic => K::InvokeStatic,
                J::InvokeInstance => K::InvokeInstance,
                J::InvokeDirect => K::InvokeDirect,
                J::InvokeConstructor => K::InvokeConstructor,
                J::InvokeInterface => K::InvokeInterface,
            };
            let target = match &h.target {
                MethodHandleRef::Field(f) => T::Field(f.clone()),
                MethodHandleRef::Method(m) => T::Method(m.clone()),
            };
            CallSiteValue::MethodHandle(MethodHandleData { kind, target })
        }
        _ => CallSiteValue::Other,
    })
}
