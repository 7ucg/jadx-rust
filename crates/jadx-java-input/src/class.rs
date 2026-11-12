//! Class file structure: constant pool access, members and class level attributes.

use jadx_input_api::{FieldRefData, MethodProtoData, MethodRefData};

use crate::attrs::{parse_attrs, AttrFilter, Attrs};
use crate::error::{JavaError, Result};
use crate::mutf8;
use crate::reader::Reader;
use crate::types::{
    Annotation, CallSite, EncodedValue, InnerClassInfo, JavaField, JavaMethod, MethodHandle,
    MethodHandleKind, MethodHandleRef,
};

pub(crate) const TAG_UTF8: u8 = 1;
pub(crate) const TAG_INTEGER: u8 = 3;
pub(crate) const TAG_FLOAT: u8 = 4;
pub(crate) const TAG_LONG: u8 = 5;
pub(crate) const TAG_DOUBLE: u8 = 6;
pub(crate) const TAG_CLASS: u8 = 7;
pub(crate) const TAG_STRING: u8 = 8;
pub(crate) const TAG_FIELD_REF: u8 = 9;
pub(crate) const TAG_METHOD_REF: u8 = 10;
pub(crate) const TAG_INTERFACE_METHOD_REF: u8 = 11;
pub(crate) const TAG_NAME_AND_TYPE: u8 = 12;
pub(crate) const TAG_METHOD_HANDLE: u8 = 15;
pub(crate) const TAG_METHOD_TYPE: u8 = 16;
pub(crate) const TAG_DYNAMIC: u8 = 17;
pub(crate) const TAG_INVOKE_DYNAMIC: u8 = 18;
pub(crate) const TAG_MODULE: u8 = 19;
pub(crate) const TAG_PACKAGE: u8 = 20;

/// Flag added to constructors (class files do not carry it).
pub const ACC_CONSTRUCTOR: u32 = 0x10000;

/// Payload size of a fixed size constant pool entry (`None` for `Utf8`).
fn tag_data_size(tag: u8) -> Option<Option<usize>> {
    Some(match tag {
        TAG_UTF8 => None,
        TAG_INTEGER | TAG_FLOAT => Some(4),
        TAG_LONG | TAG_DOUBLE => Some(8),
        TAG_CLASS | TAG_STRING | TAG_METHOD_TYPE | TAG_MODULE | TAG_PACKAGE => Some(2),
        TAG_FIELD_REF
        | TAG_METHOD_REF
        | TAG_INTERFACE_METHOD_REF
        | TAG_NAME_AND_TYPE
        | TAG_DYNAMIC
        | TAG_INVOKE_DYNAMIC => Some(4),
        TAG_METHOD_HANDLE => Some(3),
        _ => return None,
    })
}

/// A parsed `.class` file. Members and attributes are read on demand from the raw bytes.
#[derive(Debug)]
pub struct JavaClass {
    source: String,
    data: Vec<u8>,
    /// Data offset of every constant pool entry (just after its tag), `0` for unused slots.
    cp_offsets: Vec<u32>,
    cp_end: usize,
    fields_off: usize,
    methods_off: usize,
    attrs_off: usize,
}

