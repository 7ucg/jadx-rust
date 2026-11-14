//! Instruction data as handed from an input reader to the core.

use crate::Opcode;

/// What the `index` operand of an instruction refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InsnIndexType {
    None,
    TypeRef,
    StringRef,
    FieldRef,
    MethodRef,
    CallSite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchPayload {
    pub keys: Vec<i32>,
    /// Offsets relative to the switch instruction, in code units.
    pub targets: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ArrayData {
    Empty,
    Bytes(Vec<u8>),
    Shorts(Vec<i16>),
    Ints(Vec<i32>),
    Longs(Vec<i64>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayPayload {
    pub size: u32,
    pub element_size: u16,
    pub data: ArrayData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Switch(SwitchPayload),
    Array(ArrayPayload),
}

/// One decoded instruction.
///
/// `literal`, `target` and `index` are only meaningful for instruction formats that carry them
/// (they are zero otherwise). `target` is an absolute offset within the method in code units,
/// except for `invoke-polymorphic` where it holds the proto index.
#[derive(Debug, Clone)]
pub struct InsnData {
    /// Offset within the method, in 16-bit code units.
    pub offset: u32,
    /// Length in code units.
    pub length: u32,
    pub opcode: Opcode,
    /// First code unit as stored in the file.
    pub raw_opcode_unit: u16,
    pub index_type: InsnIndexType,
    pub regs: Vec<u32>,
    pub literal: i64,
    pub target: i64,
    pub index: u32,
    /// Register that receives the result of an invoke instruction without a following
    /// `move-result` (set by stack based readers, `None` for void calls and everything else).
    pub result_reg: Option<u32>,
    pub payload: Option<Payload>,
}

impl InsnData {
    pub fn regs_count(&self) -> usize {
        self.regs.len()
    }
}

/// Reference to a field as stored in the input (`Lpkg/Cls;`, name, `Lpkg/Type;`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRefData {
    pub parent_class: String,
    pub name: String,
    pub ty: String,
}

/// Reference to a method as stored in the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodRefData {
    pub parent_class: String,
    pub name: String,
    pub arg_types: Vec<String>,
    pub return_type: String,
}

/// Prototype (argument and return type descriptors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodProtoData {
    pub arg_types: Vec<String>,
    pub return_type: String,
}

/// Kind of a method handle (`MethodHandleType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodHandleKind {
    StaticPut,
    StaticGet,
    InstancePut,
    InstanceGet,
    InvokeStatic,
    InvokeInstance,
    InvokeConstructor,
    InvokeDirect,
    InvokeInterface,
}

impl MethodHandleKind {
    pub fn is_field(self) -> bool {
        matches!(
            self,
            MethodHandleKind::StaticPut
                | MethodHandleKind::StaticGet
                | MethodHandleKind::InstancePut
                | MethodHandleKind::InstanceGet
        )
    }

    /// Name of the reference enum constant.
    pub fn name(self) -> &'static str {
        match self {
            MethodHandleKind::StaticPut => "STATIC_PUT",
            MethodHandleKind::StaticGet => "STATIC_GET",
            MethodHandleKind::InstancePut => "INSTANCE_PUT",
            MethodHandleKind::InstanceGet => "INSTANCE_GET",
            MethodHandleKind::InvokeStatic => "INVOKE_STATIC",
            MethodHandleKind::InvokeInstance => "INVOKE_INSTANCE",
            MethodHandleKind::InvokeConstructor => "INVOKE_CONSTRUCTOR",
            MethodHandleKind::InvokeDirect => "INVOKE_DIRECT",
            MethodHandleKind::InvokeInterface => "INVOKE_INTERFACE",
        }
    }
}

/// Field or method a method handle refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodHandleTarget {
    Field(FieldRefData),
    Method(MethodRefData),
}

/// A method handle constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodHandleData {
    pub kind: MethodHandleKind,
    pub target: MethodHandleTarget,
}

