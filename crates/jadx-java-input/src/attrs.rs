//! Attribute parsing. Attributes that fail to parse are skipped, like the reference reader does.

use jadx_input_api::FieldRefData;

use crate::class::JavaClass;
use crate::error::{JavaError, Result};
use crate::reader::Reader;
use crate::types::{Annotation, AnnotationVisibility, EncodedValue, InnerClassInfo, MethodParam};

/// Which attributes of a list are parsed.
#[derive(Clone, Copy)]
pub(crate) enum AttrFilter<'a> {
    All,
    Only(&'a [&'a str]),
}

impl AttrFilter<'_> {
    fn accepts(&self, name: &str) -> bool {
        match self {
            AttrFilter::All => true,
            AttrFilter::Only(names) => names.contains(&name),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BootstrapMethod {
    pub method_handle: u32,
    pub args: Vec<u32>,
}

/// Stack slot kind as tracked by the code decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Slot that was never written (a `null` entry of the reference stack).
    Unset,
    Narrow,
    Wide,
}

#[derive(Debug, Clone)]
pub(crate) struct StackFrame {
    pub offset: u32,
    pub stack: Vec<Slot>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RawLocalVar {
    pub start: u32,
    pub end: u32,
    pub name: Option<String>,
    pub ty: Option<String>,
    pub reg: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Attrs {
    pub code: Option<u32>,
    pub const_value: Option<EncodedValue>,
    pub signature: Option<String>,
    pub source_file: Option<String>,
    pub exceptions: Option<Vec<String>>,
    pub method_params: Option<Vec<MethodParam>>,
    pub annotation_default: Option<EncodedValue>,
    pub inner_classes: Option<Vec<InnerClassInfo>>,
    pub bootstrap_methods: Option<Vec<BootstrapMethod>>,
    pub runtime_annotations: Option<Vec<Annotation>>,
    pub build_annotations: Option<Vec<Annotation>>,
    pub runtime_param_annotations: Option<Vec<Vec<Annotation>>>,
    pub build_param_annotations: Option<Vec<Vec<Annotation>>>,
    pub stack_map: Option<Vec<StackFrame>>,
    pub line_numbers: Option<Vec<(u32, u32)>>,
    pub local_vars: Option<Vec<RawLocalVar>>,
    pub local_var_types: Option<Vec<RawLocalVar>>,
}

impl Attrs {
    /// Runtime annotations followed by build annotations.
    pub fn annotations(&self) -> Vec<Annotation> {
        let mut out = self.runtime_annotations.clone().unwrap_or_default();
        out.extend(self.build_annotations.iter().flatten().cloned());
        out
    }

    /// Parameter annotations of both visibilities, merged per parameter.
    pub fn param_annotations(&self) -> Vec<Vec<Annotation>> {
        match (
            &self.runtime_param_annotations,
            &self.build_param_annotations,
        ) {
            (None, None) => Vec::new(),
            (Some(a), None) | (None, Some(a)) => a.clone(),
            (Some(a), Some(b)) => {
                let n = a.len().max(b.len());
                (0..n)
                    .map(|i| {
                        let mut v = a.get(i).cloned().unwrap_or_default();
                        v.extend(b.get(i).cloned().unwrap_or_default());
                        v
                    })
                    .collect()
            }
        }
    }
}

/// Parse an attribute list at the reader position; the reader ends after the list.
pub(crate) fn parse_attrs(
    cls: &JavaClass,
    r: &mut Reader<'_>,
    filter: AttrFilter<'_>,
) -> Result<Attrs> {
    let mut attrs = Attrs::default();
    let count = r.u2()?;
    for _ in 0..count {
        let name_idx = r.u2()?;
        let len = r.u4()? as usize;
        let start = r.pos();
        let end = start.saturating_add(len);
        // an attribute that fails to parse is ignored
        if let Ok(name) = cls.utf8(name_idx) {
            if filter.accepts(&name) {
                let mut ar = r.clone();
                let _ = parse_one(cls, &mut ar, &name, &mut attrs);
            }
        }
        r.set_pos(end);
    }
    Ok(attrs)
}

fn parse_one(cls: &JavaClass, r: &mut Reader<'_>, name: &str, attrs: &mut Attrs) -> Result<()> {
    match name {
        "ConstantValue" => {
            let idx = r.u2()?;
            attrs.const_value = Some(cls.constant_as_value(idx)?);
        }
        "Code" => attrs.code = Some(r.pos() as u32),
        "LineNumberTable" => {
            let n = r.u2()?;
            let mut lines: Vec<(u32, u32)> = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let off = r.u2()?;
                let line = r.u2()?;
                match lines.iter_mut().find(|(o, _)| *o == off) {
                    Some(slot) => slot.1 = line,
                    None => lines.push((off, line)),
                }
            }
            lines.sort_unstable();
            attrs.line_numbers = Some(lines);
        }
        "LocalVariableTable" => attrs.local_vars = Some(read_local_vars(cls, r)?),
        "LocalVariableTypeTable" => attrs.local_var_types = Some(read_local_vars(cls, r)?),
        "InnerClasses" => {
            let n = r.u2()?;
            let mut list: Vec<InnerClassInfo> = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let inner = cls.class_name(r.u2()?)?;
                let outer_idx = r.u2()?;
                let outer = if outer_idx == 0 {
                    None
                } else {
                    Some(cls.class_name(outer_idx)?)
                };
                let name = cls.opt_utf8(r.u2()?)?;
                let access_flags = r.u2()?;
                let info = InnerClassInfo {
                    inner_class: inner,
                    outer_class: outer,
                    name,
                    access_flags,
                };
                match list.iter_mut().find(|i| i.inner_class == info.inner_class) {
                    Some(slot) => *slot = info,
                    None => list.push(info),
                }
            }
            attrs.inner_classes = Some(list);
        }
        "BootstrapMethods" => {
            let n = r.u2()?;
            let mut list = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let method_handle = r.u2()?;
                let argc = r.u2()?;
                let mut args = Vec::with_capacity(argc as usize);
                for _ in 0..argc {
                    args.push(r.u2()?);
                }
                list.push(BootstrapMethod {
                    method_handle,
                    args,
                });
            }
            attrs.bootstrap_methods = Some(list);
        }
        "RuntimeVisibleAnnotations" => {
            attrs.runtime_annotations =
                Some(read_annotations(cls, r, AnnotationVisibility::Runtime)?)
        }
        "RuntimeInvisibleAnnotations" => {
            attrs.build_annotations = Some(read_annotations(cls, r, AnnotationVisibility::Build)?)
        }
        "RuntimeVisibleParameterAnnotations" => {
            attrs.runtime_param_annotations = Some(read_param_annotations(
                cls,
                r,
                AnnotationVisibility::Runtime,
            )?)
        }
        "RuntimeInvisibleParameterAnnotations" => {
            attrs.build_param_annotations =
                Some(read_param_annotations(cls, r, AnnotationVisibility::Build)?)
        }
        "AnnotationDefault" => attrs.annotation_default = Some(read_encoded_value(cls, r)?),
        "SourceFile" => attrs.source_file = cls.opt_utf8(r.u2()?)?,
        "Signature" => attrs.signature = cls.opt_utf8(r.u2()?)?,
        "Exceptions" => {
            let n = r.u2()?;
            let mut list = Vec::with_capacity(n as usize);
            for _ in 0..n {
                list.push(cls.class_name(r.u2()?)?);
            }
            attrs.exceptions = Some(list);
        }
        "MethodParameters" => {
            let n = r.u1()?;
            let mut list = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let name = cls.opt_utf8(r.u2()?)?;
                let access_flags = r.u2()?;
                list.push(MethodParam { name, access_flags });
            }
            attrs.method_params = Some(list);
        }
        "StackMapTable" => attrs.stack_map = Some(read_stack_map(r)?),
        _ => {}
    }
    Ok(())
}

