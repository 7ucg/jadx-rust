//! DEX file reader: header, id tables, MUTF-8 strings, class definitions and loading from
//! `.dex` / v41 containers / `.apk`/`.jar`/`.zip`. Never panics on malformed input.

pub mod annotations;
pub mod class_data;
pub mod code_refs;
pub mod error;
pub mod file;
pub mod header;
pub mod insns;
pub mod loader;
pub mod mutf8;
pub mod reader;
pub mod smali;

pub use annotations::{Annotation, ClassAnnotations, EncodedAnnotation, EncodedValue, Visibility};
pub use class_data::{ClassData, CodeItem, EncodedField, EncodedMethod};
pub use code_refs::DexCodeRefs;
pub use error::{DexError, Result};
pub use file::{ClassDef, DexFile, FieldId, FieldRef, MethodId, MethodRef, ProtoId};
pub use header::{DexHeader, MapItem};
pub use loader::{load_bytes, load_path, LoadOptions, LoadResult};
