//! `ClassModifier`: remove (hide) synthetic classes, methods and fields.

use std::collections::HashMap;

use crate::model::cfg::MethodCfg;
use crate::model::insn::{IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType, InvokeType};
use crate::model::node::{
    FieldReplace, MethodId, Root, ACC_CONSTRUCTOR, ACC_PUBLIC, ACC_STATIC, ACC_SYNTHETIC,
};
use crate::model::types::ArgType;
use crate::passes::driver::MethodState;
use crate::passes::insn_utils::{same_reg_and_svar, wrap_instruction};

const ACC_PRIVATE: u32 = 0x2;
const ACC_PROTECTED: u32 = 0x4;

type Methods = HashMap<MethodId, MethodState>;

fn cfg_of(methods: &Methods, id: MethodId) -> Option<&MethodCfg> {
    methods.get(&id).and_then(|s| s.cfg.as_ref())
}

pub fn class_modifier(root: &mut Root, methods: &mut Methods, cls: usize) -> Result<(), String> {
    if root.classes[cls].info.short_name() == "package-info" {
        return Ok(());
    }
    if is_empty_synthetic_class(root, cls) {
        root.classes[cls].dont_generate = true;
        return Ok(());
    }
    remove_synthetic_fields(root, methods, cls)?;
    for mi in 0..root.classes[cls].methods.len() {
        remove_synthetic_methods(root, methods, cls, mi)?;
    }
    for mi in 0..root.classes[cls].methods.len() {
        remove_empty_methods(root, methods, cls, mi);
    }
    Ok(())
}

fn is_empty_synthetic_class(root: &Root, cls: usize) -> bool {
    let c = &root.classes[cls];
    c.access & ACC_SYNTHETIC != 0
        && c.fields.is_empty()
        && c.methods.is_empty()
        && root.inner_classes(cls).is_empty()
}

/// Remove synthetic fields if type is outer class or class will be inlined (anonymous)
fn remove_synthetic_fields(
    root: &mut Root,
    methods: &mut Methods,
    cls: usize,
) -> Result<(), String> {
    let inline = root.classes[cls].is_anonymous();
    if !(inline || root.classes[cls].info.is_inner()) {
        return Ok(());
    }
    for fi in 0..root.classes[cls].fields.len() {
        let field = &root.classes[cls].fields[fi];
        let fld_type = field.ty.clone();
        if field.access & ACC_SYNTHETIC == 0 || !fld_type.is_object() || fld_type.is_generic_type()
        {
            continue;
        }
        let Some(fields_cls) = root.resolve_class(&fld_type) else {
            continue;
        };
        let parent_class = root.classes[cls].info.parent_class().cloned();
        let is_parent_inst = parent_class.as_ref() == Some(&root.classes[fields_cls].info);
        if !(inline || is_parent_inst) {
            continue;
        }
        let mut found = 0;
        for mi in 0..root.classes[cls].methods.len() {
            if remove_field_usage_from_constructor(root, methods, cls, mi, fi, fields_cls)? {
                found += 1;
            }
        }
        if found != 0 {
            if is_parent_inst {
                let info = root.classes[fields_cls].info.clone();
                root.classes[cls].fields[fi].replace = Some(FieldReplace::ClassInstance(info));
            }
            root.classes[cls].fields[fi].dont_generate = true;
        }
    }
    Ok(())
}