fn read_local_vars(cls: &JavaClass, r: &mut Reader<'_>) -> Result<Vec<RawLocalVar>> {
    let n = r.u2()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let start = r.u2()?;
        let length = r.u2()?;
        let name = cls.opt_utf8(r.u2()?)?;
        let ty = cls.opt_utf8(r.u2()?)?;
        let reg = r.u2()?;
        out.push(RawLocalVar {
            start,
            end: (start + length).wrapping_sub(1),
            name,
            ty,
            reg,
        });
    }
    Ok(out)
}

fn read_annotations(
    cls: &JavaClass,
    r: &mut Reader<'_>,
    vis: AnnotationVisibility,
) -> Result<Vec<Annotation>> {
    let n = r.u2()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        out.push(read_annotation(cls, r, vis)?);
    }
    Ok(out)
}

fn read_param_annotations(
    cls: &JavaClass,
    r: &mut Reader<'_>,
    vis: AnnotationVisibility,
) -> Result<Vec<Vec<Annotation>>> {
    let n = r.u1()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        out.push(read_annotations(cls, r, vis)?);
    }
    Ok(out)
}

fn read_annotation(
    cls: &JavaClass,
    r: &mut Reader<'_>,
    vis: AnnotationVisibility,
) -> Result<Annotation> {
    let class = cls.utf8(r.u2()?)?;
    let pairs = r.u2()?;
    let mut values: Vec<(String, EncodedValue)> = Vec::with_capacity(pairs as usize);
    for _ in 0..pairs {
        let name = cls.utf8(r.u2()?)?;
        let value = read_encoded_value(cls, r)?;
        match values.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => values.push((name, value)),
        }
    }
    Ok(Annotation {
        visibility: vis,
        class,
        values,
    })
}

const MAX_DEPTH: u32 = 64;

fn read_encoded_value(cls: &JavaClass, r: &mut Reader<'_>) -> Result<EncodedValue> {
    read_encoded_value_depth(cls, r, 0)
}

