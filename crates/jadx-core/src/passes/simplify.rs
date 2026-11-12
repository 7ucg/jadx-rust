//! `SimplifyVisitor`: simplify instructions (string concatenation, arithmetic, casts, conditions).

use crate::model::cfg::MethodCfg;
use crate::model::condition::{is_float_cmp, CondMode};
use crate::model::info::MethodInfo;
use crate::model::insn::{
    iflag, ArithOp, BlockId, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType, InvokeType,
    LiteralArg, RegisterArg,
};
use crate::model::ssa::ArgRef;
use crate::model::types::{ArgType, PrimitiveType};
use crate::passes::constructor::MethodEnv;
use crate::passes::insn_utils::{add_arg, insn_index, same_reg_and_svar};
use crate::passes::mod_visitor::{is_cast_duplicate, is_cast_needed};
use crate::type_compare::TypeCompare;

const CLASS_STRING_BUILDER: &str = "java.lang.StringBuilder";
const MTH_TOSTRING_SIGNATURE: &str = "toString()Ljava/lang/String;";

pub fn simplify(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    let mut changed = false;
    for block in cfg.blocks.clone() {
        if simplify_block(cfg, env, block)? {
            changed = true;
        }
    }
    if changed || cfg.regions.request_code_shrink {
        crate::passes::regions::shrink_method(cfg)?;
    }
    Ok(())
}

fn simplify_block(cfg: &mut MethodCfg, env: &MethodEnv, block: BlockId) -> Result<bool, String> {
    let mut changed = false;
    let mut i = 0;
    while i < cfg.block(block).insns.len() {
        let insn = cfg.block(block).insns[i];
        let insn_count = cfg.block(block).insns.len();
        if let Some(mod_insn) = simplify_insn(cfg, env, insn, None)? {
            if i < cfg.block(block).insns.len() && cfg.block(block).insns[i] == insn {
                cfg.arena[block].insns[i] = mod_insn;
            } else {
                let idx = insn_index(cfg, block, insn)
                    .ok_or_else(|| "Failed to replace insn".to_string())?;
                cfg.arena[block].insns[idx] = mod_insn;
            }
            cfg.unbind_insn(insn)?;
            cfg.rebind_args(mod_insn)?;
            if cfg.block(block).insns.len() < insn_count {
                // some insns removed => restart block processing
                simplify_block(cfg, env, block)?;
                return Ok(true);
            }
            changed = true;
        }
        i += 1;
    }
    Ok(changed)
}

fn simplify_args(cfg: &mut MethodCfg, env: &MethodEnv, insn: InsnId) -> Result<(), String> {
    let mut changed = false;
    let count = cfg.insn(insn).args.len();
    for k in 0..count {
        let InsnArg::Wrapped(w) = &cfg.insn(insn).args[k] else {
            continue;
        };
        let wrap_insn = w.insn;
        if let Some(replace) = simplify_insn(cfg, env, wrap_insn, Some(insn))? {
            // an argument object shared with another instruction is replaced in its parent
            let (parent, index) = resolve_alias(cfg, insn, k);
            crate::passes::insn_utils::wrap_instruction_in_place(cfg, parent, index, replace)?;
            changed = true;
        }
    }
    if changed {
        crate::passes::insn_utils::rebind_args(cfg, insn)?;
        cfg.regions.request_code_shrink = true;
    }
    Ok(())
}

fn resolve_alias(cfg: &MethodCfg, insn: InsnId, index: usize) -> (InsnId, usize) {
    let mut loc = (insn, index);
    let mut steps = 0;
    while let Some(next) = cfg.arg_alias.get(&loc) {
        loc = *next;
        steps += 1;
        if steps > 16 {
            break;
        }
    }
    loc
}

