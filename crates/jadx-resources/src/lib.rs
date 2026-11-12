//! Android resource decoding: binary XML (`AndroidManifest.xml`, layouts, ...) to text, the
//! `resources.arsc` table to `res/values*/*.xml` and `public.xml`, and the resource id to name
//! map the decompiler uses for the `R` class fields.
//!
//! The text output follows the reference decompiler byte for byte.

pub mod android_map;
pub mod apk;
pub mod axml;
pub mod config;
pub mod consts;
pub mod error;
pub mod java_map;
pub mod jstr;
pub mod manifest_attrs;
pub mod names;
pub mod res_xml_gen;
pub mod stream;
pub mod strings;
pub mod table;
pub mod text;
pub mod values;
mod xml_chars;

pub use apk::{
    decode_apk, read_resource_table, ApkOptions, ApkResources, DecodedFile, ResourceType,
};
pub use axml::{BinaryXmlParser, ClassAliasFn, XmlContext};
pub use error::{ResError, Result};
pub use manifest_attrs::ManifestAttributes;
pub use names::ResourceNameSource;
pub use res_xml_gen::{make_public_xml, make_resources_xml};
pub use table::{RField, ResOptions, ResTable, ResourceEntry, ResourceStorage};
