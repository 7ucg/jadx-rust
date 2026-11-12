//! `ExtractFieldInit`: move the initialisation of fields from the class init method and from the
//! constructors into the field declaration.
//!
//! The moved assignment is kept as the `init` of the field (`FieldInit`), the instruction stays in
//! the code of the method it came from. The display order of the fields changes when fields are
//! moved (`ClassNode::fields_order`).

use std::collections::{HashMap, HashSet};

use crate::model::cfg::MethodCfg;
use crate::model::info::FieldInfo;
use crate::model::insn::{
    iflag, BlockId, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType,
};
use crate::model::node::{FieldInitInsn, MethodId, Root, ACC_FINAL, ACC_STATIC};
use crate::passes::driver::MethodState;
use crate::passes::fix_instructions::visit_insns;
use crate::passes::insn_utils::can_reorder;

/// `FieldInitInfo`
struct FieldInitInfo {
    field: usize,
    put_insn: InsnId,
    can_move: bool,
}

pub fn extract_field_init(
    root: &mut Root,
    methods: &mut HashMap<MethodId, MethodState>,
    cls: usize,
) -> Result<(), String> {
    if !root.classes[cls].fields.is_empty() {
        move_static_fields_init(root, methods, cls)?;
        move_common_fields_init(root, methods, cls)?;
    }
    Ok(())
}

fn cfg_of(methods: &HashMap<MethodId, MethodState>, id: MethodId) -> Option<&MethodCfg> {
    methods.get(&id).and_then(|s| s.cfg.as_ref())
}

fn move_static_fields_init(
    root: &mut Root,
    methods: &mut HashMap<MethodId, MethodState>,
    cls: usize,
) -> Result<(), String> {
    let Some(mi) = root.classes[cls]
        .methods
        .iter()
        .position(|m| m.info.is_class_init())
    else {
        return Ok(());
    };
    if root.classes[cls].methods[mi].access & ACC_STATIC == 0
        || cfg_of(methods, (cls, mi)).is_none()
    {
        return Ok(());
    }
    if !root.classes[cls]
        .fields
        .iter()
        .any(|f| f.access & ACC_STATIC != 0)
    {
        return Ok(());
    }
    while process_static_fields(root, methods, cls, mi)? {
        // sometimes instructions moved to field init prevent from vars inline -> inline and try again
        let cfg = methods
            .get_mut(&(cls, mi))
            .and_then(|s| s.cfg.as_mut())
            .ok_or_else(|| "no class init code".to_string())?;
        crate::passes::regions::shrink_method(cfg)?;
    }
    Ok(())
}

fn process_static_fields(
    root: &mut Root,
    methods: &mut HashMap<MethodId, MethodState>,
    cls: usize,
    mi: usize,
) -> Result<bool, String> {
    let cfg = methods
        .get_mut(&(cls, mi))
        .and_then(|s| s.cfg.as_mut())
        .ok_or_else(|| "no class init code".to_string())?;
    let mut inits = collect_fields_init(root, cfg, cls, InsnType::Sput);
    if inits.is_empty() {
        return Ok(false);
    }
    // ignore field init constant if field initialized in class init method
    // (`ExtractFieldInit.processStaticFields`: `field.remove(CONSTANT_VALUE)`)
    for fi in &inits {
        let f = &mut root.classes[cls].fields[fi.field];
        if f.access & ACC_FINAL != 0 {
            f.zero_init = false;
            f.const_value = None;
        }
    }
    filter_fields_init(root, cfg, cls, &mut inits);
    if inits.is_empty() {
        return Ok(false);
    }
    for fi in &inits {
        let put = fi.put_insn;
        if let Some(InsnArg::Wrapped(w)) = cfg.insn(put).args.first() {
            let wi = w.insn;
            cfg.insns[wi].flags |= iflag::DECLARE_VAR;
        }
        cfg.remove_insn(put)?;
        add_field_init_attr(root, cfg, (cls, mi), fi.field, put);
    }
    fix_fields_order(root, cfg, cls, &inits);
    Ok(true)
}

