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

#[derive(Debug, Clone, PartialEq, Eq)]
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