fn simplify_insn(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    insn: InsnId,
    parent: Option<InsnId>,
) -> Result<Option<InsnId>, String> {
    if cfg.insn(insn).contains_flag(iflag::DONT_GENERATE) {
        return Ok(None);
    }
    simplify_args(cfg, env, insn)?;
    match cfg.insn(insn).ty {
        InsnType::Arith => Ok(simplify_arith(cfg, insn)),
        InsnType::If => {
            simplify_if(cfg, insn)?;
            Ok(None)
        }
        InsnType::Ternary => {
            simplify_ternary(cfg, insn)?;
            Ok(None)
        }
        InsnType::Invoke => convert_invoke(cfg, env, insn),
        InsnType::Iput | InsnType::Sput => convert_field_arith(cfg, insn),
        InsnType::Cast | InsnType::CheckCast => process_cast(cfg, env, insn, parent),
        InsnType::Move => {
            if let Some(InsnArg::Literal(l)) = cfg.insn(insn).args.first().cloned() {
                let mut const_insn = InsnNode::plain(InsnType::Const);
                const_insn.args.push(InsnArg::Literal(l));
                const_insn.copy_attributes_from(cfg.insn(insn));
                let id = cfg.add_insn(const_insn);
                let res = cfg.insn(insn).result.clone();
                cfg.set_result(id, res);
                return Ok(Some(id));
            }
            Ok(None)
        }
        InsnType::Constructor => simplify_string_constructor(cfg, env, insn),
        _ => Ok(None),
    }
}

fn simplify_string_constructor(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    insn: InsnId,
) -> Result<Option<InsnId>, String> {
    let n = cfg.insn(insn);
    let InsnKind::Constructor { call_mth, .. } = &n.kind else {
        return Ok(None);
    };
    if *call_mth.decl_class.arg_type() != ArgType::string() || n.args.is_empty() {
        return Ok(None);
    }
    let InsnArg::Wrapped(w) = &n.args[0] else {
        return Ok(None);
    };
    let arr_insn = w.insn;
    let a = cfg.insn(arr_insn);
    if a.ty != InsnType::FilledNewArray || a.args.is_empty() {
        return Ok(None);
    }
    let InsnKind::FilledNewArray(elem_type) = &a.kind else {
        return Ok(None);
    };
    if *elem_type != ArgType::byte() && *elem_type != ArgType::char() {
        return Ok(None);
    }
    let mut printable = 0usize;
    let mut arr: Vec<u8> = Vec::with_capacity(a.args.len());
    for arg in &a.args {
        let InsnArg::Literal(l) = arg else {
            return Ok(None);
        };
        let b = l.literal as u8;
        arr.push(b);
        // `(char) arr[i]` sign extends the byte
        let c = i16::from(b as i8) as u16;
        if (32..127).contains(&c) {
            printable += 1;
        }
    }
    if printable < arr.len() - printable {
        return Ok(None);
    }
    let text: Vec<u16> = String::from_utf8_lossy(&arr).encode_utf16().collect();
    let mut const_str = InsnNode::new(InsnType::ConstStr, InsnKind::ConstStr(text));
    if n.args.len() == 1 {
        const_str.copy_attributes_from(n);
        let id = cfg.add_insn(const_str);
        let res = cfg.insn(insn).result.clone();
        cfg.set_result(id, res);
        cfg.unbind_arg_usage(insn, 0);
        return Ok(Some(id));
    }
    // `new String(bytes, charset)`: use `"str".getBytes()` as the bytes argument
    let string_info = env.root.ctx.class_info(&ArgType::string())?;
    let get_bytes = MethodInfo {
        decl_class: string_info,
        name: "getBytes".to_string(),
        arg_types: Vec::new(),
        return_type: ArgType::array(ArgType::byte()),
    };
    let cs_id = cfg.add_insn(const_str);
    let cs_arg = cfg.wrap_arg(cs_id)?;
    let mut invoke = InsnNode::new(
        InsnType::Invoke,
        InsnKind::Invoke {
            invoke_type: InvokeType::Virtual,
            mth: get_bytes.clone(),
        },
    );
    invoke.args.push(cs_arg);
    let inv_id = cfg.add_insn(invoke);
    let bytes_arg = cfg.wrap_arg(inv_id)?;
    let bytes_arg = match bytes_arg {
        InsnArg::Wrapped(mut w) => {
            w.ty = get_bytes.return_type;
            InsnArg::Wrapped(w)
        }
        other => other,
    };
    cfg.replace_arg(insn, 0, bytes_arg);
    Ok(None)
}