fn move_common_fields_init(
    root: &mut Root,
    methods: &mut HashMap<MethodId, MethodState>,
    cls: usize,
) -> Result<(), String> {
    if !root.classes[cls]
        .fields
        .iter()
        .any(|f| f.access & ACC_STATIC == 0)
    {
        return Ok(());
    }
    let constructors = get_constructors_list(root, methods, cls);
    if constructors.is_empty() {
        return Ok(());
    }
    let mut info_list: Vec<(usize, Vec<FieldInitInfo>)> = Vec::with_capacity(constructors.len());
    for &mi in &constructors {
        let cfg = cfg_of(methods, (cls, mi)).ok_or_else(|| "no constructor code".to_string())?;
        let mut inits = collect_fields_init(root, cfg, cls, InsnType::Iput);
        filter_fields_init_ro(root, cfg, cls, &mut inits);
        if inits.is_empty() {
            return Ok(());
        }
        info_list.push((mi, inits));
    }
    // compare collected instructions
    for i in 1..info_list.len() {
        if !compare_field_inits(methods, cls, &info_list[0], &info_list[i]) {
            return Ok(());
        }
    }
    // all checks passed
    for (mi, inits) in &info_list {
        let cfg = methods
            .get_mut(&(cls, *mi))
            .and_then(|s| s.cfg.as_mut())
            .ok_or_else(|| "no constructor code".to_string())?;
        for fi in inits {
            if let Some(InsnArg::Wrapped(w)) = cfg.insn(fi.put_insn).args.first() {
                let wi = w.insn;
                cfg.insns[wi].flags |= iflag::DECLARE_VAR;
            }
            cfg.remove_insn(fi.put_insn)?;
        }
    }
    let (common_mi, common_inits) = &info_list[0];
    let cfg = methods
        .get_mut(&(cls, *common_mi))
        .and_then(|s| s.cfg.as_mut())
        .ok_or_else(|| "no constructor code".to_string())?;
    for fi in common_inits {
        add_field_init_attr(root, cfg, (cls, *common_mi), fi.field, fi.put_insn);
    }
    fix_fields_order(root, cfg, cls, common_inits);
    Ok(())
}

fn collect_fields_init(
    root: &Root,
    cfg: &MethodCfg,
    cls: usize,
    put_type: InsnType,
) -> Vec<FieldInitInfo> {
    let mut fields_init = Vec::new();
    let single_path: HashSet<BlockId> = single_path_blocks(cfg);
    let mut can_reorder_flag = true;
    for &block in &cfg.blocks {
        for &insn in &cfg.block(block).insns {
            let mut field_insn = false;
            let n = cfg.insn(insn);
            if n.ty == put_type {
                if let InsnKind::Index(IndexValue::Field(f)) = &n.kind {
                    if f.decl_class == root.classes[cls].info {
                        if let Some(fi) = root.classes[cls].fields.iter().position(|x| x.info == *f)
                        {
                            fields_init.push(FieldInitInfo {
                                field: fi,
                                put_insn: insn,
                                can_move: can_reorder_flag && single_path.contains(&block),
                            });
                            field_insn = true;
                        }
                    }
                }
            }
            if !field_insn && can_reorder_flag && !can_reorder(cfg, insn) {
                can_reorder_flag = false;
            }
        }
    }
    fields_init
}

/// `BlockUtils.visitSinglePath` from the enter block.
fn single_path_blocks(cfg: &MethodCfg) -> HashSet<BlockId> {
    let mut set = HashSet::new();
    let mut cur = Some(cfg.enter);
    while let Some(b) = cur {
        if !set.insert(b) {
            break;
        }
        let blk = cfg.block(b);
        cur = if blk.succs.len() == 1 {
            let next = blk.succs[0];
            if cfg.block(next).preds.len() > 1 {
                None
            } else {
                Some(next)
            }
        } else {
            None
        };
    }
    set
}

fn filter_fields_init(root: &Root, cfg: &MethodCfg, cls: usize, inits: &mut Vec<FieldInitInfo>) {
    filter_fields_init_ro(root, cfg, cls, inits)
}