fn remove_field_usage_from_constructor(
    root: &mut Root,
    methods: &mut Methods,
    cls: usize,
    mi: usize,
    field: usize,
    fields_cls: usize,
) -> Result<bool, String> {
    let m = &root.classes[cls].methods[mi];
    if m.access & ACC_CONSTRUCTOR == 0 || m.skip_first_arg {
        return Ok(false);
    }
    let Some(cfg) = methods.get_mut(&(cls, mi)).and_then(|s| s.cfg.as_mut()) else {
        return Ok(false);
    };
    let Some(arg0) = cfg.arg_regs.first().cloned() else {
        return Ok(false);
    };
    let Some(arg_var) = arg0.svar else {
        return Ok(false);
    };
    if cfg.svars[arg_var].ty != *root.classes[fields_cls].info.arg_type() {
        return Ok(false);
    }
    let enter_succs = crate::passes::regions::utils::cs(cfg, cfg.enter);
    let Some(&block) = enter_succs.first() else {
        return Err("no block after the enter block".to_string());
    };
    let Some(&insn) = cfg.block(block).insns.first() else {
        return Ok(false);
    };
    if cfg.insn(insn).ty != InsnType::Iput {
        return Ok(false);
    }
    let InsnKind::Index(IndexValue::Field(field_info)) = cfg.insn(insn).kind.clone() else {
        return Ok(false);
    };
    let put_arg0_same = matches!(cfg.insn(insn).args.first(),
        Some(InsnArg::Register(r)) if same_reg_and_svar(r, &arg0));
    if field_info != root.classes[cls].fields[field].info || !put_arg0_same {
        return Ok(false);
    }
    root.classes[cls].methods[mi].skip_first_arg = true;
    let instance = cfg.insn(insn).args[1].clone();
    cfg.remove_insn_in_block(block, insn)?;
    // other arg usage -> wrap with IGET insn
    if cfg.svars[arg_var].use_count() != 0 {
        let iget = InsnNode::new(
            InsnType::Iget,
            InsnKind::Index(IndexValue::Field(field_info)),
        );
        let iget_id = cfg.add_insn(iget);
        crate::passes::insn_utils::add_arg(cfg, iget_id, instance);
        for u in cfg.svars[arg_var].uses.clone() {
            wrap_instruction(cfg, u, iget_id, true)?;
        }
    }
    Ok(true)
}

fn remove_synthetic_methods(
    root: &mut Root,
    methods: &mut Methods,
    cls: usize,
    mi: usize,
) -> Result<(), String> {
    let m = &root.classes[cls].methods[mi];
    if cfg_of(methods, (cls, mi)).is_none() || m.dont_generate || m.access & ACC_SYNTHETIC == 0 {
        return Ok(());
    }
    if remove_bridge_method(root, methods, cls, mi) {
        root.classes[cls].methods[mi].dont_generate = true;
        return Ok(());
    }
    // remove synthetic constructor for inner classes
    let m = &root.classes[cls].methods[mi];
    if m.is_constructor() && (m.inline_candidate || m.anonymous_constructor) {
        let cfg = cfg_of(methods, (cls, mi)).expect("code");
        if let Some(insn) = only_one_insn_from_mth(cfg) {
            let args = cfg.arg_regs.clone();
            if is_removed_class_in_args(root, methods, cls, mi, &args) {
                modify_synthetic_method(root, methods, cls, mi, insn)?;
            }
        }
    }
    Ok(())
}

/// `BlockUtils.getOnlyOneInsnFromMth`
fn only_one_insn_from_mth(cfg: &MethodCfg) -> Option<InsnId> {
    let mut found: Option<InsnId> = None;
    for &b in &cfg.blocks {
        let insns = &cfg.block(b).insns;
        match insns.len() {
            0 => continue,
            1 => {
                if found.is_some() {
                    return None;
                }
                found = Some(insns[0]);
            }
            _ => return None,
        }
    }
    found
}

fn is_removed_class_in_args(
    root: &Root,
    methods: &mut Methods,
    cls: usize,
    mi: usize,
    args: &[crate::model::insn::RegisterArg],
) -> bool {
    let mut removed_found = false;
    for (ai, arg) in args.iter().enumerate() {
        let arg_type = match arg.svar {
            Some(v) => methods
                .get(&(cls, mi))
                .and_then(|s| s.cfg.as_ref())
                .map(|c| c.svars[v].ty.clone())
                .unwrap_or_else(ArgType::unknown_any),
            None => arg.init_type.clone(),
        };
        if !arg_type.is_object() {
            continue;
        }
        let remove = match root.resolve_class(&arg_type) {
            None => {
                // check if missing class from current top class
                match root.ctx.class_info(&arg_type) {
                    Ok(info) => info.parent_class().is_some_and(|p| {
                        root.classes[cls]
                            .info
                            .full_name()
                            .starts_with(p.full_name())
                    }),
                    Err(_) => false,
                }
            }
            Some(c) => root.classes[c].dont_generate || is_empty_synthetic_class(root, c),
        };
        if remove {
            if let Some(cfg) = methods.get_mut(&(cls, mi)).and_then(|s| s.cfg.as_mut()) {
                if let Some(r) = cfg.arg_regs.get_mut(ai) {
                    r.remove = true;
                }
            }
            removed_found = true;
        }
    }
    removed_found
}