impl JavaClass {
    /// Parse the structure of a class file. `source` is the input file name (for jar entries
    /// `archive.jar:path/Name.class`).
    pub fn parse(source: impl Into<String>, data: Vec<u8>) -> Result<JavaClass> {
        let source = source.into();
        let mut r = Reader::new(&data, 8);
        let cp_size = r.u2()? as usize;
        let mut cp_offsets = vec![0u32; cp_size + 1];
        let mut i = 1usize;
        while i < cp_size {
            let tag = r.u1()? as u8;
            cp_offsets[i] = r.pos() as u32;
            match tag_data_size(tag) {
                None => {
                    return Err(JavaError::format(format!(
                        "Unknown constant pool tag: {tag}"
                    )))
                }
                Some(None) => {
                    let len = r.u2()? as usize;
                    r.skip(len);
                }
                Some(Some(size)) => {
                    r.skip(size);
                    if tag == TAG_LONG || tag == TAG_DOUBLE {
                        i += 1;
                    }
                }
            }
            i += 1;
        }
        let cp_end = r.pos();
        r.set_pos(cp_end + 6);
        let interfaces = r.u2()? as usize;
        r.skip(interfaces * 2);
        let fields_off = r.pos();
        skip_members(&mut r)?;
        let methods_off = r.pos();
        skip_members(&mut r)?;
        let attrs_off = r.pos();
        // make sure the class level attributes are inside the file
        let mut ar = Reader::new(&data, attrs_off);
        let n = ar.u2()?;
        for _ in 0..n {
            ar.skip(2);
            let len = ar.u4()? as usize;
            ar.skip(len);
        }
        if ar.pos() > data.len() {
            return Err(JavaError::Truncated {
                offset: ar.pos(),
                wanted: 0,
                size: data.len(),
            });
        }
        Ok(JavaClass {
            source,
            data,
            cp_offsets,
            cp_end,
            fields_off,
            methods_off,
            attrs_off,
        })
    }

