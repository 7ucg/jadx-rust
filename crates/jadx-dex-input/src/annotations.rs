//! Annotations and encoded values.

use std::collections::HashMap;

use jadx_input_api::{
    pack_annotations, AnnotationData, AnnotationRef, AnnotationVisibility, ConstValue,
    EncodedValueData, FieldRefData, InnerClassData,
};

use crate::error::{DexError, Result};
use crate::file::{ClassDef, DexFile};
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Build,
    Runtime,
    System,
}

impl Visibility {
    pub fn name(self) -> &'static str {
        match self {
            Visibility::Build => "build",
            Visibility::Runtime => "runtime",
            Visibility::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EncodedValue {
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    MethodType(u32),
    MethodHandle(u32),
    String(u32),
    Type(u32),
    Field(u32),
    Method(u32),
    Enum(u32),
    Array(Vec<EncodedValue>),
    Annotation(EncodedAnnotation),
    Null,
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EncodedAnnotation {
    pub type_idx: u32,
    pub elements: Vec<(u32, EncodedValue)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub visibility: Visibility,
    pub annotation: EncodedAnnotation,
}

/// All annotations attached to one class, by target.
#[derive(Debug, Clone, Default)]
pub struct ClassAnnotations {
    pub class: Vec<Annotation>,
    pub fields: HashMap<u32, Vec<Annotation>>,
    pub methods: HashMap<u32, Vec<Annotation>>,
    /// Per method, per parameter.
    pub parameters: HashMap<u32, Vec<Vec<Annotation>>>,
}

const MAX_DEPTH: u32 = 64;

pub fn read_encoded_value(r: &mut Reader<'_>, depth: u32) -> Result<EncodedValue> {
    if depth > MAX_DEPTH {
        return Err(DexError::BadContainer(
            "encoded value nesting too deep".into(),
        ));
    }
    let header = r.u8()?;
    let ty = header & 0x1F;
    let arg = (header >> 5) as usize;
    let size = arg + 1;
    // little-endian, zero-extended
    let zext = |r: &mut Reader<'_>, n: usize| -> Result<u64> {
        let b = r.bytes(n)?;
        Ok(b.iter()
            .enumerate()
            .fold(0u64, |acc, (i, v)| acc | (*v as u64) << (8 * i)))
    };
    // little-endian, sign-extended
    let sext = |r: &mut Reader<'_>, n: usize| -> Result<i64> {
        let b = r.bytes(n)?;
        let v = b
            .iter()
            .enumerate()
            .fold(0u64, |acc, (i, v)| acc | (*v as u64) << (8 * i));
        let shift = 64 - 8 * n as u32;
        Ok(((v << shift) as i64) >> shift)
    };
    // right-zero-extended (floats/doubles store the most significant bytes)
    let rzext = |r: &mut Reader<'_>, n: usize, total: usize| -> Result<u64> {
        let b = r.bytes(n)?;
        let v = b
            .iter()
            .enumerate()
            .fold(0u64, |acc, (i, v)| acc | (*v as u64) << (8 * i));
        Ok(v << (8 * (total - n) as u32))
    };
    Ok(match ty {
        0x00 => EncodedValue::Byte(sext(r, 1)? as i8),
        0x02 => EncodedValue::Short(sext(r, size.min(2))? as i16),
        0x03 => EncodedValue::Char(zext(r, size.min(2))? as u16),
        0x04 => EncodedValue::Int(sext(r, size.min(4))? as i32),
        0x06 => EncodedValue::Long(sext(r, size.min(8))?),
        0x10 => EncodedValue::Float(f32::from_bits(rzext(r, size.min(4), 4)? as u32)),
        0x11 => EncodedValue::Double(f64::from_bits(rzext(r, size.min(8), 8)?)),
        0x15 => EncodedValue::MethodType(zext(r, size.min(4))? as u32),
        0x16 => EncodedValue::MethodHandle(zext(r, size.min(4))? as u32),
        0x17 => EncodedValue::String(zext(r, size.min(4))? as u32),
        0x18 => EncodedValue::Type(zext(r, size.min(4))? as u32),
        0x19 => EncodedValue::Field(zext(r, size.min(4))? as u32),
        0x1a => EncodedValue::Method(zext(r, size.min(4))? as u32),
        0x1b => EncodedValue::Enum(zext(r, size.min(4))? as u32),
        0x1c => {
            let n = r.uleb128()?;
            let mut v = Vec::with_capacity((n as usize).min(1024));
            for _ in 0..n {
                v.push(read_encoded_value(r, depth + 1)?);
            }
            EncodedValue::Array(v)
        }
        0x1d => EncodedValue::Annotation(read_encoded_annotation(r, depth + 1)?),
        0x1e => EncodedValue::Null,
        0x1f => EncodedValue::Boolean(arg != 0),
        _ => {
            return Err(DexError::BadContainer(format!(
                "unknown encoded value type 0x{ty:02x}"
            )))
        }
    })
}

