//! `ProcessMethodsForInline`: mark synthetic methods (and constructors of anonymous classes)
//! as candidates for inlining and fix the class dependencies that this inlining implies.

use crate::model::node::{MethodId, Root, ACC_STATIC, ACC_SYNTHETIC};

pub fn process_methods_for_inline(root: &mut Root, inline_anonymous_classes: bool) {
    for c in root.depth_traversal_order() {
        for m in 0..root.classes[c].methods.len() {
            if can_inline(root, (c, m), inline_anonymous_classes) {
                root.classes[c].methods[m].inline_candidate = true;
                fix_class_dependencies(root, (c, m));
            }
        }
    }
}

fn can_inline(root: &Root, id: MethodId, inline_anonymous_classes: bool) -> bool {
    let mth = &root.classes[id.0].methods[id.1];
    if mth.code_offset.is_none() {
        return false;
    }
    let synthetic = mth.access & ACC_SYNTHETIC != 0 || mth.info.name.contains('$');
    synthetic
        && (mth.access & ACC_STATIC != 0 || (mth.is_constructor() && inline_anonymous_classes))
}

fn fix_class_dependencies(root: &mut Root, id: MethodId) {
    let parent_top = root.top_parent_index(id.0);
    for use_mth in root.classes[id.0].methods[id.1].use_in.clone() {
        // remove possible cross dependency
        // to force class with inline method to be processed before its usage
        let use_top = root.top_parent_index(use_mth.0);
        if use_top != parent_top {
            let deps = &mut root.classes[parent_top].dependencies;
            if let Some(p) = deps.iter().position(|d| *d == use_top) {
                deps.remove(p);
            }
            let codegen = &mut root.classes[use_top].codegen_deps;
            if !codegen.contains(&parent_top) {
                codegen.push(parent_top);
            }
        }
    }
}