/// A value of a call site (`invoke-custom` / `invokedynamic`): the bootstrap method handle, the
/// call site name, its prototype and the static bootstrap arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallSiteValue {
    Null,
    Boolean(bool),
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    /// Bits of the `f32` value.
    Float(u32),
    /// Bits of the `f64` value.
    Double(u64),
    /// String as UTF-16 code units.
    Str(Vec<u16>),
    /// Class descriptor.
    Type(String),
    MethodType(MethodProtoData),
    MethodHandle(MethodHandleData),
    /// Value kind that is not used in call sites.
    Other,
}

/// Resolves the index operands of [`InsnData`] against the input's constant tables.
pub trait InsnResolver {
    fn string(&self, idx: u32) -> Result<String, String>;
    /// String as UTF-16 code units (exact, including unpaired surrogates).
    fn string_utf16(&self, idx: u32) -> Result<Vec<u16>, String>;
    /// Type descriptor (`Ljava/lang/String;`, `[I`).
    fn type_desc(&self, idx: u32) -> Result<String, String>;
    fn field(&self, idx: u32) -> Result<FieldRefData, String>;
    fn method(&self, idx: u32) -> Result<MethodRefData, String>;
    fn proto(&self, idx: u32) -> Result<MethodProtoData, String>;
    /// Values of the call site with the given index: `[bootstrap method handle, name, method
    /// type, static arguments...]`.
    fn call_site(&self, _idx: u32) -> Result<Vec<CallSiteValue>, String> {
        Err("call sites are not supported by this input".to_string())
    }
}

/// Field of a class as read from the input.
#[derive(Debug, Clone)]
pub struct FieldData {
    pub name: String,
    /// Type descriptor.
    pub ty: String,
    pub access_flags: u32,
    /// Generic signature (`dalvik.annotation.Signature`), joined to one string.
    pub signature: Option<String>,
    /// Constant initial value of a static field (`None` for the null value and for fields
    /// without initial value).
    pub const_value: Option<ConstValue>,
    /// Classes and fields referenced by the (non system) annotations of the field.
    pub annotation_refs: Vec<AnnotationRef>,
    pub attrs: FieldAttrs,
}

/// Method of a class as read from the input.
#[derive(Debug, Clone)]
pub struct MethodData {
    pub name: String,
    pub arg_types: Vec<String>,
    pub return_type: String,
    pub access_flags: u32,
    pub signature: Option<String>,
    /// Input specific location of the code, `None` for abstract/native methods.
    pub code_offset: Option<u32>,
    /// Classes and fields referenced by the (non system) annotations of the method and of its
    /// parameters.
    pub annotation_refs: Vec<AnnotationRef>,
    pub attrs: MethodAttrs,
}

/// Class as read from the input (members only, code is loaded on demand).
#[derive(Debug, Clone)]
pub struct ClassData {
    pub type_desc: String,
    pub access_flags: u32,
    pub super_type: Option<String>,
    pub interfaces: Vec<String>,
    pub signature: Option<String>,
    /// `InnerClass` annotation data (access flags and enclosing class of a nested class).
    pub inner_class: Option<InnerClassData>,
    /// Classes and fields referenced by the (non system) annotations of the class.
    pub annotation_refs: Vec<AnnotationRef>,
    pub fields: Vec<FieldData>,
    pub methods: Vec<MethodData>,
    pub attrs: ClassAttrs,
}

/// Data of the `InnerClass` annotation of a nested class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InnerClassData {
    /// Enclosing class descriptor, only set for member classes (not for local or anonymous ones).
    pub outer_class: Option<String>,
    /// Simple name, `None` for anonymous classes.
    pub name: Option<String>,
    /// Source level access flags of the class.
    pub access_flags: u32,
}

/// Compile time constant value of a field.
///
/// Floating point values are kept as bits (NaN is canonical) so values can be compared and
/// hashed like boxed Java values.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstValue {
    Boolean(bool),
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    Float(u32),
    Double(u64),
    /// String as UTF-16 code units.
    Str(Vec<u16>),
    /// `EncodedType.ENCODED_NULL`: an explicit `null` static value (allowed for any field type,
    /// not only reference types, since the bytecode just stores the "no value" marker).
    Null,
}

impl ConstValue {
    pub fn from_f32(v: f32) -> ConstValue {
        ConstValue::Float(if v.is_nan() { 0x7fc0_0000 } else { v.to_bits() })
    }