pub fn read_encoded_annotation(r: &mut Reader<'_>, depth: u32) -> Result<EncodedAnnotation> {
    let type_idx = r.uleb128()?;
    let n = r.uleb128()?;
    let mut elements = Vec::with_capacity((n as usize).min(256));
    for _ in 0..n {
        let name = r.uleb128()?;
        elements.push((name, read_encoded_value(r, depth)?));
    }
    Ok(EncodedAnnotation { type_idx, elements })
}

impl DexFile {
    /// `encoded_array_item` of static field initial values (`static_values_off`).
    pub fn static_values(&self, def: &ClassDef) -> Result<Vec<EncodedValue>> {
        if def.static_values_off == 0 {
            return Ok(Vec::new());
        }
        let mut r = Reader::new(self.bytes(), def.static_values_off as usize);
        let n = r.uleb128()?;
        let mut v = Vec::with_capacity((n as usize).min(1024));
        for _ in 0..n {
            v.push(read_encoded_value(&mut r, 0)?);
        }
        Ok(v)
    }

    /// Encoded array of a call site item: `[linker method handle, name, proto, extra args...]`.
    pub fn call_site(&self, idx: u32) -> Result<Vec<EncodedValue>> {
        if idx >= self.header().call_site_ids_size {
            return Err(DexError::IndexOutOfRange {
                what: "call_site",
                idx,
                count: self.header().call_site_ids_size,
            });
        }
        let mut r = Reader::new(
            self.bytes(),
            self.header().call_site_ids_off as usize + idx as usize * 4,
        );
        let off = r.u32()?;
        let mut r = Reader::new(self.bytes(), off as usize);
        let n = r.uleb128()?;
        let mut v = Vec::with_capacity((n as usize).min(64));
        for _ in 0..n {
            v.push(read_encoded_value(&mut r, 0)?);
        }
        Ok(v)
    }

    pub fn annotation_set(&self, off: u32) -> Result<Vec<Annotation>> {
        if off == 0 {
            return Ok(Vec::new());
        }
        let mut r = Reader::new(self.bytes(), off as usize);
        let n = r.u32()?;
        let mut offs = Vec::with_capacity((n as usize).min(256));
        for _ in 0..n {
            offs.push(r.u32()?);
        }
        let mut out = Vec::with_capacity(offs.len());
        for a_off in offs {
            let mut ar = Reader::new(self.bytes(), a_off as usize);
            let vis = match ar.u8()? {
                0 => Visibility::Build,
                1 => Visibility::Runtime,
                2 => Visibility::System,
                v => {
                    return Err(DexError::BadContainer(format!(
                        "bad annotation visibility {v}"
                    )))
                }
            };
            out.push(Annotation {
                visibility: vis,
                annotation: read_encoded_annotation(&mut ar, 0)?,
            });
        }
        Ok(out)
    }

    fn annotation_set_ref_list(&self, off: u32) -> Result<Vec<Vec<Annotation>>> {
        let mut r = Reader::new(self.bytes(), off as usize);
        let n = r.u32()?;
        let mut out = Vec::with_capacity((n as usize).min(256));
        for _ in 0..n {
            let set_off = r.u32()?;
            out.push(self.annotation_set(set_off)?);
        }
        Ok(out)
    }

