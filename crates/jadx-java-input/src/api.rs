//! Conversion of a parsed class into the input API model.

use jadx_input_api::{
    AnnotationRef, ClassData, CodeRef, CodeRefProvider, ConstValue, FieldData, FieldRefData,
    InnerClassData, InsnData, InsnIndexType, InsnResolver, MethodData, MethodProtoData,
    MethodRefData,
};

use crate::class::{parse_method_descriptor, JavaClass};
use crate::error::{JavaError, Result};
use crate::types::{Annotation, EncodedValue, MethodHandleRef};

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
            })
            .collect();
        let mut methods = Vec::new();
        for m in self.methods()? {
            let proto = parse_method_descriptor(&m.descriptor)?;
            let mut refs = annotation_refs(&m.annotations);
            for p in &m.param_annotations {
                refs.extend(annotation_refs(p));
            }
            methods.push(MethodData {
                name: m.name,
                arg_types: proto.arg_types,
                return_type: proto.return_type,
                access_flags: m.access_flags,
                signature: m.signature,
                code_offset: m.code_offset,
                annotation_refs: refs,
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
