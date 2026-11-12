//! `ModVisitor`: modify method instructions (remove, replace, process exception handlers).
//!
//! Constant field replacement (`CollectConstValues`) and anonymous class details
//! (`ProcessAnonymous`) are not ported: no constant field is ever found and no constructor is
//! marked as an anonymous class constructor.

use jadx_input_api::{ArrayData, ConstValue};

use crate::model::cfg::MethodCfg;
use crate::model::const_storage::FieldRef;
use crate::model::info::FieldInfo;
use crate::model::insn::{
    iflag, BlockId, FillArrayData, IfOp, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType,
    LiteralArg, NamedArg, RegisterArg,
};
use crate::model::node::{FieldNode, ACC_STATIC};
use crate::model::ssa::{ArgRef, SsaId};
use crate::model::types::ArgType;
use crate::passes::constructor::MethodEnv;
use crate::passes::insn_utils::{
    reg_name, replace_insn, replace_insn_by_ref, svar_set_name, wrap_insn_into_arg, Remover,
};
use crate::type_compare::{TypeCompare, TypeCompareEnum};

const ACC_PUBLIC: u32 = 0x1;
const ACC_PRIVATE: u32 = 0x2;
const ACC_PROTECTED: u32 = 0x4;

const DOUBLE_TO_BITS: i64 = 0x3FF0_0000_0000_0000;
const FLOAT_TO_BITS: i64 = 0x3F80_0000;

/// `IFieldInfoRef`: a field found for a constant value.
pub(crate) struct ConstField {
    pub info: FieldInfo,
    /// `(class, field)` index when the reference is a field of the input (`FieldNode`).
    pub node: Option<(usize, usize)>,
}

fn const_field_from_ref(env: &MethodEnv, r: &FieldRef) -> ConstField {
    match r {
        FieldRef::Node(c, f) => ConstField {
            info: env.root.classes[*c].fields[*f].info.clone(),
            node: Some((*c, *f)),
        },
        FieldRef::Info(info) => ConstField {
            info: info.clone(),
            node: None,
        },
    }
}

/// `ClassNode.getConstField(Object, searchGlobal)`
pub(crate) fn get_const_field(
    env: &MethodEnv,
    value: &ConstValue,
    search_global: bool,
) -> Option<ConstField> {
    env.root
        .const_storage
        .get_const_field(env.root, env.cls, value, search_global)
        .map(|r| const_field_from_ref(env, r))
}

/// `ClassNode.getConstField(String)`
pub(crate) fn get_const_field_str(env: &MethodEnv, value: &[u16]) -> Option<ConstField> {
    get_const_field(env, &ConstValue::Str(value.to_vec()), true)
}

/// `ClassNode.getConstField(ArgType)`: the constant storage only holds primitive and string
/// values, so a class literal never has a field.
fn get_const_field_class(_env: &MethodEnv, _value: &ArgType) -> Option<ConstField> {
    None
}

/// `ClassNode.getConstFieldByLiteralArg`
pub(crate) fn get_const_field_literal(env: &MethodEnv, value: &LiteralArg) -> Option<ConstField> {
    env.root
        .const_storage
        .get_const_field_by_literal(env.root, env.cls, value.literal, &value.ty)
        .map(|r| const_field_from_ref(env, r))
}

/// `ModVisitor.addFieldUsage`: remember that the method uses the field (`FieldNode.addUseIn`).
/// The usage of the processed methods is merged into the field nodes by the caller.
pub(crate) fn add_field_usage(cfg: &mut MethodCfg, field: &ConstField) {
    if let Some(n) = field.node {
        cfg.field_uses.push(n);
    }
}

pub fn mod_visitor(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    replace_step(cfg, env)?;
    remove_step(cfg)?;
    iterative_remove_step(cfg)
}