fn read_encoded_value_depth(
    cls: &JavaClass,
    r: &mut Reader<'_>,
    depth: u32,
) -> Result<EncodedValue> {
    if depth > MAX_DEPTH {
        return Err(JavaError::format("encoded value nesting too deep"));
    }
    let tag = r.u1()? as u8;
    Ok(match tag {
        b'B' => EncodedValue::Byte(cls.int_value(r.u2()?)? as i8),
        b'C' => EncodedValue::Char(cls.int_value(r.u2()?)? as u16),
        b'D' => EncodedValue::Double(cls.double_value(r.u2()?)?),
        b'F' => EncodedValue::Float(cls.float_value(r.u2()?)?),
        b'I' => EncodedValue::Int(cls.int_value(r.u2()?)?),
        b'J' => EncodedValue::Long(cls.long_value(r.u2()?)?),
        b'S' => EncodedValue::Short(cls.int_value(r.u2()?)? as i16),
        b'Z' => EncodedValue::Boolean(cls.int_value(r.u2()?)? == 1),
        b's' => EncodedValue::String(cls.utf8_units(r.u2()?)?),
        b'e' => {
            let class = cls.utf8(r.u2()?)?;
            let name = cls.utf8(r.u2()?)?;
            EncodedValue::Enum(FieldRefData {
                parent_class: class.clone(),
                name,
                ty: class,
            })
        }
        b'c' => EncodedValue::Type(cls.utf8(r.u2()?)?),
        b'@' => EncodedValue::Annotation(read_annotation(cls, r, AnnotationVisibility::Runtime)?),
        b'[' => {
            let n = r.u2()?;
            let mut values = Vec::with_capacity((n as usize).min(1024));
            for _ in 0..n {
                values.push(read_encoded_value_depth(cls, r, depth + 1)?);
            }
            EncodedValue::Array(values)
        }
        t => {
            return Err(JavaError::format(format!(
                "Unknown element value tag: {}",
                t as char
            )))
        }
    })
}

// ---- StackMapTable ----

const ITEM_DOUBLE: u32 = 3;
const ITEM_LONG: u32 = 4;
const ITEM_OBJECT: u32 = 7;
const ITEM_UNINITIALIZED: u32 = 8;

fn read_type_info_list(r: &mut Reader<'_>, count: usize) -> Result<Vec<Slot>> {
    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let tag = r.u1()?;
        out.push(match tag {
            ITEM_DOUBLE | ITEM_LONG => Slot::Wide,
            ITEM_OBJECT | ITEM_UNINITIALIZED => {
                r.u2()?;
                Slot::Narrow
            }
            _ => Slot::Narrow,
        });
    }
    Ok(out)
}

fn skip_type_info_list(r: &mut Reader<'_>, count: usize) -> Result<()> {
    for _ in 0..count {
        let tag = r.u1()?;
        if tag == ITEM_OBJECT || tag == ITEM_UNINITIALIZED {
            r.u2()?;
        }
    }
    Ok(())
}

fn read_stack_map(r: &mut Reader<'_>) -> Result<Vec<StackFrame>> {
    let count = r.u2()?;
    let mut frames: Vec<StackFrame> = Vec::with_capacity(count as usize);
    let mut prev_offset: Option<u32> = None;
    for _ in 0..count {
        let ty = r.u1()?;
        let calc = |delta: u32| match prev_offset {
            None => delta,
            Some(p) => p.wrapping_add(delta).wrapping_add(1),
        };
        let frame = match ty {
            0..=63 => StackFrame {
                offset: calc(ty),
                stack: Vec::new(),
            },
            64..=127 => {
                let stack = read_type_info_list(r, 1)?;
                StackFrame {
                    offset: calc(ty - 64),
                    stack,
                }
            }
            247 => {
                let delta = r.u2()?;
                let stack = read_type_info_list(r, 1)?;
                StackFrame {
                    offset: calc(delta),
                    stack,
                }
            }
            248..=250 | 251 => {
                let delta = r.u2()?;
                StackFrame {
                    offset: calc(delta),
                    stack: Vec::new(),
                }
            }
            252..=254 => {
                let delta = r.u2()?;
                skip_type_info_list(r, (ty - 251) as usize)?;
                StackFrame {
                    offset: calc(delta),
                    stack: Vec::new(),
                }
            }
            255 => {
                let delta = r.u2()?;
                let locals = r.u2()? as usize;
                skip_type_info_list(r, locals)?;
                let stack_size = r.u2()? as usize;
                let stack = read_type_info_list(r, stack_size)?;
                StackFrame {
                    offset: calc(delta),
                    stack,
                }
            }
            t => {
                return Err(JavaError::format(format!(
                    "Found unsupported stack frame type: {t}"
                )))
            }
        };
        prev_offset = Some(frame.offset);
        frames.push(frame);
    }
    Ok(frames)
}
