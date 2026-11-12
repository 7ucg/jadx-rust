//! JVM `.class` / `.jar` input reader.
//!
//! Parses class files (also from `.jar`/`.zip` archives, nested archives included) and converts
//! the stack based JVM code to the register based, jadx-normalised
//! [`InsnData`](jadx_input_api::InsnData) model like the reference Java input plugin does:
//! every operand stack slot gets a register and local variable `n` lives in register
//! `max_stack + 1 + n` (see [`CodeInfo::args_start_reg`]).

mod api;
mod attrs;
mod class;
mod code;
mod error;
mod insns;
mod javahash;
mod loader;
mod mutf8;
mod reader;
mod types;

pub use api::{
    annotation_data, annotation_refs, array_elem_type, const_value, pack_annotations, value_data,
    JavaCodeRefs, NEW_ARRAY_PRIMITIVE_TAG,
};
pub use class::{parse_method_descriptor, JavaClass, ACC_CONSTRUCTOR};
pub use error::{JavaError, Result};
pub use loader::{load_bytes, load_path, LoadResult};
pub use types::{
    Annotation, AnnotationVisibility, CallSite, CodeInfo, DebugInfo, EncodedValue, InnerClassInfo,
    JavaField, JavaMethod, LocalVar, MethodHandle, MethodHandleKind, MethodHandleRef, MethodParam,
};
