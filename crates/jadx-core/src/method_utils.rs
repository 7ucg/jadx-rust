//! `MethodUtils`: details (signature types) of called methods.

use crate::clsp::ClspMethod;
use crate::model::info::MethodInfo;
use crate::model::insn::{InsnKind, InsnNode, InvokeType};
use crate::model::node::Root;
use crate::model::types::ArgType;

/// `IMethodDetails`
#[derive(Debug, Clone)]
pub enum MethodDetails {
    /// Method of the input (class index, method index).
    App(usize, usize),
    /// Framework method.
    Clsp(ClspMethod),
    /// Unknown method: only the info of the call.
    Simple(MethodInfo),
}

impl MethodDetails {
    pub fn return_type(&self, root: &Root) -> ArgType {
        match self {
            MethodDetails::App(c, m) => root.classes[*c].methods[*m].return_type.clone(),
            MethodDetails::Clsp(m) => m.return_type.clone(),
            MethodDetails::Simple(i) => i.return_type.clone(),
        }
    }

    pub fn arg_types(&self, root: &Root) -> Vec<ArgType> {
        match self {
            MethodDetails::App(c, m) => root.classes[*c].methods[*m].arg_types.clone(),
            MethodDetails::Clsp(m) => m.arg_types.clone(),
            MethodDetails::Simple(i) => i.arg_types.clone(),
        }
    }

    /// `getMethodInfo().getDeclClass().getType()`
    pub fn decl_class_type(&self, root: &Root) -> ArgType {
        match self {
            MethodDetails::App(c, _) => root.classes[*c].info.arg_type().clone(),
            MethodDetails::Clsp(m) => ArgType::object(&m.decl_class),
            MethodDetails::Simple(i) => i.decl_class.arg_type().clone(),
        }
    }
}

/// `getMethodDetails(MethodInfo)`
pub fn get_method_details(root: &Root, call: &MethodInfo) -> Option<MethodDetails> {
    if let Some((c, m)) = root.resolve_method(call) {
        return Some(MethodDetails::App(c, m));
    }
    clsp_method_details(root, call)
}

/// `ClspGraph.getMethodDetails`
fn clsp_method_details(root: &Root, call: &MethodInfo) -> Option<MethodDetails> {
    let cls = root.clsp.cls_details(&call.decl_class.raw_name())?;
    let id = call.short_id();
    if let Some(m) = cls.method_by_short_id(&id) {
        return Some(MethodDetails::Clsp(m.clone()));
    }
    for parent in cls.parents() {
        let Some(name) = parent.object_name() else {
            continue;
        };
        if let Some(pc) = root.clsp.cls_details(&name) {
            if let Some(m) = pc.method_by_short_id(&id) {
                return Some(MethodDetails::Clsp(m.clone()));
            }
        }
    }
    Some(MethodDetails::Simple(call.clone()))
}

/// The method an invoke or constructor instruction calls.
pub fn call_method(insn: &InsnNode) -> Option<&MethodInfo> {
    match &insn.kind {
        InsnKind::Invoke { mth, .. } => Some(mth),
        InsnKind::Constructor { call_mth, .. } => Some(call_mth),
        _ => None,
    }
}

/// `getMethodDetails(BaseInvokeNode)`: the attached details, else the details of the call.
pub fn insn_method_details(root: &Root, insn: &InsnNode) -> Option<MethodDetails> {
    if let Some(m) = &insn.details {
        return get_method_details(root, m);
    }
    get_method_details(root, call_method(insn)?)
}

/// `getMethodGenericReturnType`
pub fn method_generic_return_type(root: &Root, insn: &InsnNode) -> Option<ArgType> {
    let d = insn_method_details(root, insn)?;
    let rt = d.return_type(root);
    rt.contains_generic().then_some(rt)
}

/// `getMethodOriginDeclClass`
pub fn method_origin_decl_class(root: &Root, cls: usize, mth: usize) -> ArgType {
    let m = &root.classes[cls].methods[mth];
    if let Some(t) = &m.override_base_decl {
        return t.clone();
    }
    if let Some(b) = m.bridged_by {
        return method_origin_decl_class(root, cls, b);
    }
    root.classes[cls].info.arg_type().clone()
}

/// `BaseInvokeNode.getInstanceArg` index: argument 0 of non-static invokes.
pub fn has_instance_arg(insn: &InsnNode) -> bool {
    matches!(
        &insn.kind,
        InsnKind::Invoke { invoke_type, .. } if *invoke_type != InvokeType::Static
    ) && !insn.args.is_empty()
}

/// `BaseInvokeNode.getFirstArgOffset`
pub fn first_arg_offset(insn: &InsnNode) -> usize {
    match &insn.kind {
        InsnKind::Invoke { invoke_type, .. } => usize::from(*invoke_type != InvokeType::Static),
        _ => 0,
    }
}