    pub fn class_annotations(&self, def: &ClassDef) -> Result<ClassAnnotations> {
        let mut out = ClassAnnotations::default();
        if def.annotations_off == 0 {
            return Ok(out);
        }
        let mut r = Reader::new(self.bytes(), def.annotations_off as usize);
        let class_off = r.u32()?;
        let fields_n = r.u32()?;
        let methods_n = r.u32()?;
        let params_n = r.u32()?;
        out.class = self.annotation_set(class_off)?;
        let pairs = |n: u32, r: &mut Reader<'_>| -> Result<Vec<(u32, u32)>> {
            let mut v = Vec::with_capacity((n as usize).min(1024));
            for _ in 0..n {
                v.push((r.u32()?, r.u32()?));
            }
            Ok(v)
        };
        let f = pairs(fields_n, &mut r)?;
        let m = pairs(methods_n, &mut r)?;
        let p = pairs(params_n, &mut r)?;
        for (idx, off) in f {
            out.fields.insert(idx, self.annotation_set(off)?);
        }
        for (idx, off) in m {
            out.methods.insert(idx, self.annotation_set(off)?);
        }
        for (idx, off) in p {
            out.parameters
                .insert(idx, self.annotation_set_ref_list(off)?);
        }
        Ok(out)
    }
}

impl DexFile {
    /// Extract the generic signature (`dalvik.annotation.Signature`) from an annotation list.
    fn signature_of(&self, anns: &[Annotation]) -> Result<Option<String>> {
        for a in anns {
            if self.type_descriptor(a.annotation.type_idx)? != "Ldalvik/annotation/Signature;" {
                continue;
            }
            for (name_idx, value) in &a.annotation.elements {
                if self.string(*name_idx)? != "value" {
                    continue;
                }
                if let EncodedValue::Array(parts) = value {
                    let mut sig = String::new();
                    for p in parts {
                        if let EncodedValue::String(i) = p {
                            sig.push_str(&self.string(*i)?);
                        }
                    }
                    return Ok(Some(sig));
                }
            }
        }
        Ok(None)
    }

    /// `InnerClass` annotation data (`dalvik.annotation.InnerClass` and `EnclosingClass`).
    fn inner_class_of(&self, anns: &[Annotation]) -> Result<Option<InnerClassData>> {
        for a in anns {
            if a.visibility != Visibility::System
                || self.type_descriptor(a.annotation.type_idx)? != "Ldalvik/annotation/InnerClass;"
            {
                continue;
            }
            let mut name = None;
            let mut access_flags = 0;
            for (name_idx, value) in &a.annotation.elements {
                match (self.string(*name_idx)?.as_str(), value) {
                    ("name", EncodedValue::String(i)) => name = Some(self.string(*i)?),
                    ("accessFlags", EncodedValue::Int(v)) => access_flags = *v as u32,
                    _ => {}
                }
            }
            if name.is_none() && access_flags == 0 {
                continue;
            }
            return Ok(Some(InnerClassData {
                outer_class: self.enclosing_class_of(anns)?,
                name,
                access_flags,
            }));
        }
        Ok(None)
    }

    fn enclosing_class_of(&self, anns: &[Annotation]) -> Result<Option<String>> {
        for a in anns {
            if a.visibility != Visibility::System
                || self.type_descriptor(a.annotation.type_idx)?
                    != "Ldalvik/annotation/EnclosingClass;"
            {
                continue;
            }
            for (name_idx, value) in &a.annotation.elements {
                if let ("value", EncodedValue::Type(t)) = (self.string(*name_idx)?.as_str(), value)
                {
                    return Ok(Some(self.type_descriptor(*t)?));
                }
            }
        }
        Ok(None)
    }

    /// Classes and fields used by the annotations that are visible to the decompiler (all but
    /// system annotations, one annotation per class).
    fn annotation_refs_of(&self, anns: &[Annotation]) -> Result<Vec<AnnotationRef>> {
        let mut by_class: Vec<(String, &Annotation)> = Vec::new();
        for a in anns {
            if a.visibility == Visibility::System {
                continue;
            }
            let cls = self.type_descriptor(a.annotation.type_idx)?;
            match by_class.iter_mut().find(|(c, _)| *c == cls) {
                Some(slot) => slot.1 = a,
                None => by_class.push((cls, a)),
            }
        }
        let mut out = Vec::new();
        for (_, a) in by_class {
            self.collect_annotation_refs(&a.annotation, &mut out)?;
        }
        Ok(out)
    }