fn replace_step(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    for bid in cfg.blocks.clone() {
        let mut remover = Remover::default();
        let size = cfg.block(bid).insns.len();
        for i in 0..size {
            let iid = cfg.block(bid).insns[i];
            match cfg.insn(iid).ty {
                InsnType::Constructor => process_anonymous_constructor(cfg, env, iid),
                InsnType::Const | InsnType::ConstStr | InsnType::ConstClass => {
                    replace_const(cfg, env, bid, i, iid)?
                }
                InsnType::Switch => replace_const_keys(cfg, env, iid),
                InsnType::NewArray => process_new_array(cfg, env, bid, i, iid, &mut remover)?,
                InsnType::MoveException => process_move_exception(cfg, bid, iid, &mut remover)?,
                InsnType::Arith => process_arith(cfg, env, iid)?,
                InsnType::CmpL | InsnType::CmpG => inline_cmp_insns(cfg, iid, &mut remover)?,
                InsnType::CheckCast => remove_check_cast(cfg, env, bid, i, iid)?,
                InsnType::Cast => fix_primitive_cast(cfg, bid, i, iid)?,
                InsnType::Iput | InsnType::Iget => fix_field_usage(cfg, env, iid)?,
                _ => {}
            }
        }
        remover.perform(cfg, bid)?;
    }
    Ok(())
}

/// If field is not visible from use site => cast to origin class
fn fix_field_usage(cfg: &mut MethodCfg, env: &MethodEnv, insn: InsnId) -> Result<(), String> {
    let idx = if cfg.insn(insn).ty == InsnType::Iget {
        0
    } else {
        1
    };
    let at = ArgRef::Arg(insn, idx);
    match &cfg.insn(insn).args[idx] {
        InsnArg::Register(r) if r.is_super => return Ok(()),
        InsnArg::Wrapped(w) if cfg.insn(w.insn).ty == InsnType::Cast => return Ok(()),
        _ => {}
    }
    let InsnKind::Index(IndexValue::Field(field_info)) = cfg.insn(insn).kind.clone() else {
        return Ok(());
    };
    let cls_type = field_info.decl_class.arg_type().clone();
    let instance_type = cfg.arg_type(at);
    if cls_type == instance_type {
        // cast not needed
        return Ok(());
    }
    let compare = TypeCompare::new(&env.root.clsp);
    match env.root.resolve_field(&field_info) {
        None => {
            // unknown field
            let result = compare.compare_types(&instance_type, &cls_type);
            if result.is_equal()
                || (result == TypeCompareEnum::NarrowByGeneric && !instance_type.is_generic_type())
            {
                return Ok(());
            }
        }
        Some(field) => {
            if is_field_visible_in_method(env, field) {
                return Ok(());
            }
        }
    }
    // insert cast
    let instance_arg = cfg.insn(insn).args[idx].clone();
    let dup = cfg.duplicate_arg(&instance_arg);
    let mut cast = InsnNode::new(
        InsnType::Cast,
        InsnKind::Index(IndexValue::Type(cls_type.clone())),
    );
    cast.flags |= iflag::SYNTHETIC | iflag::EXPLICIT_CAST;
    let cast_id = cfg.add_insn(cast);
    let dup_var = match &dup {
        InsnArg::Register(r) => r.svar,
        _ => None,
    };
    cfg.insns[cast_id].args.push(dup);
    if let Some(v) = dup_var {
        cfg.svar_use(v, ArgRef::Arg(cast_id, 0));
    }
    let wrapped = wrap_insn_into_arg(cfg, cast_id)?;
    let InsnArg::Wrapped(mut w) = wrapped else {
        return Ok(());
    };
    w.ty = cls_type;
    cfg.replace_arg(insn, idx, InsnArg::Wrapped(w));
    Ok(())
}

fn is_field_visible_in_method(env: &MethodEnv, field: &FieldNode) -> bool {
    let access = field.access;
    if access & ACC_PUBLIC != 0 {
        return true;
    }
    let use_cls = env.cls;
    let field_cls = env.root.class_index(&field.info.decl_class.raw_name());
    let static_mth = env.root.classes[env.cls].methods[env.mth].access & ACC_STATIC != 0;
    if field_cls == Some(use_cls) && !static_mth {
        return true;
    }
    if access & ACC_PRIVATE != 0 {
        return false;
    }
    // package-private or protected
    let use_info = &env.root.classes[use_cls].info;
    if use_info.package() == field.info.decl_class.package() {
        // same package
        return true;
    }
    if access & (ACC_PUBLIC | ACC_PRIVATE | ACC_PROTECTED) == 0 {
        return false;
    }
    // protected: true if use class is subclass of field class
    let result = TypeCompare::new(&env.root.clsp)
        .compare_objects(use_info.arg_type(), field.info.decl_class.arg_type());
    result == TypeCompareEnum::Narrow
}

