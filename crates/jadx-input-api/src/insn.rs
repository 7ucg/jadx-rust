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
}

/// Class as read from the input (members only, code is loaded on demand).
#[derive(Debug, Clone)]
pub struct ClassData {
    pub type_desc: String,
    pub access_flags: u32,
    pub super_type: Option<String>,
    pub interfaces: Vec<String>,
    pub signature: Option<String>,
    pub fields: Vec<FieldData>,
    pub methods: Vec<MethodData>,
}