    fn collect_annotation_refs(
        &self,
        ann: &EncodedAnnotation,
        out: &mut Vec<AnnotationRef>,
    ) -> Result<()> {
        out.push(AnnotationRef::Class(self.type_descriptor(ann.type_idx)?));
        for (_, v) in &ann.elements {
            self.collect_value_refs(v, out)?;
        }
        Ok(())
    }

    fn collect_value_refs(&self, v: &EncodedValue, out: &mut Vec<AnnotationRef>) -> Result<()> {
        match v {
            EncodedValue::Type(t) => out.push(AnnotationRef::Class(self.type_descriptor(*t)?)),
            EncodedValue::Field(f) | EncodedValue::Enum(f) => {
                let r = self.field_ref(*f)?;
                out.push(AnnotationRef::Field(FieldRefData {
                    parent_class: r.class,
                    name: r.name,
                    ty: r.ty,
                }));
            }
            EncodedValue::Array(items) => {
                for i in items {
                    self.collect_value_refs(i, out)?;
                }
            }
            EncodedValue::Annotation(a) => self.collect_annotation_refs(a, out)?,
            _ => {}
        }
        Ok(())
    }

    fn const_value_of(&self, v: &EncodedValue) -> Result<Option<ConstValue>> {
        Ok(Some(match v {
            EncodedValue::Boolean(b) => ConstValue::Boolean(*b),
            EncodedValue::Byte(x) => ConstValue::Byte(*x),
            EncodedValue::Short(x) => ConstValue::Short(*x),
            EncodedValue::Char(x) => ConstValue::Char(*x),
            EncodedValue::Int(x) => ConstValue::Int(*x),
            EncodedValue::Long(x) => ConstValue::Long(*x),
            EncodedValue::Float(x) => ConstValue::from_f32(*x),
            EncodedValue::Double(x) => ConstValue::from_f64(*x),
            EncodedValue::String(i) => ConstValue::Str(self.string_utf16(*i)?),
            // a class value is stored as its descriptor string
            EncodedValue::Type(t) => {
                ConstValue::Str(self.type_descriptor(*t)?.encode_utf16().collect())
            }
            _ => return Ok(None),
        }))
    }

    fn value_data(&self, v: &EncodedValue) -> Result<EncodedValueData> {
        Ok(match v {
            EncodedValue::Byte(x) => EncodedValueData::Byte(*x),
            EncodedValue::Short(x) => EncodedValueData::Short(*x),
            EncodedValue::Char(x) => EncodedValueData::Char(*x),
            EncodedValue::Int(x) => EncodedValueData::Int(*x),
            EncodedValue::Long(x) => EncodedValueData::Long(*x),
            EncodedValue::Float(x) => EncodedValueData::Float(x.to_bits()),
            EncodedValue::Double(x) => EncodedValueData::Double(x.to_bits()),
            EncodedValue::String(i) => EncodedValueData::Str(self.string_utf16(*i)?),
            EncodedValue::Type(t) => EncodedValueData::Type(self.type_descriptor(*t)?),
            EncodedValue::Field(f) | EncodedValue::Enum(f) => {
                let r = self.field_ref(*f)?;
                let fr = FieldRefData {
                    parent_class: r.class,
                    name: r.name,
                    ty: r.ty,
                };
                if matches!(v, EncodedValue::Enum(_)) {
                    EncodedValueData::Enum(fr)
                } else {
                    EncodedValueData::Field(fr)
                }
            }
            EncodedValue::Method(_) => EncodedValueData::Method,
            EncodedValue::MethodType(_) => EncodedValueData::Unsupported("METHOD_TYPE".into()),
            EncodedValue::MethodHandle(_) => EncodedValueData::Unsupported("METHOD_HANDLE".into()),
            EncodedValue::Array(items) => EncodedValueData::Array(
                items
                    .iter()
                    .map(|i| self.value_data(i))
                    .collect::<Result<_>>()?,
            ),
            EncodedValue::Annotation(a) => EncodedValueData::Annotation(
                self.annotation_data(a, AnnotationVisibility::Runtime)?,
            ),
            EncodedValue::Null => EncodedValueData::Null,
            EncodedValue::Boolean(b) => EncodedValueData::Boolean(*b),
        })
    }