/// `ModVisitor.replaceConstKeys`
fn replace_const_keys(cfg: &mut MethodCfg, env: &MethodEnv, insn: InsnId) {
    let InsnKind::Switch {
        data: Some(data), ..
    } = &cfg.insn(insn).kind
    else {
        return;
    };
    let keys = data.keys.clone();
    for (k, key) in keys.into_iter().enumerate() {
        if let Some(f) = get_const_field(env, &ConstValue::Int(key), true) {
            if let InsnKind::Switch {
                data: Some(data), ..
            } = &mut cfg.insns[insn].kind
            {
                data.key_fields.resize(data.keys.len(), None);
                data.key_fields[k] = Some(f.info.clone());
            }
            add_field_usage(cfg, &f);
        }
    }
}

fn fix_primitive_cast(
    cfg: &mut MethodCfg,
    block: BlockId,
    i: usize,
    insn: InsnId,
) -> Result<(), String> {
    // replace boolean to (byte/char/short/long/double/float) cast with ternary
    if cfg.arg_type(ArgRef::Arg(insn, 0)) != ArgType::boolean() {
        return Ok(());
    }
    let ty = cfg.reg_type(ArgRef::Result(insn));
    if !ty.is_primitive() {
        return Ok(());
    }
    let Some(result) = cfg.insn(insn).result.clone() else {
        return Ok(());
    };
    let cast_arg = cfg.insn(insn).args[0].clone();
    let ternary = make_boolean_convert_insn(cfg, result, cast_arg, &ty);
    replace_insn(cfg, block, i, ternary)?;
    Ok(())
}

/// `ModVisitor.makeBooleanConvertInsn`
fn make_boolean_convert_insn(
    cfg: &mut MethodCfg,
    result: RegisterArg,
    cast_arg: InsnArg,
    ty: &ArgType,
) -> InsnNode {
    let zero = InsnArg::Literal(LiteralArg {
        literal: 0,
        ty: ty.clone(),
        explicit_primitive: false,
    });
    let lit_val = if *ty == ArgType::double() {
        DOUBLE_TO_BITS
    } else if *ty == ArgType::float() {
        FLOAT_TO_BITS
    } else {
        1
    };
    let one = InsnArg::Literal(LiteralArg {
        literal: lit_val,
        ty: ty.clone(),
        explicit_primitive: false,
    });
    let cast_var = match &cast_arg {
        InsnArg::Register(r) => r.svar,
        _ => None,
    };
    let mut if_node = InsnNode::new(
        InsnType::If,
        InsnKind::If {
            op: IfOp::Eq,
            target: -1,
            then_block: None,
            else_block: None,
        },
    );
    if_node.args.push(cast_arg);
    if_node.args.push(InsnArg::Literal(LiteralArg {
        literal: 1,
        ty: ArgType::boolean(),
        explicit_primitive: false,
    }));
    let cond = cfg.add_insn(if_node);
    if let Some(v) = cast_var {
        cfg.svar_use(v, ArgRef::Arg(cond, 0));
    }
    let mut ternary = InsnNode::new(InsnType::Ternary, InsnKind::Ternary { cond });
    ternary.result = Some(result);
    ternary.args.push(one);
    ternary.args.push(zero);
    ternary
}

fn replace_const(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    block: BlockId,
    i: usize,
    insn: InsnId,
) -> Result<(), String> {
    let f = {
        let n = cfg.insn(insn);
        match (&n.ty, &n.kind) {
            (InsnType::ConstStr, InsnKind::ConstStr(s)) => get_const_field_str(env, s),
            (InsnType::ConstClass, InsnKind::ConstClass(t)) => get_const_field_class(env, t),
            _ => match n.args.first() {
                Some(InsnArg::Literal(l)) => get_const_field_literal(env, l),
                _ => None,
            },
        }
    };
    if let Some(f) = f {
        let mut sget = InsnNode::new(
            InsnType::Sget,
            InsnKind::Index(IndexValue::Field(f.info.clone())),
        );
        sget.result = cfg.insn(insn).result.clone();
        replace_insn(cfg, block, i, sget)?;
        add_field_usage(cfg, &f);
    }
    Ok(())
}

