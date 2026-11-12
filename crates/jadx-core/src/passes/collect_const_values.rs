//! `CollectConstValues`: collect the values of static final fields into the const storage.

use jadx_input_api::ConstValue;

use crate::model::node::{Root, ACC_FINAL, ACC_PUBLIC, ACC_STATIC};

pub fn collect_const_values(root: &mut Root) {
    if !root.const_storage.is_replace_enabled() {
        return;
    }
    let mut found = Vec::new();
    for c in root.sorted_class_order() {
        for f in 0..root.classes[c].fields.len() {
            if let Some(value) = field_const_value(root, c, f) {
                found.push((c, f, value, is_accessible_globally(root, c, f)));
            }
        }
    }
    for (c, f, value, public) in found {
        root.const_storage.add_const_field(c, f, value, public);
    }
}

/// Field can be used from any class only if it and all its enclosing classes are public.
/// (fields of interfaces are implicitly public, so check declaring classes too)
fn is_accessible_globally(root: &Root, cls: usize, fld: usize) -> bool {
    if root.classes[cls].fields[fld].access & ACC_PUBLIC == 0 {
        return false;
    }
    let mut cur = Some(cls);
    while let Some(c) = cur {
        if root.classes[c].access & ACC_PUBLIC == 0 {
            return false;
        }
        cur = root.parent_index(c).filter(|p| *p != c);
    }
    true
}

/// Constant value of a static final field that is not used anywhere in the input.
pub fn field_const_value(root: &Root, cls: usize, fld: usize) -> Option<ConstValue> {
    let f = &root.classes[cls].fields[fld];
    if f.access & ACC_STATIC == 0 || f.access & ACC_FINAL == 0 {
        return None;
    }
    let value = f.const_value.as_ref()?;
    if !f.use_in.is_empty() {
        // field still used somewhere and not inlined by compiler, so we don't need to restore it
        return None;
    }
    Some(value.clone())
}