fn filter_fields_init_ro(root: &Root, cfg: &MethodCfg, cls: usize, inits: &mut Vec<FieldInitInfo>) {
    let field_info =
        |fi: &FieldInitInfo| -> FieldInfo { root.classes[cls].fields[fi.field].info.clone() };
    // exclude fields initialized several times
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for fi in inits.iter() {
        *counts.entry(fi.field).or_insert(0) += 1;
    }
    let mut excluded: HashSet<FieldInfo> = counts
        .iter()
        .filter(|(_, c)| **c > 1)
        .map(|(f, _)| root.classes[cls].fields[*f].info.clone())
        .collect();
    for fi in inits.iter() {
        if !check_insn(cfg, fi) {
            excluded.insert(field_info(fi));
        }
    }
    if !excluded.is_empty() {
        loop {
            let mut changed = false;
            for fi in inits.iter() {
                let info = field_info(fi);
                if excluded.contains(&info) {
                    continue;
                }
                if insn_use_excluded_field(cfg, fi, &excluded) {
                    excluded.insert(info);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }
    // apply
    if !excluded.is_empty() {
        inits.retain(|fi| !excluded.contains(&field_info(fi)));
    }
}

fn check_insn(cfg: &MethodCfg, init: &FieldInitInfo) -> bool {
    if !init.can_move {
        return false;
    }
    let insn = cfg.insn(init.put_insn);
    match insn.args.first() {
        Some(InsnArg::Wrapped(w)) => {
            if !can_reorder(cfg, w.insn) && insn.catch.is_some() {
                return false;
            }
        }
        Some(InsnArg::Literal(_)) => return true,
        Some(InsnArg::Register(r)) => return r.is_this,
        _ => return false,
    }
    for a in crate::passes::insn_utils::register_args(cfg, init.put_insn) {
        if let Some(r) = cfg.try_reg(a) {
            if !r.is_this {
                return false;
            }
        }
    }
    true
}

fn insn_use_excluded_field(
    cfg: &MethodCfg,
    init: &FieldInitInfo,
    excluded: &HashSet<FieldInfo>,
) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let static_field = cfg.insn(init.put_insn).ty == InsnType::Sput;
    let use_type = if static_field {
        InsnType::Sget
    } else {
        InsnType::Iget
    };
    let mut all = Vec::new();
    visit_insns(cfg, init.put_insn, &mut all);
    all.into_iter().any(|i| {
        let n = cfg.insn(i);
        n.ty == use_type
            && matches!(&n.kind, InsnKind::Index(IndexValue::Field(f)) if excluded.contains(f))
    })
}

fn fix_fields_order(root: &mut Root, cfg: &MethodCfg, cls: usize, inits: &[FieldInitInfo]) {
    let ordered_fields = process_fields_dependencies(root, cfg, cls, inits);
    // apply source line and alias name sorting (same as for methods and inner classes)
    let mut cls_fields: Vec<usize> = root.classes[cls]
        .fields_order
        .clone()
        .unwrap_or_else(|| (0..root.classes[cls].fields.len()).collect());
    // `ClassGen.getShortAlias`: alias and type signature
    let alias = |f: usize| -> Vec<u16> {
        let fld = &root.classes[cls].fields[f];
        format!(
            "{}:{}",
            fld.alias.as_deref().unwrap_or(&fld.info.name),
            crate::model::info::type_signature(&fld.info.ty)
        )
        .encode_utf16()
        .collect()
    };
    cls_fields.sort_by_key(|f| alias(*f));
    if !ordered_fields.is_empty() {
        // check if already ordered
        let ordered = cls_fields
            .windows(ordered_fields.len())
            .any(|w| w == ordered_fields.as_slice());
        if !ordered {
            cls_fields.retain(|f| !ordered_fields.contains(f));
            cls_fields.extend(ordered_fields);
        }
    }
    root.classes[cls].fields_order = Some(cls_fields);
}

fn process_fields_dependencies(
    root: &Root,
    cfg: &MethodCfg,
    cls: usize,
    inits: &[FieldInitInfo],
) -> Vec<usize> {
    // collect dependant fields
    let deps = build_field_deps(root, cfg, cls, inits);
    if deps.is_empty() {
        return Vec::new();
    }
    // build new list with deps fields before usage field
    let ordered_fields: Vec<usize> = inits.iter().map(|i| i.field).collect();
    let mut result: Vec<usize> = Vec::new();
    for field in ordered_fields {
        let idx = result.iter().position(|f| *f == field);
        let Some(field_deps) = deps.get(&field) else {
            if idx.is_none() {
                result.push(field);
            }
            continue;
        };
        let Some(idx) = idx else {
            for dep in field_deps {
                if !result.contains(dep) {
                    result.push(*dep);
                }
            }
            result.push(field);
            continue;
        };
        for dep in field_deps {
            match result.iter().position(|f| f == dep) {
                None => result.insert(idx, *dep),
                Some(dep_idx) if dep_idx > idx => {
                    result.remove(dep_idx);
                    result.insert(idx, *dep);
                }
                _ => {}
            }
        }
    }
    result
}

fn build_field_deps(
    root: &Root,
    cfg: &MethodCfg,
    cls: usize,
    inits: &[FieldInitInfo],
) -> HashMap<usize, Vec<usize>> {
    let mut deps: HashMap<usize, Vec<usize>> = HashMap::new();
    for init in inits {
        let static_field = cfg.insn(init.put_insn).ty == InsnType::Sput;
        let use_type = if static_field {
            InsnType::Sget
        } else {
            InsnType::Iget
        };
        let mut all = Vec::new();
        visit_insns(cfg, init.put_insn, &mut all);
        for i in all {
            let n = cfg.insn(i);
            if n.ty != use_type {
                continue;
            }
            let InsnKind::Index(IndexValue::Field(f)) = &n.kind else {
                continue;
            };
            if f.decl_class != root.classes[cls].info {
                continue;
            }
            if let Some(dep) = root.classes[cls].fields.iter().position(|x| x.info == *f) {
                deps.entry(init.field).or_default().push(dep);
            }
        }
    }
    deps
}

fn compare_field_inits(
    methods: &HashMap<MethodId, MethodState>,
    cls: usize,
    base: &(usize, Vec<FieldInitInfo>),
    other: &(usize, Vec<FieldInitInfo>),
) -> bool {
    if base.1.len() != other.1.len() {
        return false;
    }
    let (Some(bc), Some(oc)) = (
        cfg_of(methods, (cls, base.0)),
        cfg_of(methods, (cls, other.0)),
    ) else {
        return false;
    };
    base.1
        .iter()
        .zip(&other.1)
        .all(|(a, b)| same_field_init(bc, a.put_insn, oc, b.put_insn))
}

/// `InsnNode.isSame`: type, instruction specific data and the shape of wrapped instructions.
fn is_same(a: &MethodCfg, ia: InsnId, b: &MethodCfg, ib: InsnId) -> bool {
    let (x, y) = (a.insn(ia), b.insn(ib));
    if x.ty != y.ty || x.args.len() != y.args.len() {
        return false;
    }
    for (p, q) in x.args.iter().zip(&y.args) {
        if let InsnArg::Wrapped(pw) = p {
            let InsnArg::Wrapped(qw) = q else {
                return false;
            };
            if !is_same(a, pw.insn, b, qw.insn) {
                return false;
            }
        }
    }
    match (&x.kind, &y.kind) {
        (InsnKind::Index(p), InsnKind::Index(q)) => p == q,
        (
            InsnKind::Invoke {
                invoke_type: t1,
                mth: m1,
            },
            InsnKind::Invoke {
                invoke_type: t2,
                mth: m2,
            },
        ) => t1 == t2 && m1 == m2,
        (InsnKind::Arith(p), InsnKind::Arith(q)) => p == q,
        (InsnKind::ConstStr(p), InsnKind::ConstStr(q)) => p == q,
        (InsnKind::NewArray(p), InsnKind::NewArray(q)) => p == q,
        (InsnKind::FilledNewArray(p), InsnKind::FilledNewArray(q)) => p == q,
        (
            InsnKind::Constructor {
                call_mth: m1,
                call_type: t1,
            },
            InsnKind::Constructor {
                call_mth: m2,
                call_type: t2,
            },
        ) => m1 == m2 && t1 == t2,
        _ => true,
    }
}

fn same_field_init(a: &MethodCfg, ia: InsnId, b: &MethodCfg, ib: InsnId) -> bool {
    if !is_same(a, ia, b, ib) {
        return false;
    }
    let (x, y) = (a.insn(ia), b.insn(ib));
    for (p, q) in x.args.iter().zip(&y.args) {
        // Constructors can use different registers and SSA variables for the same instance.
        if matches!((p, q), (InsnArg::Register(r1), InsnArg::Register(r2)) if r1.is_this && r2.is_this)
        {
            continue;
        }
        match (p, q) {
            (InsnArg::Wrapped(pw), InsnArg::Wrapped(qw)) => {
                if !same_field_init(a, pw.insn, b, qw.insn) {
                    return false;
                }
            }
            (InsnArg::Literal(l1), InsnArg::Literal(l2)) => {
                if l1.literal != l2.literal || l1.ty != l2.ty {
                    return false;
                }
            }
            (InsnArg::Named(n1), InsnArg::Named(n2)) => {
                if n1.name != n2.name {
                    return false;
                }
            }
            // registers of different methods are different variables
            _ => return false,
        }
    }
    true
}

fn get_constructors_list(
    root: &Root,
    methods: &HashMap<MethodId, MethodState>,
    cls: usize,
) -> Vec<usize> {
    let mut list = Vec::new();
    for (mi, m) in root.classes[cls].methods.iter().enumerate() {
        if m.access & ACC_STATIC == 0 && m.access & crate::model::node::ACC_CONSTRUCTOR != 0 {
            list.push(mi);
            let Some(cfg) = cfg_of(methods, (cls, mi)) else {
                return Vec::new();
            };
            if cfg.blocks.iter().all(|b| cfg.block(*b).insns.is_empty()) {
                return Vec::new();
            }
        }
    }
    list
}

fn add_field_init_attr(
    root: &mut Root,
    cfg: &mut MethodCfg,
    mth: MethodId,
    field: usize,
    put_insn: InsnId,
) {
    let assign_insn = match cfg.insn(put_insn).args.first().cloned() {
        Some(InsnArg::Wrapped(w)) => w.insn,
        Some(arg) => {
            // `InsnNode.wrapArg`
            let mut n = InsnNode::plain(InsnType::OneArg);
            n.args.push(arg);
            cfg.add_insn(n)
        }
        None => return,
    };
    let f = &mut root.classes[mth.0].fields[field];
    f.init_insn = Some(FieldInitInsn {
        mth: mth.1,
        insn: assign_insn,
    });
    f.zero_init = false;
}