    /// Input file name.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Raw class file bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub(crate) fn reader_at(&self, pos: usize) -> Reader<'_> {
        Reader::new(&self.data, pos)
    }

    pub fn minor_version(&self) -> Result<u32> {
        self.reader_at(4).u2()
    }

    pub fn major_version(&self) -> Result<u32> {
        self.reader_at(6).u2()
    }

    // ---- constant pool ----

    pub fn constant_pool_count(&self) -> usize {
        self.cp_offsets.len() - 1
    }

    /// Data offset of entry `idx` (just after the tag).
    fn entry(&self, idx: u32) -> Result<usize> {
        match self.cp_offsets.get(idx as usize) {
            Some(off) if *off != 0 => Ok(*off as usize),
            _ => Err(JavaError::format(format!(
                "invalid constant pool index: {idx}"
            ))),
        }
    }

    /// Reader positioned at the data of constant pool entry `idx` (just after its tag).
    pub(crate) fn entry_reader(&self, idx: u32) -> Result<Reader<'_>> {
        Ok(self.reader_at(self.entry(idx)?))
    }

    pub(crate) fn tag(&self, idx: u32) -> Result<u8> {
        let off = self.entry(idx)?;
        Ok(self.data[off - 1])
    }

    fn entry_checked(&self, idx: u32, tags: &[u8]) -> Result<usize> {
        let off = self.entry(idx)?;
        let tag = self.data[off - 1];
        if tags.contains(&tag) {
            Ok(off)
        } else {
            Err(JavaError::format(format!(
                "unexpected constant pool tag {tag} at index {idx}"
            )))
        }
    }

    /// `Utf8` entry as UTF-16 code units.
    pub fn utf8_units(&self, idx: u32) -> Result<Vec<u16>> {
        let off = self.entry_checked(idx, &[TAG_UTF8])?;
        let mut r = self.reader_at(off);
        let len = r.u2()? as usize;
        mutf8::decode(r.bytes(len)?)
    }

    /// `Utf8` entry, unpaired surrogates are replaced.
    pub fn utf8(&self, idx: u32) -> Result<String> {
        Ok(String::from_utf16_lossy(&self.utf8_units(idx)?))
    }

    /// `Utf8` entry where index `0` stands for no value.
    pub fn opt_utf8(&self, idx: u32) -> Result<Option<String>> {
        if idx == 0 {
            Ok(None)
        } else {
            self.utf8(idx).map(Some)
        }
    }

    /// Class entry as type descriptor: array descriptors are kept, internal names are wrapped
    /// into `L...;`.
    pub fn class_name(&self, idx: u32) -> Result<String> {
        let off = self.entry_checked(idx, &[TAG_CLASS])?;
        let name_idx = self.reader_at(off).u2()?;
        fix_type(self.utf8(name_idx)?)
    }

    pub fn int_value(&self, idx: u32) -> Result<i32> {
        let off = self.entry_checked(idx, &[TAG_INTEGER])?;
        self.reader_at(off).s4()
    }

    pub fn long_value(&self, idx: u32) -> Result<i64> {
        let off = self.entry_checked(idx, &[TAG_LONG])?;
        Ok(self.reader_at(off).u8()? as i64)
    }

    pub fn float_value(&self, idx: u32) -> Result<f32> {
        let off = self.entry_checked(idx, &[TAG_FLOAT])?;
        Ok(f32::from_bits(self.reader_at(off).u4()?))
    }

    pub fn double_value(&self, idx: u32) -> Result<f64> {
        let off = self.entry_checked(idx, &[TAG_DOUBLE])?;
        Ok(f64::from_bits(self.reader_at(off).u8()?))
    }

    fn name_and_type(&self, idx: u32) -> Result<(String, String)> {
        let off = self.entry_checked(idx, &[TAG_NAME_AND_TYPE])?;
        let mut r = self.reader_at(off);
        let name = r.u2()?;
        let desc = r.u2()?;
        Ok((self.utf8(name)?, self.utf8(desc)?))
    }

    pub fn field_ref(&self, idx: u32) -> Result<FieldRefData> {
        let off = self.entry_checked(idx, &[TAG_FIELD_REF])?;
        let mut r = self.reader_at(off);
        let cls = r.u2()?;
        let nt = r.u2()?;
        let (name, ty) = self.name_and_type(nt)?;
        Ok(FieldRefData {
            parent_class: self.class_name(cls)?,
            name,
            ty,
        })
    }

    /// Type descriptor of a field reference.
    pub fn field_ref_type(&self, idx: u32) -> Result<String> {
        let off = self.entry_checked(idx, &[TAG_FIELD_REF])?;
        let mut r = self.reader_at(off);
        r.skip(2);
        let nt = r.u2()?;
        Ok(self.name_and_type(nt)?.1)
    }

    pub fn method_ref(&self, idx: u32) -> Result<MethodRefData> {
        let off = self.entry_checked(idx, &[TAG_METHOD_REF, TAG_INTERFACE_METHOD_REF])?;
        let mut r = self.reader_at(off);
        let cls = r.u2()?;
        let nt = r.u2()?;
        let (name, desc) = self.name_and_type(nt)?;
        let proto = parse_method_descriptor(&desc)?;
        Ok(MethodRefData {
            parent_class: self.class_name(cls)?,
            name,
            arg_types: proto.arg_types,
            return_type: proto.return_type,
        })
    }

    pub(crate) fn method_handle(&self, idx: u32) -> Result<MethodHandle> {
        let off = self.entry_checked(idx, &[TAG_METHOD_HANDLE])?;
        let mut r = self.reader_at(off);
        let kind = r.u1()?;
        let ref_idx = r.u2()?;
        let kind = match kind {
            1 => MethodHandleKind::StaticPut,
            2 => MethodHandleKind::StaticGet,
            3 => MethodHandleKind::InstancePut,
            4 => MethodHandleKind::InstanceGet,
            5 => MethodHandleKind::InvokeInstance,
            6 => MethodHandleKind::InvokeStatic,
            7 => MethodHandleKind::InvokeDirect,
            8 => MethodHandleKind::InvokeConstructor,
            9 => MethodHandleKind::InvokeInterface,
            k => {
                return Err(JavaError::format(format!(
                    "Unknown method handle type: {k}"
                )))
            }
        };
        let target = if kind.is_field() {
            MethodHandleRef::Field(self.field_ref(ref_idx)?)
        } else {
            MethodHandleRef::Method(self.method_ref(ref_idx)?)
        };
        Ok(MethodHandle { kind, target })
    }

    /// Constant as an encoded value (static bootstrap arguments and `ConstantValue`).
    pub fn constant_as_value(&self, idx: u32) -> Result<EncodedValue> {
        let tag = self.tag(idx)?;
        let off = self.entry(idx)?;
        Ok(match tag {
            TAG_UTF8 => EncodedValue::String(self.utf8_units(idx)?),
            TAG_STRING => {
                let s = self.reader_at(off).u2()?;
                EncodedValue::String(self.utf8_units(s)?)
            }
            TAG_INTEGER => EncodedValue::Int(self.int_value(idx)?),
            TAG_FLOAT => EncodedValue::Float(self.float_value(idx)?),
            TAG_LONG => EncodedValue::Long(self.long_value(idx)?),
            TAG_DOUBLE => EncodedValue::Double(self.double_value(idx)?),
            TAG_CLASS => EncodedValue::Type(self.class_name(idx)?),
            TAG_METHOD_TYPE => {
                let d = self.reader_at(off).u2()?;
                EncodedValue::MethodType(parse_method_descriptor(&self.utf8(d)?)?)
            }
            TAG_METHOD_HANDLE => EncodedValue::MethodHandle(self.method_handle(idx)?),
            t => {
                return Err(JavaError::format(format!(
                    "Can't encode constant with tag {t} as encoded value"
                )))
            }
        })
    }

    /// Dynamic call site of an `invokedynamic` constant pool entry.
    pub fn call_site(&self, idx: u32) -> Result<CallSite> {
        let tag = self.tag(idx)?;
        let off = self.entry(idx)?;
        match tag {
            TAG_INVOKE_DYNAMIC => {}
            TAG_DYNAMIC => {
                return Err(JavaError::format("Field call site not yet implemented"));
            }
            t => {
                return Err(JavaError::format(format!(
                    "Unexpected tag type for call site: {t}"
                )))
            }
        }
        let mut r = self.reader_at(off);
        let bootstrap_idx = r.u2()? as usize;
        let nt = r.u2()?;
        let (name, desc) = self.name_and_type(nt)?;
        let mut ar = self.reader_at(self.attrs_off);
        let attrs = parse_attrs(self, &mut ar, AttrFilter::Only(&["BootstrapMethods"]))?;
        let bootstrap = attrs
            .bootstrap_methods
            .as_ref()
            .ok_or_else(|| JavaError::format("Unexpected missing BootstrapMethods attribute"))?;
        let bm = bootstrap
            .get(bootstrap_idx)
            .ok_or_else(|| JavaError::format("bootstrap method index out of range"))?;
        let mut values = Vec::with_capacity(3 + bm.args.len());
        values.push(EncodedValue::MethodHandle(
            self.method_handle(bm.method_handle)?,
        ));
        values.push(EncodedValue::String(name.encode_utf16().collect()));
        values.push(EncodedValue::MethodType(parse_method_descriptor(&desc)?));
        for arg in &bm.args {
            values.push(self.constant_as_value(*arg)?);
        }
        Ok(CallSite { values })
    }

    // ---- class level data ----

    pub fn access_flags(&self) -> Result<u32> {
        self.reader_at(self.cp_end).u2()
    }

    /// Offset of the access flags (start of the class header after the constant pool).
    pub fn header_offset(&self) -> u32 {
        self.cp_end as u32
    }

    /// Class descriptor (`Lpkg/Name;`).
    pub fn type_desc(&self) -> Result<String> {
        let idx = self.reader_at(self.cp_end + 2).u2()?;
        self.class_name(idx)
    }

    pub fn super_type(&self) -> Result<Option<String>> {
        let idx = self.reader_at(self.cp_end + 4).u2()?;
        if idx == 0 {
            return Ok(None);
        }
        self.class_name(idx).map(Some)
    }

    pub fn interfaces(&self) -> Result<Vec<String>> {
        let mut r = self.reader_at(self.cp_end + 6);
        let n = r.u2()?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            out.push(self.class_name(r.u2()?)?);
        }
        Ok(out)
    }

    /// Attributes of the class.
    pub(crate) fn class_attrs(&self) -> Result<Attrs> {
        let mut r = self.reader_at(self.attrs_off);
        parse_attrs(self, &mut r, AttrFilter::All)
    }

    pub fn signature(&self) -> Result<Option<String>> {
        Ok(self.class_attrs()?.signature)
    }

    pub fn source_file(&self) -> Result<Option<String>> {
        Ok(self.class_attrs()?.source_file)
    }

    /// Runtime annotations followed by build annotations.
    pub fn annotations(&self) -> Result<Vec<Annotation>> {
        Ok(self.class_attrs()?.annotations())
    }

    /// Entries of the `InnerClasses` attribute (one per inner class, the last one wins).
    pub fn inner_classes(&self) -> Result<Vec<InnerClassInfo>> {
        Ok(self.class_attrs()?.inner_classes.unwrap_or_default())
    }

    pub fn fields(&self) -> Result<Vec<JavaField>> {
        let mut r = self.reader_at(self.fields_off);
        let n = r.u2()?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let access_flags = r.u2()?;
            let name = r.u2()?;
            let desc = r.u2()?;
            let attrs = parse_attrs(self, &mut r, AttrFilter::All)?;
            out.push(JavaField {
                name: self.utf8(name)?,
                descriptor: self.utf8(desc)?,
                access_flags,
                signature: attrs.signature.clone(),
                const_value: attrs.const_value.clone(),
                annotations: attrs.annotations(),
            });
        }
        Ok(out)
    }

    pub fn methods(&self) -> Result<Vec<JavaMethod>> {
        let mut r = self.reader_at(self.methods_off);
        let n = r.u2()?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let mut access_flags = r.u2()?;
            let name = r.u2()?;
            let desc = r.u2()?;
            let attrs = parse_attrs(self, &mut r, AttrFilter::All)?;
            let name = self.utf8(name)?;
            if name == "<init>" {
                access_flags |= ACC_CONSTRUCTOR;
            }
            out.push(JavaMethod {
                name,
                descriptor: self.utf8(desc)?,
                access_flags,
                code_offset: attrs.code,
                signature: attrs.signature.clone(),
                exceptions: attrs.exceptions.clone(),
                parameters: attrs.method_params.clone(),
                annotation_default: attrs.annotation_default.clone(),
                annotations: attrs.annotations(),
                param_annotations: attrs.param_annotations(),
            });
        }
        Ok(out)
    }
}

