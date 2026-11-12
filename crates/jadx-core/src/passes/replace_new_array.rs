//! `ReplaceNewArray`: replace `new-array` followed by a sequence of `aput` with a filled array.

use std::collections::BTreeMap;

use crate::model::cfg::MethodCfg;
use crate::model::insn::{
    iflag, BlockId, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType, RegisterArg,
};
use crate::passes::constructor::MethodEnv;
use crate::passes::insn_utils::{
    add_arg, arg_contains_var, insn_index, same_reg_and_svar, Remover,
};
use crate::passes::mod_visitor::{add_field_usage, const_literal_by_arg, get_const_field_literal};

pub fn replace_new_array(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    if !cfg.has_new_array {
        return Ok(());
    }
    let mut k = 0;
    loop {
        let mut changed = false;
        for block in cfg.blocks.clone() {
            let mut remover = Remover::default();
            let size = cfg.block(block).insns.len();
            for i in 0..size {
                changed |= process_insn(cfg, env, block, i, &mut remover)?;
            }
            remover.perform(cfg, block)?;
        }
        if changed {
            crate::passes::regions::shrink_method(cfg)?;
        } else {
            break;
        }
        k += 1;
        if k > 100 {
            cfg.warnings
                .push("Reached limit for ReplaceNewArray iterations".to_string());
            break;
        }
    }
    Ok(())
}

fn process_insn(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    block: BlockId,
    i: usize,
    remover: &mut Remover,
) -> Result<bool, String> {
    let insn = cfg.block(block).insns[i];
    let n = cfg.insn(insn);
    if n.ty == InsnType::NewArray && !n.contains_flag(iflag::REMOVE) {
        return process_new_array(cfg, env, block, insn, remover);
    }
    Ok(false)
}

fn process_new_array(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    block: BlockId,
    new_array: InsnId,
    remover: &mut Remover,
) -> Result<bool, String> {
    let Some(len_arg) = cfg.insn(new_array).args.first() else {
        return Ok(false);
    };
    let Some(len) = const_literal_by_arg(cfg, env, len_arg) else {
        return Ok(false);
    };
    let len = len as i32;
    if len == 0 {
        return Ok(false);
    }
    let InsnKind::NewArray(arr_type) = cfg.insn(new_array).kind.clone() else {
        return Ok(false);
    };
    let Some(elem_type) = arr_type.array_element().cloned() else {
        return Ok(false);
    };
    let allow_missing_keys = arr_type.array_dimension() == 1 && elem_type.is_primitive();
    let min_len = if allow_missing_keys { len / 2 } else { len };

    let Some(arr_reg) = cfg.insn(new_array).result.clone() else {
        return Ok(false);
    };
    let Some(var) = arr_reg.svar else {
        return Ok(false);
    };
    let use_list = cfg.svars[var].uses.clone();
    if (use_list.len() as i64) < i64::from(min_len) {
        return Ok(false);
    }
    // quick check if APUT is used
    let found_put = use_list.iter().any(|u| {
        u.parent_insn()
            .is_some_and(|p| cfg.insn(p).ty == InsnType::Aput)
    });
    if !found_put {
        return Ok(false);
    }
    // collect put instructions sorted by array index
    let mut arr_puts: BTreeMap<i64, InsnId> = BTreeMap::new();
    let mut first_not_a_put_usage: Option<InsnId> = None;
    for u in &use_list {
        let parent = u.parent_insn();
        let is_put = parent.is_some_and(|p| {
            cfg.insn(p).ty == InsnType::Aput
                && matches!(&cfg.insn(p).args[0], InsnArg::Register(r) if same_reg_and_svar(&arr_reg, r))
        });
        if !is_put {
            if first_not_a_put_usage.is_none() {
                first_not_a_put_usage = parent;
            }
            continue;
        }
        let parent = parent.expect("put instruction");
        let Some(index) = const_literal_by_arg(cfg, env, &cfg.insn(parent).args[1]) else {
            return Ok(false);
        };
        if index >= i64::from(len) {
            return Ok(false);
        }
        if arr_puts.contains_key(&index) {
            // stop on index rewrite
            break;
        }
        arr_puts.insert(index, parent);
    }
    if (arr_puts.len() as i64) < i64::from(min_len) {
        return Ok(false);
    }
    if !verify_put_insns(cfg, block, &arr_reg, &arr_puts) {
        return Ok(false);
    }

    // checks complete, apply
    let mut filled = InsnNode::new(
        InsnType::FilledNewArray,
        InsnKind::FilledNewArray(elem_type.clone()),
    );
    filled.copy_attributes_from(cfg.insn(new_array));
    filled.offset = cfg.insn(new_array).offset;
    let filled_id = cfg.add_insn(filled);
    // the result is a duplicate of the array register, the variable is assigned by the new insn
    cfg.set_result(filled_id, Some(arr_reg.clone()));
    cfg.inherit_metadata(filled_id, new_array);

    let mut prev_index: i64 = -1;
    for (&index, &put) in &arr_puts {
        if index != prev_index {
            // use zero for missing keys
            let mut j = prev_index + 1;
            while j < index {
                add_arg(cfg, filled_id, InsnArg::lit(0, elem_type.clone()));
                j += 1;
            }
        }
        let value = cfg.insn(put).args[2].clone();
        let repl = replace_const_in_arg(cfg, env, &value);
        add_arg(cfg, filled_id, repl);
        remover.add_and_unbind(cfg, put)?;
        prev_index = index;
    }
    // add missing trailing zeros
    let mut j = prev_index + 1;
    while j < i64::from(len) {
        add_arg(cfg, filled_id, InsnArg::lit(0, elem_type.clone()));
        j += 1;
    }
    remover.add_and_unbind(cfg, new_array)?;

    // place new insn at last array put or before first usage
    let last_put = *arr_puts.values().next_back().expect("puts");
    let mut new_pos = insn_index(cfg, block, last_put).unwrap_or(0);
    if let Some(first) = first_not_a_put_usage {
        if let Some(idx) = insn_index(cfg, block, first) {
            new_pos = new_pos.min(idx);
        }
    }
    cfg.arena[block].insns.insert(new_pos, filled_id);
    Ok(true)
}

fn verify_put_insns(
    cfg: &MethodCfg,
    block: BlockId,
    arr_reg: &RegisterArg,
    arr_puts: &BTreeMap<i64, InsnId>,
) -> bool {
    let insn_list = &cfg.block(block).insns;
    // expect all puts to be in the same block
    if insn_list.len() < arr_puts.len() {
        return false;
    }
    if !arr_puts.values().all(|p| insn_list.contains(p)) {
        return false;
    }
    // array arg shouldn't be used in puts insns
    for &put in arr_puts.values() {
        if arg_contains_var(cfg, &cfg.insn(put).args[2], arr_reg) {
            return false;
        }
    }
    true
}

fn replace_const_in_arg(cfg: &mut MethodCfg, env: &MethodEnv, value: &InsnArg) -> InsnArg {
    if let InsnArg::Literal(l) = value {
        if let Some(f) = get_const_field_literal(env, l) {
            let sget = InsnNode::new(
                InsnType::Sget,
                InsnKind::Index(IndexValue::Field(f.info.clone())),
            );
            let id = cfg.add_insn(sget);
            if let Ok(arg) = cfg.wrap_arg(id) {
                add_field_usage(cfg, &f);
                return arg;
            }
        }
    }
    cfg.duplicate_arg(value)
}