fn process_cast(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    cast: InsnId,
    parent: Option<InsnId>,
) -> Result<Option<InsnId>, String> {
    if cfg.insn(cast).contains_flag(iflag::EXPLICIT_CAST) {
        return Ok(None);
    }
    let cast_arg_ty = cfg.arg_type(ArgRef::Arg(cast, 0));
    let mut arg_type = cast_arg_ty;
    // Don't removes CHECK_CAST for wrapped INVOKE if invoked method returns different type
    if let InsnArg::Wrapped(w) = &cfg.insn(cast).args[0] {
        let wi = cfg.insn(w.insn);
        if wi.ty == InsnType::Invoke {
            if let InsnKind::Invoke { mth, .. } = &wi.kind {
                arg_type = mth.return_type.clone();
            }
        }
    }
    let InsnKind::Index(IndexValue::Type(cast_to_type)) = cfg.insn(cast).kind.clone() else {
        return Ok(None);
    };
    if is_arith_wide_up_cast(cfg, parent, &arg_type, &cast_to_type) {
        return Ok(None);
    }
    if !is_cast_needed(env, &arg_type, &cast_to_type)
        || is_cast_duplicate(cfg, cast).is_some()
        || shadowed_by_outer_cast(cfg, env, &cast_to_type, parent)
    {
        let mut mv = InsnNode::plain(InsnType::Move);
        mv.offset = cfg.insn(cast).offset;
        let arg = cfg.insn(cast).args[0].clone();
        let dup = cfg.duplicate_arg(&arg);
        let id = cfg.add_insn(mv);
        add_arg(cfg, id, dup);
        let res = cfg.insn(cast).result.clone();
        cfg.set_result(id, res);
        return Ok(Some(id));
    }
    Ok(None)
}

/// Keep cast to wide types in arith instructions, because arguments type determine instruction
/// used in result bytecode.
fn is_arith_wide_up_cast(
    cfg: &MethodCfg,
    parent: Option<InsnId>,
    arg_type: &ArgType,
    cast_to_type: &ArgType,
) -> bool {
    if let Some(p) = parent {
        if cfg.insn(p).ty == InsnType::Arith
            && arg_type.is_primitive()
            && cast_to_type.is_primitive()
        {
            return cast_to_type.reg_count() > arg_type.reg_count();
        }
    }
    false
}

fn shadowed_by_outer_cast(
    cfg: &MethodCfg,
    env: &MethodEnv,
    cast_type: &ArgType,
    parent: Option<InsnId>,
) -> bool {
    if let Some(p) = parent {
        if cfg.insn(p).ty == InsnType::Cast {
            if let InsnKind::Index(IndexValue::Type(parent_cast_type)) = &cfg.insn(p).kind {
                return TypeCompare::new(&env.root.clsp)
                    .compare_types(parent_cast_type, cast_type)
                    .is_narrow();
            }
        }
    }
    false
}

/// Simplify 'cmp' instruction in if condition
fn simplify_if(cfg: &mut MethodCfg, insn: InsnId) -> Result<(), String> {
    let n = cfg.insn(insn);
    let Some(InsnArg::Wrapped(w)) = n.args.first() else {
        return Ok(());
    };
    let wi = w.insn;
    if !matches!(cfg.insn(wi).ty, InsnType::CmpL | InsnType::CmpG) {
        return Ok(());
    }
    let zero = matches!(n.args.get(1), Some(InsnArg::Literal(l)) if l.literal == 0);
    if zero {
        if is_float_cmp(cfg, wi) {
            // keep 'cmp' to not lose NaN handling
            return Ok(());
        }
        let a0 = cfg.insn(wi).args[0].clone();
        let a1 = cfg.insn(wi).args[1].clone();
        let d0 = cfg.duplicate_arg(&a0);
        let d1 = cfg.duplicate_arg(&a1);
        cfg.replace_arg(insn, 0, d0);
        cfg.replace_arg(insn, 1, d1);
        cfg.unbind_insn(wi)?;
    } else {
        cfg.warnings.push(format!("TODO: cmp {insn}"));
    }
    Ok(())
}