/// Remove synthetic constructor and redirect calls to existing constructor
fn modify_synthetic_method(
    root: &mut Root,
    methods: &mut Methods,
    cls: usize,
    mi: usize,
    insn: InsnId,
) -> Result<(), String> {
    let cfg = cfg_of(methods, (cls, mi)).expect("code");
    let n = cfg.insn(insn);
    let InsnKind::Constructor {
        call_mth,
        call_type: crate::model::insn::CallType::This,
    } = &n.kind
    else {
        return Ok(());
    };
    let call_mth = call_mth.clone();
    let args = cfg.arg_regs.clone();
    if args.is_empty() {
        return Ok(());
    }
    // remove first arg for non-static class (references to outer class)
    // the parent of a top level class is the class itself
    let parent = root.parent_index(cls).unwrap_or(cls);
    let first_ty = args[0]
        .svar
        .map(|v| cfg.svars[v].ty.clone())
        .unwrap_or_else(ArgType::unknown_any);
    let args_count = root.classes[cls].methods[mi].info.arg_types.len();
    let mut skip: Vec<usize> = Vec::new();
    if first_ty == *root.classes[parent].info.arg_type() {
        skip.push(0);
    }
    // remove unused args
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.svar {
            if cfg.svars[v].use_count() == 0 {
                skip.push(i);
            }
        }
    }
    for i in skip {
        let s = root.classes[cls].methods[mi]
            .skip_args
            .get_or_insert_with(|| vec![false; args_count]);
        if i < s.len() {
            s[i] = true;
        }
    }
    if let Some((c, m)) = root.resolve_method(&call_mth) {
        root.classes[cls].methods[mi].replace = Some((c, m));
        root.classes[cls].methods[mi].dont_generate = true;
        // `UsageInfoVisitor.replaceMethodUsage`
        let source = (cls, mi);
        let source_use = std::mem::take(&mut root.classes[cls].methods[mi].use_in);
        let mut merged = root.classes[c].methods[m].use_in.clone();
        for u in source_use {
            if !merged.contains(&u) {
                merged.push(u);
            }
        }
        merged.retain(|u| *u != source);
        root.classes[c].methods[m].use_in = merged;
    }
    Ok(())
}

fn remove_bridge_method(root: &mut Root, methods: &Methods, cls: usize, mi: usize) -> bool {
    let Some(cfg) = cfg_of(methods, (cls, mi)) else {
        return false;
    };
    let mut all: Vec<InsnId> = Vec::new();
    for &b in &cfg.blocks {
        all.extend(cfg.block(b).insns.iter().copied());
    }
    if all.len() != 1 {
        return false;
    }
    let mut wrapped = all[0];
    if cfg.insn(wrapped).ty == InsnType::Return {
        if let Some(InsnArg::Wrapped(w)) = cfg.insn(wrapped).args.first() {
            wrapped = w.insn;
        }
    }
    check_synthetic_wrapper(root, cfg, cls, mi, wrapped)
}

