//! Usage queries answered from the usage information the decompiler core computes on the root
//! (`use_in` / `used` lists of classes, methods and fields), without decoding any code.
//!
//! The lists are filled by the usage pass of the pre decompile stage; on a root that did not
//! run it they are empty. Compared to [`UsageIndex`](crate::UsageIndex) they carry no
//! instruction offsets and do not distinguish field reads from writes, but they are free.

use jadx_core::model::info::MethodInfo;
use jadx_core::model::node::{MethodId, Root};

use crate::resolve::FieldId;

/// Methods that call `method`.
pub fn method_users(root: &Root, method: MethodId) -> &[MethodId] {
    &root.classes[method.0].methods[method.1].use_in
}

/// Methods called by `method` that are part of the root.
pub fn method_callees(root: &Root, method: MethodId) -> &[MethodId] {
    &root.classes[method.0].methods[method.1].used
}

/// Methods called by `method` that are not part of the root.
pub fn method_external_callees(root: &Root, method: MethodId) -> &[MethodInfo] {
    &root.classes[method.0].methods[method.1].unresolved_used
}

/// Methods that read or write `field`.
pub fn field_users(root: &Root, field: FieldId) -> &[MethodId] {
    &root.classes[field.0].fields[field.1].use_in
}

/// Classes and methods that use `class`.
pub fn class_users(root: &Root, class: usize) -> (&[usize], &[MethodId]) {
    let c = &root.classes[class];
    (&c.use_in, &c.use_in_mth)
}
