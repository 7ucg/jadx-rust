//! `PrepareAccessFlags`: use class access flags from the `InnerClass` annotation.
//!
//! The flags stored in the class definition lack `private`, `protected` and `static` of nested
//! classes, the annotation keeps the source level flags. A member class that has lost its
//! `static` flag (for example by R8) is detected by its constructors: those of a non static
//! member class take the outer class instance as the first argument.

use crate::model::node::{Root, ACC_STATIC};
use crate::model::types::ArgType;

pub fn prepare_access_flags(root: &mut Root) {
    for idx in 0..root.classes.len() {
        process(root, idx);
    }
}

fn process(root: &mut Root, idx: usize) {
    let cls = &mut root.classes[idx];
    let Some(inner) = cls.inner_class.clone() else {
        return;
    };
    cls.access = inner.access_flags;
    if cls.access & ACC_STATIC == 0 {
        if let Some(outer) = &inner.outer_class {
            if verify_constructors(cls, &ArgType::object(outer)) {
                cls.access |= ACC_STATIC;
            }
        }
    }
}

/// True if the class has constructors and none of them takes the outer instance.
fn verify_constructors(cls: &crate::model::node::ClassNode, outer_type: &ArgType) -> bool {
    let mut found = false;
    for m in cls.methods.iter().filter(|m| m.info.is_constructor()) {
        if m.info.arg_types.first() == Some(outer_type) {
            return false;
        }
        found = true;
    }
    found
}