fn process_arith(cfg: &mut MethodCfg, env: &MethodEnv, insn: InsnId) -> Result<(), String> {
    if cfg.insn(insn).args.len() != 2 {
        return Err(format!("Invalid args count in insn: {insn}"));
    }
    if let InsnArg::Literal(l) = cfg.insn(insn).args[1].clone() {
        if let Some(f) = get_const_field_literal(env, &l) {
            let sget = InsnNode::new(
                InsnType::Sget,
                InsnKind::Index(IndexValue::Field(f.info.clone())),
            );
            let id = cfg.add_insn(sget);
            let arg = cfg.wrap_arg(id)?;
            cfg.replace_arg(insn, 1, arg);
            add_field_usage(cfg, &f);
        }
    }
    Ok(())
}

/// Inline CMP instructions into 'if' to help conditions merging
fn inline_cmp_insns(
    cfg: &mut MethodCfg,
    insn: InsnId,
    remover: &mut Remover,
) -> Result<(), String> {
    let Some(var) = cfg.insn(insn).result.as_ref().and_then(|r| r.svar) else {
        return Err("No SSA var for result arg".into());
    };
    let uses = cfg.svars[var].uses.clone();
    let all_if = !uses.is_empty()
        && uses.iter().all(|u| {
            u.parent_insn()
                .is_some_and(|p| cfg.insn(p).ty == InsnType::If)
        });
    if !all_if {
        return Ok(());
    }
    for use_arg in uses {
        let ArgRef::Arg(use_insn, k) = use_arg else {
            continue;
        };
        let copy = cfg.copy_without_result(insn);
        let wrap = wrap_insn_into_arg(cfg, copy)?;
        // `replaceArg` only succeeds if the use is still an argument of its parent
        if !matches!(cfg.insn(use_insn).args.get(k), Some(InsnArg::Register(_))) {
            cfg.warnings
                .push(format!("Failed to inline CMP insn: {insn} into {use_insn}"));
            return Ok(());
        }
        cfg.replace_arg(use_insn, k, wrap);
    }
    remover.add_and_unbind(cfg, insn)
}

fn process_new_array(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    block: BlockId,
    i: usize,
    insn: InsnId,
    remover: &mut Remover,
) -> Result<(), String> {
    // replace with filled array if 'fill-array' is next instruction
    let Some(var) = cfg.insn(insn).result.as_ref().and_then(|r| r.svar) else {
        return Err("No SSA var for result arg".into());
    };
    let Some(next) = get_first_use_skip_move(cfg, var) else {
        return Ok(());
    };
    if cfg.insn(next).ty != InsnType::FillArray {
        return Ok(());
    }
    if check_arr_sizes(cfg, env, insn, next) {
        let filled = make_filled_array_insn(cfg, env, insn, next)?;
        replace_insn(cfg, block, i, filled)?;
        remover.add_and_unbind(cfg, next)?;
    }
    Ok(())
}

fn check_arr_sizes(cfg: &MethodCfg, env: &MethodEnv, new_arr: InsnId, fill: InsnId) -> bool {
    let InsnKind::FillArray {
        data: Some(data), ..
    } = &cfg.insn(fill).kind
    else {
        return false;
    };
    let Some(arg) = cfg.insn(new_arr).args.first() else {
        return false;
    };
    match const_literal_by_arg(cfg, env, arg) {
        Some(literal) => i64::from(data.size as i32) == i64::from(literal as i32),
        None => false,
    }
}

/// `InsnUtils.getConstValueByArg`, only literal values are of interest here.
pub(crate) fn const_literal_by_arg(cfg: &MethodCfg, env: &MethodEnv, arg: &InsnArg) -> Option<i64> {
    match arg {
        InsnArg::Literal(l) => Some(l.literal),
        InsnArg::Register(r) => {
            let assign = cfg.svars[r.svar?].assign.parent_insn()?;
            let a = cfg.insn(assign);
            if a.ty == InsnType::Move {
                return const_literal_by_arg(cfg, env, a.args.first()?);
            }
            const_literal_by_insn(cfg, env, assign)
        }
        InsnArg::Wrapped(w) => const_literal_by_insn(cfg, env, w.insn),
        InsnArg::Named(_) => None,
    }
}