fn skip_members(r: &mut Reader<'_>) -> Result<()> {
    let n = r.u2()?;
    for _ in 0..n {
        r.skip(6);
        let attrs = r.u2()?;
        for _ in 0..attrs {
            r.skip(2);
            let len = r.u4()? as usize;
            r.skip(len);
        }
    }
    Ok(())
}

fn fix_type(name: String) -> Result<String> {
    match name.chars().next() {
        None => Err(JavaError::format("empty class name")),
        Some('[') => Ok(name),
        Some('L') | Some('T') if name.ends_with(';') => Ok(name),
        _ => Ok(format!("L{name};")),
    }
}

/// Split a method descriptor into argument and return types.
pub fn parse_method_descriptor(desc: &str) -> Result<MethodProtoData> {
    let b = desc.as_bytes();
    let bad = || JavaError::format(format!("Unexpected char in descriptor: {desc}"));
    if b.first() != Some(&b'(') {
        return Err(bad());
    }
    let mut pos = 1usize;
    let mut args = Vec::new();
    loop {
        match b.get(pos) {
            None => return Err(bad()),
            Some(b')') => break,
            _ => args.push(read_type(desc, &mut pos)?),
        }
    }
    pos += 1;
    let ret = read_type(desc, &mut pos)?;
    Ok(MethodProtoData {
        arg_types: args,
        return_type: ret,
    })
}

fn read_type(desc: &str, pos: &mut usize) -> Result<String> {
    let b = desc.as_bytes();
    let cur = *pos;
    match b.get(cur) {
        None => Err(JavaError::format(format!(
            "Unexpected end of descriptor: {desc}"
        ))),
        Some(b'L') => {
            let end = desc[cur..]
                .find(';')
                .ok_or_else(|| JavaError::format(format!("Unexpected object type: {desc}")))?
                + cur
                + 1;
            *pos = end;
            Ok(desc[cur..end].to_string())
        }
        Some(b'[') => {
            *pos += 1;
            Ok(format!("[{}", read_type(desc, pos)?))
        }
        Some(c) if b"ZBCSIJFDV".contains(c) => {
            *pos += 1;
            Ok((*c as char).to_string())
        }
        Some(c) => Err(JavaError::format(format!(
            "Unexpected char '{}' in descriptor {desc}",
            *c as char
        ))),
    }
}