    pub fn from_f64(v: f64) -> ConstValue {
        ConstValue::Double(if v.is_nan() {
            0x7ff8_0000_0000_0000
        } else {
            v.to_bits()
        })
    }
}

/// Class or field referenced from an annotation value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationRef {
    /// Class descriptor (annotation class or class value).
    Class(String),
    /// Field or enum constant value.
    Field(FieldRefData),
}

/// Reference found in an instruction of a method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeRef {
    /// Type descriptor operand.
    Type(String),
    Field(FieldRefData),
    /// Method operand, or the implementation method handle of a call site.
    Method(MethodRefData),
}

/// Access to the instructions of methods for analyses that need only the referenced entities.
pub trait CodeRefProvider {
    /// References of all instructions of the method at `code_offset` of the input class with
    /// index `class_idx` (position in the class list handed to the core).
    fn code_refs(&self, class_idx: usize, code_offset: u32) -> Result<Vec<CodeRef>, String>;
}

/// Exception handlers of a try block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchData {
    /// Handler offsets (code units), parallel to `types`.
    pub handlers: Vec<u32>,
    pub types: Vec<String>,
    /// Catch-all handler offset, `-1` if none.
    pub catch_all: i64,
}

/// A try block: `[start, end]` (inclusive) in code units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TryData {
    pub start: u32,
    pub end: u32,
    pub catch: CatchData,
}

/// Visibility of an annotation in the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationVisibility {
    Build,
    Runtime,
    System,
}

/// Value of an annotation element or a method default.
#[derive(Debug, Clone, PartialEq)]
pub enum EncodedValueData {
    Null,
    Boolean(bool),
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    /// Bits of the `f32` value.
    Float(u32),
    /// Bits of the `f64` value.
    Double(u64),
    /// String as UTF-16 code units.
    Str(Vec<u16>),
    /// Class descriptor.
    Type(String),
    /// Static field reference.
    Field(FieldRefData),
    /// Enum constant reference.
    Enum(FieldRefData),
    /// Method reference (not printed).
    Method,
    Array(Vec<EncodedValueData>),
    Annotation(AnnotationData),
    /// Value kind that can not be printed (method handle or type).
    Unsupported(String),
}

/// An annotation with its element values (in input order).
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationData {
    /// Annotation class descriptor.
    pub type_desc: String,
    pub visibility: AnnotationVisibility,
    pub values: Vec<(String, EncodedValueData)>,
}

/// Collect the annotations of one node like `AnnotationsAttr.pack`: system annotations are
/// dropped, annotations of the same class replace each other and the result is in the iteration
/// order of the reference's hash map (keyed by the class descriptor).
pub fn pack_annotations(list: Vec<AnnotationData>) -> Vec<AnnotationData> {
    if list.is_empty() {
        return Vec::new();
    }
    let initial = list.len();
    let mut cap = initial.next_power_of_two().max(1);
    let mut buckets: Vec<Vec<AnnotationData>> = Vec::new();
    let mut size = 0usize;
    for ann in list {
        if ann.visibility == AnnotationVisibility::System {
            continue;
        }
        if buckets.is_empty() {
            buckets = vec![Vec::new(); cap];
        }
        let idx = java_hash_index(&ann.type_desc, cap);
        if let Some(slot) = buckets[idx]
            .iter_mut()
            .find(|a| a.type_desc == ann.type_desc)
        {
            *slot = ann;
            continue;
        }
        buckets[idx].push(ann);
        size += 1;
        if size * 4 > cap * 3 {
            let new_cap = cap * 2;
            let mut next: Vec<Vec<AnnotationData>> = vec![Vec::new(); new_cap];
            for bucket in buckets {
                for a in bucket {
                    let i = java_hash_index(&a.type_desc, new_cap);
                    next[i].push(a);
                }
            }
            buckets = next;
            cap = new_cap;
        }
    }
    buckets.into_iter().flatten().collect()
}

/// `HashMap` bucket index of a string key in a table of `cap` entries.
fn java_hash_index(key: &str, cap: usize) -> usize {
    let mut h: i32 = 0;
    for u in key.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(i32::from(u));
    }
    let h = (h as u32) ^ ((h as u32) >> 16);
    (h as usize) & (cap - 1)
}