/// `InsnUtils.getConstValueByInsn`, literal values only.
fn const_literal_by_insn(cfg: &MethodCfg, env: &MethodEnv, insn: InsnId) -> Option<i64> {
    let n = cfg.insn(insn);
    match (&n.ty, &n.kind) {
        (InsnType::Const, _) => match n.args.first() {
            Some(InsnArg::Literal(l)) => Some(l.literal),
            _ => None,
        },
        (InsnType::Sget, InsnKind::Index(IndexValue::Field(f))) => {
            let value = env.root.resolve_field(f)?.const_value.as_ref()?;
            match value {
                ConstValue::Boolean(b) => Some(i64::from(*b)),
                ConstValue::Byte(v) => Some(i64::from(*v)),
                ConstValue::Short(v) => Some(i64::from(*v)),
                ConstValue::Char(v) => Some(i64::from(*v)),
                ConstValue::Int(v) => Some(i64::from(*v)),
                ConstValue::Long(v) => Some(*v),
                ConstValue::Float(bits) => Some(i64::from(*bits as i32)),
                ConstValue::Double(bits) => Some(*bits as i64),
                ConstValue::Str(_) => None,
            }
        }
        _ => None,
    }
}

/// Return first usage instruction for arg.
/// If used only once try to follow move chain
fn get_first_use_skip_move(cfg: &MethodCfg, var: SsaId) -> Option<InsnId> {
    let uses = &cfg.svars[var].uses;
    if uses.is_empty() {
        return None;
    }
    let parent = uses[0].parent_insn()?;
    if uses.len() == 1 && cfg.insn(parent).ty == InsnType::Move {
        let next = cfg.insn(parent).result.as_ref()?.svar?;
        return get_first_use_skip_move(cfg, next);
    }
    Some(parent)
}

/// `FillArrayData.getLiteralArgs`
pub(crate) fn literal_args(data: &FillArrayData, ty: &ArgType) -> Vec<InsnArg> {
    let values: Vec<i64> = match &data.data {
        ArrayData::Empty => Vec::new(),
        ArrayData::Bytes(v) => v.iter().map(|b| i64::from(*b as i8)).collect(),
        ArrayData::Shorts(v) => v.iter().map(|b| i64::from(*b)).collect(),
        ArrayData::Ints(v) => v.iter().map(|b| i64::from(*b)).collect(),
        ArrayData::Longs(v) => v.clone(),
    };
    values
        .into_iter()
        .map(|v| InsnArg::lit(v, ty.clone()))
        .collect()
}

fn make_filled_array_insn(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    new_array: InsnId,
    fill: InsnId,
) -> Result<InsnNode, String> {
    let InsnKind::NewArray(insn_array_type) = cfg.insn(new_array).kind.clone() else {
        return Err("Not a new array insn".into());
    };
    let InsnKind::FillArray {
        data: Some(data), ..
    } = cfg.insn(fill).kind.clone()
    else {
        return Err("Fill array insn without data".into());
    };
    let insn_element_type = insn_array_type
        .array_element()
        .cloned()
        .ok_or_else(|| format!("Not an array type: {insn_array_type}"))?;
    let mut el_type = data.elem_type.clone();
    if !el_type.is_type_known() {
        if let Some(p) = insn_element_type.primitive_type() {
            if insn_element_type.is_primitive() && el_type.contains(p) {
                el_type = insn_element_type.clone();
            }
        }
    }
    if el_type != insn_element_type && insn_array_type != ArgType::object_type() {
        cfg.warnings.push(format!(
            "Incorrect type for fill-array insn {:x}, element type: {el_type}, insn element type: {insn_element_type}",
            cfg.insn(fill).offset
        ));
    }
    if !el_type.is_type_known() {
        el_type = if insn_element_type.is_type_known() {
            insn_element_type
        } else {
            el_type
                .select_first()
                .ok_or_else(|| "Null array element type".to_string())?
        };
    }
    let list = literal_args(&data, &el_type);
    let mut filled = InsnNode::new(
        InsnType::FilledNewArray,
        InsnKind::FilledNewArray(el_type.clone()),
    );
    filled.result = cfg.insn(new_array).result.clone();
    for arg in list {
        let InsnArg::Literal(l) = &arg else {
            continue;
        };
        match get_const_field_literal(env, l) {
            Some(f) => {
                let sget = InsnNode::new(
                    InsnType::Sget,
                    InsnKind::Index(IndexValue::Field(f.info.clone())),
                );
                let id = cfg.add_insn(sget);
                let wrapped = cfg.wrap_arg(id)?;
                filled.args.push(wrapped);
                add_field_usage(cfg, &f);
            }
            None => filled.args.push(arg),
        }
    }
    Ok(filled)
}