fn check_synthetic_wrapper(
    root: &mut Root,
    cfg: &MethodCfg,
    cls: usize,
    mi: usize,
    insn: InsnId,
) -> bool {
    let n = cfg.insn(insn);
    if n.ty != InsnType::Invoke {
        return false;
    }
    let InsnKind::Invoke {
        invoke_type,
        mth: call_mth,
    } = &n.kind
    else {
        return false;
    };
    if *invoke_type == InvokeType::Super {
        return false;
    }
    let Some((wc, wm)) = root.resolve_method(call_mth) else {
        return false;
    };
    let wrapped_acc = root.classes[wc].methods[wm].access;
    if wrapped_acc & ACC_STATIC != 0 {
        return false;
    }
    if call_mth.arg_types.len() != root.classes[cls].methods[mi].info.arg_types.len() {
        return false;
    }
    // rename method only from current class
    if wc != cls {
        return false;
    }
    // all args must be registers passed from method args (allow only casts insns)
    if !n.args.iter().all(|a| registers_and_casts_only(cfg, a)) {
        return false;
    }
    // remove confirmed, change visibility and name if needed
    if wrapped_acc & ACC_PUBLIC == 0 {
        // must be public
        let m = &mut root.classes[wc].methods[wm];
        m.access = (m.access & !(ACC_PRIVATE | ACC_PROTECTED | ACC_PUBLIC)) | ACC_PUBLIC;
    }
    let bridge = &root.classes[cls].methods[mi];
    let alias = bridge
        .alias
        .clone()
        .unwrap_or_else(|| bridge.info.name.clone());
    let (override_set, override_base) = (bridge.override_set, bridge.override_base_decl.clone());
    let wrapped_m = &mut root.classes[wc].methods[wm];
    let wrapped_alias = wrapped_m
        .alias
        .clone()
        .unwrap_or_else(|| wrapped_m.info.name.clone());
    if wrapped_alias != alias {
        wrapped_m.alias = Some(alias);
    }
    wrapped_m.replace = Some((cls, mi));
    wrapped_m.override_set = override_set;
    wrapped_m.override_base_decl = override_base;
    true
}

fn registers_and_casts_only(cfg: &MethodCfg, arg: &InsnArg) -> bool {
    match arg {
        InsnArg::Register(_) => true,
        InsnArg::Wrapped(w) => {
            let n = cfg.insn(w.insn);
            n.ty == InsnType::CheckCast
                && n.args
                    .first()
                    .is_some_and(|a| registers_and_casts_only(cfg, a))
        }
        _ => false,
    }
}

/// Remove public empty constructors (static or default)
fn remove_empty_methods(root: &mut Root, methods: &Methods, cls: usize, mi: usize) {
    let m = &root.classes[cls].methods[mi];
    let cfg = cfg_of(methods, (cls, mi));
    if cfg.is_some_and(|c| !c.arg_regs.is_empty()) {
        return;
    }
    let public_constructor = m.is_constructor() && m.access & ACC_PUBLIC != 0;
    let cls_init = m.info.is_class_init() && m.access & ACC_STATIC != 0;
    if !(public_constructor || cls_init) {
        return;
    }
    let Some(cfg) = cfg else {
        return;
    };
    if !cfg.blocks.iter().all(|b| cfg.block(*b).insns.is_empty()) {
        return;
    }
    if cls_init {
        root.classes[cls].methods[mi].dont_generate = true;
    } else if is_default_constructor(root, methods, cls, mi)
        && !non_default_constructor_exists(root, methods, cls, mi)
        && root.classes[cls].methods[mi].annotation_refs.is_empty()
    {
        root.classes[cls].methods[mi].dont_generate = true;
    }
}

fn is_default_constructor(root: &Root, methods: &Methods, cls: usize, mi: usize) -> bool {
    if !root.classes[cls].methods[mi].is_constructor() {
        return false;
    }
    let Some(cfg) = cfg_of(methods, (cls, mi)) else {
        return true;
    };
    let mut default_arg_count = 0;
    // workaround for non-static inner class constructor, that has synthetic argument
    if root.classes[cls].info.is_inner() && root.classes[cls].access & ACC_STATIC == 0 {
        if let (Some(first), Some(outer)) = (cfg.arg_regs.first(), root.parent_index(cls)) {
            if first.init_type == *root.classes[outer].info.arg_type() {
                default_arg_count = 1;
            }
        }
    }
    cfg.arg_regs.len() == default_arg_count
}

fn non_default_constructor_exists(root: &Root, methods: &Methods, cls: usize, def: usize) -> bool {
    (0..root.classes[cls].methods.len()).any(|mi| {
        mi != def
            && root.classes[cls].methods[mi].is_constructor()
            && !is_default_constructor(root, methods, cls, mi)
    })
}