/// Attributes of a class that are only needed to print it.
#[derive(Debug, Clone, Default)]
pub struct ClassAttrs {
    /// `SourceFile` attribute.
    pub source_file: Option<String>,
    /// Name of the input file the class was read from.
    pub input_file_name: Option<String>,
    /// Visible annotations (see [`pack_annotations`]).
    pub annotations: Vec<AnnotationData>,
}

/// Attributes of a field that are only needed to print it.
#[derive(Debug, Clone, Default)]
pub struct FieldAttrs {
    pub annotations: Vec<AnnotationData>,
}

/// Attributes of a method that are only needed to print it.
#[derive(Debug, Clone, Default)]
pub struct MethodAttrs {
    pub annotations: Vec<AnnotationData>,
    /// Annotations of the parameters, by parameter index (may be shorter than the parameters).
    pub param_annotations: Vec<Vec<AnnotationData>>,
    /// Declared exceptions (class descriptors).
    pub throws: Vec<String>,
    /// Default value of an annotation method.
    pub annotation_default: Option<EncodedValueData>,
    /// Names and access flags of the parameters (`dalvik.annotation.MethodParameters`).
    pub method_parameters: Vec<(Option<String>, u32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(desc: &str, vis: AnnotationVisibility) -> AnnotationData {
        AnnotationData {
            type_desc: desc.to_string(),
            visibility: vis,
            values: Vec::new(),
        }
    }

    fn packed(descs: &[&str]) -> Vec<String> {
        let list = descs
            .iter()
            .map(|d| {
                let vis = if d.starts_with("Ldalvik") {
                    AnnotationVisibility::System
                } else {
                    AnnotationVisibility::Runtime
                };
                ann(d, vis)
            })
            .collect();
        pack_annotations(list)
            .into_iter()
            .map(|a| a.type_desc)
            .collect()
    }

    /// Orders taken from `HashMap` iteration of the reference runtime.
    #[test]
    fn annotations_follow_the_hash_map_order() {
        assert_eq!(
            packed(&[
                "Lkotlin/Metadata;",
                "Ljava/lang/Deprecated;",
                "Landroidx/annotation/Nullable;"
            ]),
            [
                "Landroidx/annotation/Nullable;",
                "Lkotlin/Metadata;",
                "Ljava/lang/Deprecated;"
            ]
        );
        assert_eq!(
            packed(&[
                "Ldalvik/annotation/Signature;",
                "Ljava/lang/Deprecated;",
                "Lkotlin/Metadata;",
                "Landroidx/annotation/RestrictTo;",
                "Lcom/google/Foo;",
                "Lb/B;",
                "Lc/C;",
                "Ld/D;",
                "Le/E;"
            ]),
            [
                "Le/E;",
                "Ljava/lang/Deprecated;",
                "Ld/D;",
                "Lkotlin/Metadata;",
                "Lc/C;",
                "Lcom/google/Foo;",
                "Lb/B;",
                "Landroidx/annotation/RestrictTo;"
            ]
        );
        assert_eq!(
            packed(&[
                "Ljavax/annotation/Nonnull;",
                "Ljavax/annotation/meta/TypeQualifier;",
                "Ljava/lang/annotation/Documented;",
                "Ljava/lang/annotation/Retention;"
            ]),
            [
                "Ljavax/annotation/Nonnull;",
                "Ljavax/annotation/meta/TypeQualifier;",
                "Ljava/lang/annotation/Documented;",
                "Ljava/lang/annotation/Retention;"
            ]
        );
    }

    #[test]
    fn later_annotation_of_a_class_replaces_the_earlier() {
        let mut a = ann("Lx/A;", AnnotationVisibility::Runtime);
        a.values.push(("v".into(), EncodedValueData::Int(1)));
        let mut b = ann("Lx/A;", AnnotationVisibility::Build);
        b.values.push(("v".into(), EncodedValueData::Int(2)));
        let out = pack_annotations(vec![a, b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].values[0].1, EncodedValueData::Int(2));
    }
}