fn remove_check_cast(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    block: BlockId,
    i: usize,
    insn: InsnId,
) -> Result<(), String> {
    let at = ArgRef::Arg(insn, 0);
    if matches!(&cfg.insn(insn).args[0], InsnArg::Literal(l) if l.literal == 0) {
        // always keep cast for 'null'
        cfg.insns[insn].flags |= iflag::EXPLICIT_CAST;
        return Ok(());
    }
    let InsnKind::Index(IndexValue::Type(cast_type)) = cfg.insn(insn).kind.clone() else {
        return Ok(());
    };
    let from = cfg.arg_type(at);
    if !is_cast_needed(env, &from, &cast_type) {
        let Some(var) = cfg.insn(insn).result.as_ref().and_then(|r| r.svar) else {
            return Err("No SSA var for result arg".into());
        };
        cfg.svar_set_type(var, from)?;
        let mut mv = InsnNode::plain(InsnType::Move);
        mv.result = cfg.insn(insn).result.clone();
        mv.args.push(cfg.insn(insn).args[0].clone());
        replace_insn(cfg, block, i, mv)?;
        return Ok(());
    }
    if let Some(prev) = is_cast_duplicate(cfg, insn) {
        // replace previous cast with move
        let prev_res = cfg.insn(prev).result.clone();
        let prev_arg = cfg.insn(prev).args[0].clone();
        let prev_var = prev_res.as_ref().and_then(|r| r.svar);
        let mut mv = InsnNode::plain(InsnType::Move);
        mv.args.push(prev_arg.clone());
        let mv_id = cfg.add_insn(mv);
        // the argument object moves from the previous cast to the move instruction
        if let InsnArg::Register(r) = &prev_arg {
            if let Some(v) = r.svar {
                cfg.svar_remove_use(v, ArgRef::Arg(prev, 0));
                cfg.svar_use(v, ArgRef::Arg(mv_id, 0));
            }
        }
        cfg.set_result(mv_id, prev_res);
        if let (Some(v), Some(r)) = (prev_var, cfg.insns[prev].result.as_mut()) {
            // the result object is shared between both instructions
            r.svar = Some(v);
        }
        replace_insn_by_ref(cfg, block, prev, mv_id)?;
    }
    Ok(())
}

/// `ArgType.isCastNeeded`
pub(crate) fn is_cast_needed(env: &MethodEnv, from: &ArgType, to: &ArgType) -> bool {
    if from == to {
        return false;
    }
    !TypeCompare::new(&env.root.clsp)
        .compare_types(from, to)
        .is_narrow()
}

pub(crate) fn is_cast_duplicate(cfg: &MethodCfg, cast: InsnId) -> Option<InsnId> {
    let InsnArg::Register(r) = cfg.insn(cast).args.first()? else {
        return None;
    };
    let var = r.svar?;
    let v = &cfg.svars[var];
    if v.use_count() != 1 || v.is_used_in_phi() {
        return None;
    }
    let assign = v.assign.parent_insn()?;
    let a = cfg.insn(assign);
    if a.ty != InsnType::CheckCast {
        return None;
    }
    let (InsnKind::Index(IndexValue::Type(assign_type)), InsnKind::Index(IndexValue::Type(ty))) =
        (&a.kind, &cfg.insn(cast).kind)
    else {
        return None;
    };
    (assign_type == ty).then_some(assign)
}