    fn annotation_data(
        &self,
        a: &EncodedAnnotation,
        visibility: AnnotationVisibility,
    ) -> Result<AnnotationData> {
        let mut values = Vec::with_capacity(a.elements.len());
        for (name_idx, v) in &a.elements {
            values.push((self.string(*name_idx)?, self.value_data(v)?));
        }
        Ok(AnnotationData {
            type_desc: self.type_descriptor(a.type_idx)?,
            visibility,
            values,
        })
    }

    fn annotation_list_data(&self, anns: &[Annotation]) -> Result<Vec<AnnotationData>> {
        let mut out = Vec::with_capacity(anns.len());
        for a in anns {
            let vis = match a.visibility {
                Visibility::Build => AnnotationVisibility::Build,
                Visibility::Runtime => AnnotationVisibility::Runtime,
                Visibility::System => AnnotationVisibility::System,
            };
            out.push(self.annotation_data(&a.annotation, vis)?);
        }
        Ok(out)
    }

    /// Declared exceptions (`dalvik.annotation.Throws`).
    fn throws_of(&self, anns: &[Annotation]) -> Result<Vec<String>> {
        for a in anns {
            if a.visibility != Visibility::System
                || self.type_descriptor(a.annotation.type_idx)? != "Ldalvik/annotation/Throws;"
            {
                continue;
            }
            for (name_idx, value) in &a.annotation.elements {
                if self.string(*name_idx)? != "value" {
                    continue;
                }
                if let EncodedValue::Array(parts) = value {
                    let mut out = Vec::new();
                    for p in parts {
                        if let EncodedValue::Type(t) = p {
                            out.push(self.type_descriptor(*t)?);
                        }
                    }
                    return Ok(out);
                }
            }
        }
        Ok(Vec::new())
    }

