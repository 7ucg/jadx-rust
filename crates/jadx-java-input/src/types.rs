//! Data types produced by the class file reader that go beyond the input API model
//! (annotations, call sites, debug info, ...).

use jadx_input_api::{FieldRefData, MethodProtoData, MethodRefData};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationVisibility {
    /// `RuntimeVisibleAnnotations`
    Runtime,
    /// `RuntimeInvisibleAnnotations`
    Build,
}

impl AnnotationVisibility {
    pub fn name(self) -> &'static str {
        match self {
            AnnotationVisibility::Runtime => "RUNTIME",
            AnnotationVisibility::Build => "BUILD",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub visibility: AnnotationVisibility,
    /// Annotation class descriptor.
    pub class: String,
    /// Element values in declaration order (a repeated name keeps its first position and the
    /// last value).
    pub values: Vec<(String, EncodedValue)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodHandleKind {
    StaticPut,
    StaticGet,
    InstancePut,
    InstanceGet,
    InvokeStatic,
    InvokeInstance,
    InvokeDirect,
    InvokeConstructor,
    InvokeInterface,
}

impl MethodHandleKind {
    pub fn name(self) -> &'static str {
        match self {
            MethodHandleKind::StaticPut => "STATIC_PUT",
            MethodHandleKind::StaticGet => "STATIC_GET",
            MethodHandleKind::InstancePut => "INSTANCE_PUT",
            MethodHandleKind::InstanceGet => "INSTANCE_GET",
            MethodHandleKind::InvokeStatic => "INVOKE_STATIC",
            MethodHandleKind::InvokeInstance => "INVOKE_INSTANCE",
            MethodHandleKind::InvokeDirect => "INVOKE_DIRECT",
            MethodHandleKind::InvokeConstructor => "INVOKE_CONSTRUCTOR",
            MethodHandleKind::InvokeInterface => "INVOKE_INTERFACE",
        }
    }

    pub fn is_field(self) -> bool {
        matches!(
            self,
            MethodHandleKind::StaticPut
                | MethodHandleKind::StaticGet
                | MethodHandleKind::InstancePut
                | MethodHandleKind::InstanceGet
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodHandleRef {
    Field(FieldRefData),
    Method(MethodRefData),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodHandle {
    pub kind: MethodHandleKind,
    pub target: MethodHandleRef,
}

/// Constant value of an annotation element, a `ConstantValue` attribute or a bootstrap method
/// argument.
#[derive(Debug, Clone, PartialEq)]
pub enum EncodedValue {
    Null,
    Boolean(bool),
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    /// String as UTF-16 code units.
    String(Vec<u16>),
    /// Class descriptor.
    Type(String),
    /// Enum constant (`parent_class` and `ty` hold the enum class descriptor).
    Enum(FieldRefData),
    MethodType(MethodProtoData),
    MethodHandle(MethodHandle),
    Array(Vec<EncodedValue>),
    Annotation(Annotation),
}

/// Dynamic call site of an `invokedynamic` instruction: the bootstrap method handle, the call
/// site name, its prototype and the static bootstrap arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct CallSite {
    /// `[bootstrap method handle, name, method type, static arguments...]`
    pub values: Vec<EncodedValue>,
}

impl CallSite {
    /// Prototype of the call site (captured arguments and the produced type).
    pub fn proto(&self) -> Option<&MethodProtoData> {
        match self.values.get(2) {
            Some(EncodedValue::MethodType(p)) => Some(p),
            _ => None,
        }
    }
}

/// One entry of the `InnerClasses` attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InnerClassInfo {
    pub inner_class: String,
    pub outer_class: Option<String>,
    pub name: Option<String>,
    pub access_flags: u32,
}

/// One entry of the `MethodParameters` attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodParam {
    pub name: Option<String>,
    pub access_flags: u32,
}

/// Entry of the `LocalVariableTable` (register numbers are already shifted to the register
/// numbering of the decoded instructions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVar {
    pub reg: u32,
    pub name: Option<String>,
    pub ty: Option<String>,
    pub signature: Option<String>,
    pub start: u32,
    /// Inclusive end offset.
    pub end: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugInfo {
    /// `(code offset, source line)` sorted by offset.
    pub lines: Vec<(u32, u32)>,
    pub vars: Vec<LocalVar>,
}

/// Sizes of a `Code` attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeInfo {
    pub max_stack: u32,
    pub max_locals: u32,
    pub code_length: u32,
}

impl CodeInfo {
    /// Register that holds the first local variable (and the first method argument): one
    /// register per operand stack slot plus a temporary register used by `swap`.
    pub fn args_start_reg(&self) -> u32 {
        self.max_stack + 1
    }

    /// Total number of registers.
    pub fn registers_count(&self) -> u32 {
        self.max_stack + 1 + self.max_locals
    }
}

/// Field as read from a class file.
#[derive(Debug, Clone)]
pub struct JavaField {
    pub name: String,
    pub descriptor: String,
    pub access_flags: u32,
    pub signature: Option<String>,
    pub const_value: Option<EncodedValue>,
    /// Runtime annotations followed by build annotations.
    pub annotations: Vec<Annotation>,
}

/// Method as read from a class file.
#[derive(Debug, Clone)]
pub struct JavaMethod {
    pub name: String,
    pub descriptor: String,
    /// Access flags, `<init>` carries the jadx constructor flag.
    pub access_flags: u32,
    /// Offset of the `Code` attribute data in the class file.
    pub code_offset: Option<u32>,
    pub signature: Option<String>,
    pub exceptions: Option<Vec<String>>,
    pub parameters: Option<Vec<MethodParam>>,
    pub annotation_default: Option<EncodedValue>,
    /// Runtime annotations followed by build annotations.
    pub annotations: Vec<Annotation>,
    /// Per parameter (as stored in the file, may differ from the descriptor argument count).
    pub param_annotations: Vec<Vec<Annotation>>,
}