/// Remove unnecessary instructions
fn remove_step(cfg: &mut MethodCfg) -> Result<(), String> {
    for bid in cfg.blocks.clone() {
        let mut remover = Remover::default();
        for iid in cfg.block(bid).insns.clone() {
            let n = cfg.insn(iid);
            if matches!(n.ty, InsnType::Nop | InsnType::Goto | InsnType::NewInstance)
                || n.contains_flag(iflag::REMOVE)
            {
                remover.add_and_unbind(cfg, iid)?;
            }
        }
        remover.perform(cfg, bid)?;
    }
    Ok(())
}

fn iterative_remove_step(cfg: &mut MethodCfg) -> Result<(), String> {
    loop {
        let mut changed = false;
        for bid in cfg.blocks.clone() {
            let found = cfg.block(bid).insns.iter().copied().find(|&i| {
                let n = cfg.insn(i);
                n.ty == InsnType::Move && n.attrs_empty() && is_result_arg_not_used(cfg, i)
            });
            if let Some(i) = found {
                cfg.remove_insn_in_block(bid, i)?;
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
    }
}

fn is_result_arg_not_used(cfg: &MethodCfg, insn: InsnId) -> bool {
    match cfg.insn(insn).result.as_ref().and_then(|r| r.svar) {
        Some(v) => cfg.svars[v].use_count() == 0,
        None => false,
    }
}

/// For args in anonymous constructor invoke apply:
/// - forbid inline into constructor call
/// - make variables final (compiler require this implicitly)
fn process_anonymous_constructor(cfg: &mut MethodCfg, env: &MethodEnv, co: InsnId) {
    let Some(crate::method_utils::MethodDetails::App(c, m)) =
        crate::method_utils::insn_method_details(env.root, cfg.insn(co))
    else {
        return;
    };
    let call_mth = &env.root.classes[c].methods[m];
    if !call_mth.anonymous_constructor || call_mth.no_skip_args {
        return;
    }
    let args_count = cfg.insn(co).args.len();
    match &call_mth.skip_args {
        Some(skip) => {
            let count = env.root.classes[c].methods[m]
                .info
                .args_count()
                .min(args_count);
            for i in 0..count {
                if skip.get(i).copied().unwrap_or(false) {
                    anonymous_call_arg_mod(cfg, co, i);
                }
            }
        }
        None => {
            // additional info not available apply mods to all args (the safest solution)
            for i in 0..args_count {
                anonymous_call_arg_mod(cfg, co, i);
            }
        }
    }
}

fn anonymous_call_arg_mod(cfg: &mut MethodCfg, insn: InsnId, k: usize) {
    if let InsnArg::Register(r) = &mut cfg.insns[insn].args[k] {
        r.dont_inline = true;
        if let Some(cv) = r.svar.and_then(|v| cfg.svars[v].code_var) {
            cfg.code_vars[cv].is_final = true;
        }
    }
}

fn process_move_exception(
    cfg: &mut MethodCfg,
    block: BlockId,
    insn: InsnId,
    remover: &mut Remover,
) -> Result<(), String> {
    let Some(handler) = cfg.block(block).exc_handler else {
        return Ok(());
    };
    // result arg used both in this insn and exception handler,
    let Some(res) = cfg.insn(insn).result.clone() else {
        return Ok(());
    };
    let ty = cfg.handlers[handler].arg_type();
    let name = if cfg.handlers[handler].is_catch_all() {
        "th"
    } else {
        "e"
    };
    let Some(var) = res.svar else {
        return Err("No SSA var for result arg".into());
    };
    if reg_name(cfg, &res).is_none() {
        svar_set_name(cfg, var, name)?;
    }
    if cfg.svars[var].use_count() == 0 {
        cfg.handlers[handler].named_arg = Some((name.to_string(), ty));
        remover.add_and_unbind(cfg, insn)?;
    } else if cfg.svars[var].is_used_in_phi() {
        // exception var moved to external variable => replace with 'move' insn
        let mut mv = InsnNode::plain(InsnType::Move);
        mv.result = Some(res);
        mv.args.push(InsnArg::Named(NamedArg {
            name: name.to_string(),
            ty: ty.clone(),
        }));
        cfg.handlers[handler].named_arg = Some((name.to_string(), ty));
        replace_insn(cfg, block, 0, mv)?;
    }
    Ok(())
}
