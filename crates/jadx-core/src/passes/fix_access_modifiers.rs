//! `FixAccessModifiers`: widen the visibility of classes and methods that are used from places
//! where the original visibility does not allow the access (after inlining and class merging).

use std::collections::HashSet;

use crate::model::node::{MethodId, OverrideFirst, Root, ACC_PUBLIC};

const ACC_PRIVATE: u32 = 0x2;
const ACC_PROTECTED: u32 = 0x4;
const VISIBILITY_FLAGS: u32 = ACC_PUBLIC | ACC_PRIVATE | ACC_PROTECTED;

/// A node whose visibility is checked: a class or a method.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Node {
    Class(usize),
    Method(MethodId),
}

/// `AccessInfo.changeVisibility`: the new access flags.
fn changed_visibility(flags: u32, new_vis: u32) -> u32 {
    if flags & VISIBILITY_FLAGS == new_vis {
        flags
    } else {
        (flags & !VISIBILITY_FLAGS) | new_vis
    }
}

/// `FixAccessModifiers.changeVisibility` of a method.
pub fn change_method_visibility(root: &mut Root, id: MethodId, new_vis: u32) {
    let m = &mut root.classes[id.0].methods[id.1];
    m.access = changed_visibility(m.access, new_vis);
}

/// `FixAccessModifiers.changeVisibility` of a class.
pub fn change_class_visibility(root: &mut Root, cls: usize, new_vis: u32) {
    let c = &mut root.classes[cls];
    c.access = changed_visibility(c.access, new_vis);
}

fn change_node_visibility(root: &mut Root, node: Node, new_vis: u32) {
    match node {
        Node::Class(c) => change_class_visibility(root, c, new_vis),
        Node::Method(m) => change_method_visibility(root, m, new_vis),
    }
}

fn node_access(root: &Root, node: Node) -> u32 {
    match node {
        Node::Class(c) => root.classes[c].access,
        Node::Method(m) => root.classes[m.0].methods[m.1].access,
    }
}

/// `getDeclaringClass`: the outer class of an inner class, the class of a method.
fn declaring_class(root: &Root, node: Node) -> Option<usize> {
    match node {
        Node::Class(c) => {
            if root.classes[c].is_inner() {
                root.parent_index(c)
            } else {
                None
            }
        }
        Node::Method(m) => Some(m.0),
    }
}

fn is_super_type(root: &Root, cls: usize, super_cls: usize) -> bool {
    let name = root.classes[super_cls].info.raw_name();
    root.clsp
        .super_types(&root.classes[cls].info.raw_name())
        .is_some_and(|s| s.contains(&name))
}

/// `VisibilityUtils.checkVisibility`: nodes with a visibility that is not enough for the access
/// from `caller` and the visibility they need.
fn check_visibility(root: &Root, target: Node, caller: Node) -> Result<Vec<(Node, u32)>, String> {
    let target_cls = match target {
        Node::Class(c) => c,
        Node::Method(m) => m.0,
    };
    let caller_cls = match caller {
        Node::Class(c) => c,
        Node::Method(m) => m.0,
    };
    let mut out = Vec::new();
    if target_cls == caller_cls
        || root.top_parent_index(target_cls) == root.top_parent_index(caller_cls)
    {
        return Ok(out);
    }
    // the node itself, then all declaring classes
    let mut nodes = vec![target];
    let mut cur = target;
    while let Some(d) = declaring_class(root, cur) {
        cur = Node::Class(d);
        nodes.push(cur);
    }
    if root.classes[target_cls].info.package() == root.classes[caller_cls].info.package() {
        for n in nodes {
            if node_access(root, n) & ACC_PRIVATE != 0 {
                out.push((n, 0));
            }
        }
    } else {
        for n in nodes {
            let vis = node_access(root, n) & VISIBILITY_FLAGS;
            if vis == ACC_PUBLIC {
                continue;
            }
            let decl = declaring_class(root, n);
            if vis == ACC_PRIVATE || vis == 0 {
                let expected = if decl.is_some_and(|d| is_super_type(root, caller_cls, d)) {
                    ACC_PROTECTED
                } else {
                    ACC_PUBLIC
                };
                out.push((n, expected));
            } else if vis == ACC_PROTECTED {
                if !decl.is_some_and(|d| is_super_type(root, caller_cls, d)) {
                    out.push((n, ACC_PUBLIC));
                }
            } else {
                return Err(format!("{vis} is not supported"));
            }
        }
    }
    Ok(out)
}

fn apply_checks(root: &mut Root, target: Node, caller: Node) {
    // the callbacks run while the declaring nodes are visited: a changed visibility of a node does
    // not affect the check of the next node
    if let Ok(list) = check_visibility(root, target, caller) {
        for (n, vis) in list {
            change_node_visibility(root, n, vis);
        }
    }
}

/// Class part of the pass.
pub fn fix_class_visibility(root: &mut Root, cls: usize) {
    let access = root.classes[cls].access;
    let is_top = !root.classes[cls].is_inner();
    if is_top && access & ACC_PUBLIC != 0 {
        return;
    }
    if is_top && access & (ACC_PRIVATE | ACC_PROTECTED) != 0 {
        change_class_visibility(root, cls, ACC_PUBLIC);
        return;
    }
    for use_cls in root.classes[cls].use_in.clone() {
        apply_checks(root, Node::Class(cls), Node::Class(use_cls));
    }
    for use_mth in root.classes[cls].use_in_mth.clone() {
        let m = &root.classes[use_mth.0].methods[use_mth.1];
        let is_inline = m.inline.as_ref().is_some_and(|a| !a.is_not_needed());
        if is_inline || m.inline_candidate {
            let mut used_in: Vec<usize> = Vec::new();
            let mut seen = HashSet::new();
            for u in &m.use_in {
                if seen.insert(u.0) {
                    used_in.push(u.0);
                }
            }
            for use_cls in used_in {
                apply_checks(root, Node::Class(cls), Node::Class(use_cls));
            }
        }
    }
}

/// Method part of the pass.
pub fn fix_method_visibility(root: &mut Root, id: MethodId) {
    let m = &root.classes[id.0].methods[id.1];
    if m.dont_generate {
        return;
    }
    if let Some(first) = m.override_first {
        // visibility can't be weaker
        let parent_access = match first {
            OverrideFirst::App(p) => root.classes[p.0].methods[p.1].access,
            OverrideFirst::Clsp(a) => a,
        };
        let this_vis = m.access & VISIBILITY_FLAGS;
        let parent_vis = parent_access & VISIBILITY_FLAGS;
        if this_vis != parent_vis && ordered_visibility(this_vis) < ordered_visibility(parent_vis) {
            change_method_visibility(root, id, parent_vis);
        }
    }
    for use_mth in root.classes[id.0].methods[id.1].use_in.clone() {
        apply_checks(root, Node::Method(id), Node::Method(use_mth));
    }
}

fn ordered_visibility(flag: u32) -> u32 {
    match flag {
        ACC_PRIVATE => 1,
        0 => 2,
        ACC_PROTECTED => 3,
        _ => 4,
    }
}