    /// Parameter names and flags (`dalvik.annotation.MethodParameters`).
    fn method_parameters_of(&self, anns: &[Annotation]) -> Result<Vec<(Option<String>, u32)>> {
        for a in anns {
            if a.visibility != Visibility::System
                || self.type_descriptor(a.annotation.type_idx)?
                    != "Ldalvik/annotation/MethodParameters;"
            {
                continue;
            }
            let mut names: Vec<Option<String>> = Vec::new();
            let mut flags: Vec<u32> = Vec::new();
            for (name_idx, value) in &a.annotation.elements {
                let EncodedValue::Array(items) = value else {
                    continue;
                };
                match self.string(*name_idx)?.as_str() {
                    "names" => {
                        for i in items {
                            names.push(match i {
                                EncodedValue::String(s) => Some(self.string(*s)?),
                                _ => None,
                            });
                        }
                    }
                    "accessFlags" => {
                        for i in items {
                            if let EncodedValue::Int(v) = i {
                                flags.push(*v as u32);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !names.is_empty() && names.len() == flags.len() {
                return Ok(names.into_iter().zip(flags).collect());
            }
        }
        Ok(Vec::new())
    }

    /// Default values of the annotation methods (`dalvik.annotation.AnnotationDefault`).
    fn annotation_defaults_of(
        &self,
        anns: &[Annotation],
    ) -> Result<Vec<(String, EncodedValueData)>> {
        for a in anns {
            if a.visibility != Visibility::System
                || self.type_descriptor(a.annotation.type_idx)?
                    != "Ldalvik/annotation/AnnotationDefault;"
            {
                continue;
            }
            for (name_idx, value) in &a.annotation.elements {
                if self.string(*name_idx)? != "value" {
                    continue;
                }
                if let EncodedValue::Annotation(def) = value {
                    let data = self.annotation_data(def, AnnotationVisibility::Runtime)?;
                    return Ok(data.values);
                }
            }
        }
        Ok(Vec::new())
    }

    /// Class members in the form handed to the decompiler core (no code, only locations).
    pub fn class_api(&self, def: &ClassDef) -> Result<jadx_input_api::ClassData> {
        use jadx_input_api::{
            ClassAttrs, ClassData, FieldAttrs, FieldData, MethodAttrs, MethodData,
        };
        let anns = self.class_annotations(def)?;
        let data = self.class_data(def)?.unwrap_or_default();
        let static_values = self.static_values(def).unwrap_or_default();
        let mut fields = Vec::new();
        for (i, f) in data
            .static_fields
            .iter()
            .chain(&data.instance_fields)
            .enumerate()
        {
            let fr = self.field_ref(f.field_idx)?;
            let signature = match anns.fields.get(&f.field_idx) {
                Some(l) => self.signature_of(l)?,
                None => None,
            };
            let const_value = match static_values
                .get(i)
                .filter(|_| i < data.static_fields.len())
            {
                Some(v) => self.const_value_of(v)?,
                None => None,
            };
            let annotation_refs = match anns.fields.get(&f.field_idx) {
                Some(l) => self.annotation_refs_of(l)?,
                None => Vec::new(),
            };
            let attrs = FieldAttrs {
                annotations: match anns.fields.get(&f.field_idx) {
                    Some(l) => pack_annotations(self.annotation_list_data(l)?),
                    None => Vec::new(),
                },
            };
            fields.push(FieldData {
                attrs,
                name: fr.name,
                ty: fr.ty,
                access_flags: f.access_flags,
                signature,
                const_value,
                annotation_refs,
            });
        }
        let defaults = self.annotation_defaults_of(&anns.class)?;
        let mut methods = Vec::new();
        for m in data.direct_methods.iter().chain(&data.virtual_methods) {
            let mr = self.method_ref(m.method_idx)?;
            let signature = match anns.methods.get(&m.method_idx) {
                Some(l) => self.signature_of(l)?,
                None => None,
            };
            let mut annotation_refs = match anns.methods.get(&m.method_idx) {
                Some(l) => self.annotation_refs_of(l)?,
                None => Vec::new(),
            };
            for params in anns.parameters.get(&m.method_idx).into_iter().flatten() {
                annotation_refs.extend(self.annotation_refs_of(params)?);
            }
            let own = anns
                .methods
                .get(&m.method_idx)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let mut param_annotations = Vec::new();
            for params in anns.parameters.get(&m.method_idx).into_iter().flatten() {
                param_annotations.push(pack_annotations(self.annotation_list_data(params)?));
            }
            let annotation_default = defaults
                .iter()
                .find(|(n, _)| *n == mr.name)
                .map(|(_, v)| v.clone());
            let attrs = MethodAttrs {
                annotations: pack_annotations(self.annotation_list_data(own)?),
                param_annotations,
                throws: self.throws_of(own)?,
                annotation_default,
                method_parameters: self.method_parameters_of(own)?,
            };
            methods.push(MethodData {
                attrs,
                name: mr.name,
                arg_types: mr.arg_types,
                return_type: mr.return_type,
                access_flags: m.access_flags,
                signature,
                code_offset: (m.code_off != 0).then_some(m.code_off),
                annotation_refs,
            });
        }
        Ok(ClassData {
            type_desc: self.type_descriptor(def.class_idx)?,
            access_flags: def.access_flags,
            super_type: self.opt_type_descriptor(def.superclass_idx)?,
            interfaces: self.type_list(def.interfaces_off)?,
            signature: self.signature_of(&anns.class)?,
            inner_class: self.inner_class_of(&anns.class)?,
            annotation_refs: self.annotation_refs_of(&anns.class)?,
            fields,
            methods,
            attrs: ClassAttrs {
                source_file: self
                    .opt_string(def.source_file_idx)?
                    .filter(|s| !s.is_empty()),
                input_file_name: Some(self.name().to_string()),
                annotations: pack_annotations(self.annotation_list_data(&anns.class)?),
            },
        })
    }
}