/// Simplify condition in ternary operation
fn simplify_ternary(cfg: &mut MethodCfg, insn: InsnId) -> Result<(), String> {
    match cfg.insn(insn).kind.clone() {
        InsnKind::Ternary { cond } => simplify_if(cfg, cond),
        InsnKind::TernaryCond(c) => {
            if c.mode == CondMode::Compare {
                match c.compare {
                    Some(i) => simplify_if(cfg, i),
                    None => Ok(()),
                }
            } else {
                crate::passes::regions::ternary_mod::simplify_ternary_condition(cfg, insn);
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

/// Simplify chains of calls to `StringBuilder#append()` plus constructor of `StringBuilder`.
fn convert_invoke(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    insn: InsnId,
) -> Result<Option<InsnId>, String> {
    let InsnKind::Invoke { mth, .. } = &cfg.insn(insn).kind else {
        return Ok(None);
    };
    if mth.decl_class.full_name() != CLASS_STRING_BUILDER
        || mth.short_id() != MTH_TOSTRING_SIGNATURE
    {
        return Ok(None);
    }
    let Some(instance) = cfg.insn(insn).args.first().cloned() else {
        return Err("no instance argument".to_string());
    };
    match instance {
        InsnArg::Wrapped(_) => {
            // Convert 'new StringBuilder(xxx).append(yyy).append(zzz).toString()' to STR_CONCAT insn
            let chain = flatten_insn_chain_until(cfg, insn, InsnType::Constructor);
            Ok(convert_string_builder_chain(cfg, env, insn, &chain))
        }
        InsnArg::Register(r) => {
            // Convert 'StringBuilder sb = new StringBuilder(xxx); sb.append(yyy); String str = sb.toString();'
            let chain = collect_use_chain(cfg, insn, &r);
            Ok(convert_string_builder_chain(cfg, env, insn, &chain))
        }
        _ => Ok(None),
    }
}

fn collect_use_chain(cfg: &MethodCfg, insn: InsnId, instance: &RegisterArg) -> Vec<InsnId> {
    let Some(var) = instance.svar else {
        return Vec::new();
    };
    let sv = &cfg.svars[var];
    if sv.is_used_in_phi() || sv.use_count() == 0 {
        return Vec::new();
    }
    let mut use_chain: Vec<InsnId> = Vec::with_capacity(sv.use_count() + 1);
    let Some(assign_insn) = sv.assign.parent_insn() else {
        return Vec::new();
    };
    use_chain.push(assign_insn);
    for u in &sv.uses {
        let Some(p) = u.parent_insn() else {
            return Vec::new();
        };
        use_chain.push(p);
    }
    let Some(to_str_idx) = use_chain.iter().position(|i| *i == insn) else {
        return Vec::new();
    };
    if use_chain.len() - 1 != to_str_idx {
        return Vec::new();
    }
    use_chain.remove(to_str_idx);

    // all insns must be in one block and sequential
    let Some(assign_block) = cfg.block_by_insn(assign_insn) else {
        return Vec::new();
    };
    let block_insns = &cfg.block(assign_block).insns;
    let Some(assign_idx) = block_insns.iter().position(|i| *i == assign_insn) else {
        return Vec::new();
    };
    let chain_size = use_chain.len();
    let last_insn = block_insns.len() - assign_idx;
    if last_insn < chain_size {
        return Vec::new();
    }
    for (i, chain_insn) in use_chain.iter().enumerate().skip(1) {
        if block_insns[assign_idx + i] != *chain_insn {
            return Vec::new();
        }
    }
    use_chain
}

fn convert_string_builder_chain(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    to_str: InsnId,
    chain: &[InsnId],
) -> Option<InsnId> {
    match try_convert_chain(cfg, env, to_str, chain) {
        Ok(r) => r,
        Err(e) => {
            cfg.warnings
                .push(format!("String concatenation convert failed: {e}"));
            None
        }
    }
}

fn try_convert_chain(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    to_str: InsnId,
    chain: &[InsnId],
) -> Result<Option<InsnId>, String> {
    let chain_size = chain.len();
    if chain_size < 2 {
        return Ok(None);
    }
    let mut args: Vec<InsnArg> = Vec::with_capacity(chain_size);
    let first = chain[0];
    if cfg.insn(first).ty != InsnType::Constructor {
        return Ok(None);
    }
    let constr = cfg.insn(first);
    if constr.args.len() == 1 {
        let InsnKind::Constructor { call_mth, .. } = &constr.kind else {
            return Ok(None);
        };
        let arg_type = call_mth
            .arg_types
            .first()
            .ok_or_else(|| "no constructor argument type".to_string())?;
        if !arg_type.is_object() {
            return Ok(None);
        }
        args.push(constr.args[0].clone());
    }
    for &chain_insn in &chain[1..] {
        let Some(arg) = get_arg_from_append(cfg, chain_insn) else {
            return Ok(None);
        };
        args.push(arg);
    }

    // all check passed
    let dup_args: Vec<InsnArg> = args.iter().map(|a| cfg.duplicate_arg(a)).collect();
    let simplified_args = concat_const_args(cfg, dup_args)?;
    let mut concat = InsnNode::plain(InsnType::StrConcat);
    concat.flags |= iflag::SYNTHETIC;
    let concat_id = cfg.add_insn(concat);
    for a in simplified_args {
        add_arg(cfg, concat_id, a);
    }
    let to_str_n = cfg.insn(to_str);
    if to_str_n.result.is_none() && !to_str_n.contains_flag(iflag::WRAPPED) {
        // string concat without assign to variable will cause compilation error
        let res = cfg.make_synthetic_reg_arg(ArgType::string())?;
        cfg.set_result(concat_id, Some(res));
    } else {
        let res = to_str_n.result.clone();
        cfg.set_result(concat_id, res);
    }
    let to_str_copy = cfg.insn(to_str).clone();
    cfg.insns[concat_id].copy_attributes_from(&to_str_copy);
    remove_string_builder_insns(cfg, to_str, chain)?;
    let _ = env;
    Ok(Some(concat_id))
}

fn is_const_concat_needed(cfg: &MethodCfg, args: &[InsnArg]) -> bool {
    let mut prev_const = false;
    for a in args {
        let cur_const = is_const_arg(cfg, a);
        if cur_const && prev_const {
            // found 2 consecutive constants
            return true;
        }
        prev_const = cur_const;
    }
    false
}

/// `InsnArg.isConst`
fn is_const_arg(cfg: &MethodCfg, a: &InsnArg) -> bool {
    match a {
        InsnArg::Literal(_) => true,
        InsnArg::Wrapped(w) => crate::passes::insn_utils::is_const_insn(cfg.insn(w.insn).ty),
        _ => false,
    }
}

fn concat_const_args(cfg: &mut MethodCfg, args: Vec<InsnArg>) -> Result<Vec<InsnArg>, String> {
    if !is_const_concat_needed(cfg, &args) {
        return Ok(args);
    }
    let size = args.len();
    let mut new_args: Vec<InsnArg> = Vec::with_capacity(size);
    let mut concat_list: Vec<Vec<u16>> = Vec::with_capacity(size);
    for i in 0..size {
        let const_str = get_const_string(cfg, &args[i]);
        match const_str {
            Some(s) => concat_list.push(s),
            None => {
                if !concat_list.is_empty() {
                    new_args.push(get_concat_arg(cfg, &concat_list, &args, i)?);
                    concat_list.clear();
                }
                new_args.push(args[i].clone());
            }
        }
    }
    if !concat_list.is_empty() {
        new_args.push(get_concat_arg(cfg, &concat_list, &args, size)?);
    }
    Ok(new_args)
}

fn get_concat_arg(
    cfg: &mut MethodCfg,
    concat_list: &[Vec<u16>],
    args: &[InsnArg],
    idx: usize,
) -> Result<InsnArg, String> {
    if concat_list.len() == 1 {
        return Ok(args[idx - 1].clone());
    }
    let joined: Vec<u16> = concat_list.iter().flatten().copied().collect();
    let id = cfg.add_insn(InsnNode::new(
        InsnType::ConstStr,
        InsnKind::ConstStr(joined),
    ));
    cfg.wrap_arg(id)
}

fn get_const_string(cfg: &MethodCfg, arg: &InsnArg) -> Option<Vec<u16>> {
    match arg {
        InsnArg::Literal(l) => crate::codegen::type_gen::literal_to_raw_string(l.literal, &l.ty)
            .map(|s| s.encode_utf16().collect()),
        InsnArg::Wrapped(w) => match (&cfg.insn(w.insn).ty, &cfg.insn(w.insn).kind) {
            (InsnType::ConstStr, InsnKind::ConstStr(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Remove and unbind all instructions with StringBuilder
fn remove_string_builder_insns(
    cfg: &mut MethodCfg,
    to_str: InsnId,
    chain: &[InsnId],
) -> Result<(), String> {
    cfg.unbind_all_args(to_str);
    for &i in chain {
        cfg.unbind_all_args(i);
    }
    let mut to_remove: Vec<InsnId> = Vec::new();
    for &i in chain {
        if i != to_str {
            cfg.unbind_insn(i)?;
            to_remove.push(i);
        }
    }
    for i in to_remove {
        cfg.remove_insn(i)?;
    }
    Ok(())
}

fn flatten_insn_chain_until(cfg: &MethodCfg, insn: InsnId, until: InsnType) -> Vec<InsnId> {
    let mut chain = Vec::new();
    let mut arg = cfg.insn(insn).args.first().cloned();
    while let Some(InsnArg::Wrapped(w)) = arg {
        let wi = w.insn;
        chain.push(wi);
        if cfg.insn(wi).ty == until || cfg.insn(wi).args.is_empty() {
            break;
        }
        arg = cfg.insn(wi).args.first().cloned();
    }
    chain.reverse();
    chain
}

fn get_arg_from_append(cfg: &MethodCfg, chain_insn: InsnId) -> Option<InsnArg> {
    let n = cfg.insn(chain_insn);
    if n.ty == InsnType::Invoke && n.args.len() == 2 {
        if let InsnKind::Invoke { mth, .. } = &n.kind {
            if mth.decl_class.full_name() == CLASS_STRING_BUILDER && mth.name == "append" {
                return Some(n.args[1].clone());
            }
        }
    }
    None
}

fn simplify_arith(cfg: &mut MethodCfg, arith: InsnId) -> Option<InsnId> {
    let n = cfg.insn(arith);
    if n.args.len() != 2 {
        return None;
    }
    let lit = match &n.args[1] {
        InsnArg::Wrapped(w) => {
            let wr = cfg.insn(w.insn);
            if wr.ty == InsnType::Const {
                match wr.args.first() {
                    Some(InsnArg::Literal(l)) => Some(l.clone()),
                    _ => None,
                }
            } else {
                None
            }
        }
        InsnArg::Literal(l) => Some(l.clone()),
        _ => None,
    }?;
    let InsnKind::Arith(op) = n.kind else {
        return None;
    };
    match op {
        ArithOp::Add => {
            // fix 'c + (-1)' to 'c - (1)'
            if is_negative(&lit) {
                let neg = negate(&lit)?;
                let first = cfg.insn(arith).args[0].clone();
                let first_dup = cfg.duplicate_arg(&first);
                let mut new_insn = InsnNode::new(InsnType::Arith, InsnKind::Arith(ArithOp::Sub));
                new_insn.copy_attributes_from(cfg.insn(arith));
                new_insn.offset = cfg.insn(arith).offset;
                let id = cfg.add_insn(new_insn);
                let res = cfg.insn(arith).result.clone();
                cfg.set_result(id, res);
                add_arg(cfg, id, first_dup);
                add_arg(cfg, id, InsnArg::Literal(neg));
                return Some(id);
            }
        }
        ArithOp::Xor => {
            // simplify xor on boolean
            let first = cfg.insn(arith).args[0].clone();
            let first_ty = match &first {
                InsnArg::Register(_) => cfg.arg_type(ArgRef::Arg(arith, 0)),
                InsnArg::Literal(l) => l.ty.clone(),
                InsnArg::Wrapped(w) => w.ty.clone(),
                InsnArg::Named(n) => n.ty.clone(),
            };
            if first_ty == ArgType::boolean() && (lit.literal == 0 || lit.literal == 1) {
                let ty = if lit.literal == 0 {
                    InsnType::Move
                } else {
                    InsnType::Not
                };
                let mut new_insn = InsnNode::plain(ty);
                new_insn.copy_attributes_from(cfg.insn(arith));
                new_insn.offset = cfg.insn(arith).offset;
                let id = cfg.add_insn(new_insn);
                let res = cfg.insn(arith).result.clone();
                cfg.set_result(id, res);
                let dup = cfg.duplicate_arg(&first);
                add_arg(cfg, id, dup);
                return Some(id);
            }
        }
        _ => {}
    }
    None
}

/// `LiteralArg.isInteger`
fn is_integer(ty: &ArgType) -> bool {
    ty.is_primitive()
        && matches!(
            ty.primitive_type(),
            Some(
                PrimitiveType::Int
                    | PrimitiveType::Byte
                    | PrimitiveType::Char
                    | PrimitiveType::Short
                    | PrimitiveType::Long
            )
        )
}

/// `LiteralArg.isNegative`
fn is_negative(l: &LiteralArg) -> bool {
    if is_integer(&l.ty) {
        return l.literal < 0;
    }
    if l.ty == ArgType::float() {
        let v = f32::from_bits(l.literal as u32);
        return v < 0.0 && v.is_finite();
    }
    if l.ty == ArgType::double() {
        let v = f64::from_bits(l.literal as u64);
        return v < 0.0 && v.is_finite();
    }
    false
}

/// `LiteralArg.negate`
fn negate(l: &LiteralArg) -> Option<LiteralArg> {
    let neg = if is_integer(&l.ty) {
        l.literal.wrapping_neg()
    } else if l.ty == ArgType::float() {
        i64::from((-f32::from_bits(l.literal as u32)).to_bits() as i32)
    } else if l.ty == ArgType::double() {
        (-f64::from_bits(l.literal as u64)).to_bits() as i64
    } else {
        return None;
    };
    Some(LiteralArg {
        literal: neg,
        ty: l.ty.clone(),
        explicit_primitive: false,
    })
}

/// Convert field arith operation to arith instruction
/// (`IPUT (ARITH (IGET, lit))` -> `ARITH ((IGET)) <op>= lit)`).
fn convert_field_arith(cfg: &mut MethodCfg, insn: InsnId) -> Result<Option<InsnId>, String> {
    let Some(InsnArg::Wrapped(w)) = cfg.insn(insn).args.first().cloned() else {
        return Ok(None);
    };
    let wrap = w.insn;
    let wrap_type = cfg.insn(wrap).ty;
    if (wrap_type != InsnType::Arith && wrap_type != InsnType::StrConcat)
        || !matches!(cfg.insn(wrap).args.first(), Some(InsnArg::Wrapped(_)))
    {
        return Ok(None);
    }
    let Some(InsnArg::Wrapped(get_wrap)) = cfg.insn(wrap).args.first().cloned() else {
        return Ok(None);
    };
    let get = get_wrap.insn;
    let get_type = cfg.insn(get).ty;
    if get_type != InsnType::Iget && get_type != InsnType::Sget {
        return Ok(None);
    }
    let (InsnKind::Index(IndexValue::Field(field)), InsnKind::Index(IndexValue::Field(inner))) =
        (&cfg.insn(insn).kind, &cfg.insn(get).kind)
    else {
        return Ok(None);
    };
    if field != inner {
        return Ok(None);
    }
    if get_type == InsnType::Iget && cfg.insn(insn).ty == InsnType::Iput {
        let (Some(InsnArg::Register(reg)), Some(InsnArg::Register(put_reg))) =
            (cfg.insn(get).args.first(), cfg.insn(insn).args.get(1))
        else {
            return Ok(None);
        };
        if !same_reg_and_svar(reg, put_reg) {
            return Ok(None);
        }
    }
    let f_arg = cfg.duplicate_arg(&InsnArg::Wrapped(get_wrap));
    cfg.unbind_insn(get)?;
    if cfg.insn(insn).ty == InsnType::Iput {
        cfg.unbind_arg_usage(insn, 1);
    }
    if wrap_type == InsnType::Arith {
        let InsnKind::Arith(op) = cfg.insn(wrap).kind else {
            return Ok(None);
        };
        let second = cfg.insn(wrap).args[1].clone();
        let second = cfg.duplicate_arg(&second);
        let id = one_arg_op(cfg, op, f_arg, second);
        let copy = cfg.insn(insn).clone();
        cfg.insns[id].copy_attributes_from(&copy);
        cfg.insns[id].offset = copy.offset;
        return Ok(Some(id));
    }
    let args_count = cfg.insn(wrap).args.len();
    let concat_id = cfg.add_insn(InsnNode::plain(InsnType::StrConcat));
    for i in 1..args_count {
        let a = cfg.insn(wrap).args[i].clone();
        let dup = cfg.duplicate_arg(&a);
        add_arg(cfg, concat_id, dup);
    }
    let concat_arg = cfg.wrap_arg(concat_id)?;
    let concat_arg = match concat_arg {
        InsnArg::Wrapped(mut w) => {
            w.ty = ArgType::string();
            InsnArg::Wrapped(w)
        }
        other => other,
    };
    let id = one_arg_op(cfg, ArithOp::Add, f_arg, concat_arg);
    let copy = cfg.insn(wrap).clone();
    cfg.insns[id].copy_attributes_from(&copy);
    cfg.insns[id].offset = copy.offset;
    Ok(Some(id))
}

/// `ArithNode.oneArgOp`
fn one_arg_op(cfg: &mut MethodCfg, op: ArithOp, res: InsnArg, a: InsnArg) -> InsnId {
    let mut n = InsnNode::new(InsnType::Arith, InsnKind::Arith(op));
    n.flags |= iflag::ARITH_ONEARG;
    let id = cfg.add_insn(n);
    add_arg(cfg, id, res);
    add_arg(cfg, id, a);
    id
}
