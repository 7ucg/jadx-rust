//! `BlockExceptionHandler`: connect exception handlers to the blocks they protect and build
//! the try/catch block structure.

use std::collections::HashMap;

use crate::clsp::ClspGraph;
use crate::model::bitset::{java_hash_map_order_compute, java_sort, java_string_hash};
use crate::model::cfg::{bflag, MethodCfg, TryBlock};
use crate::model::info::ClassInfo;
use crate::model::insn::{iflag, BlockId, HandlerId, InsnType, RegisterArg};
use crate::passes::block_processor::{detach_block, remove_marked_blocks};
use crate::passes::block_utils::*;
use crate::passes::dominators::{compute_dominance_frontier, dfs_order};
use crate::type_compare::{TypeCompare, TypeCompareEnum};

type Res<T> = Result<T, String>;

fn can_throw(t: InsnType) -> bool {
    !matches!(
        t,
        InsnType::Return
            | InsnType::If
            | InsnType::Goto
            | InsnType::Move
            | InsnType::MoveException
            | InsnType::Neg
            | InsnType::Const
            | InsnType::ConstStr
            | InsnType::ConstClass
            | InsnType::CmpL
            | InsnType::CmpG
            | InsnType::Nop
    )
}

fn update_clean_successors_all(cfg: &mut MethodCfg) {
    for b in cfg.blocks.clone() {
        cfg.update_clean_successors(b);
    }
}

pub fn process(cfg: &mut MethodCfg, clsp: &ClspGraph) -> Res<bool> {
    if cfg.handler_list.is_empty() {
        return Ok(false);
    }
    update_clean_successors_all(cfg);
    compute_dominance_frontier(cfg);
    process_catch_attr(cfg);
    init_exc_handlers(cfg)?;
    let try_blocks = prepare_try_blocks(cfg, clsp)?;
    connect_exc_handlers(cfg, &try_blocks)?;
    cfg.try_list = try_blocks.clone();
    update_clean_successors_all(cfg);
    for eh in cfg.handler_list.clone() {
        remove_monitor_exit_from_exc_handler(cfg, eh);
    }
    remove_marked_blocks(cfg);
    let sorted = dfs_order(cfg, cfg.enter, false);
    remove_unused_exc_handlers(cfg, &try_blocks, &sorted);
    Ok(true)
}

fn process_catch_attr(cfg: &mut MethodCfg) {
    for b in cfg.blocks.clone() {
        for insn in &mut cfg.block_mut(b).insns {
            if insn.catch.is_some() && !can_throw(insn.ty) {
                insn.catch = None;
            }
        }
    }
    for b in cfg.blocks.clone() {
        let mut common: Option<Vec<HandlerId>> = None;
        let mut conflict = false;
        for insn in &cfg.block(b).insns {
            if let Some(c) = &insn.catch {
                match &common {
                    None => common = Some(c.clone()),
                    Some(cc) => {
                        if cc != c {
                            conflict = true;
                            break;
                        }
                    }
                }
            }
        }
        if conflict {
            continue;
        }
        if let Some(c) = common {
            let (mut enter, mut leave) = (false, false);
            for insn in &cfg.block(b).insns {
                enter |= insn.contains_flag(iflag::TRY_ENTER);
                leave |= insn.contains_flag(iflag::TRY_LEAVE);
            }
            let blk = cfg.block_mut(b);
            blk.catch = Some(c);
            if enter {
                blk.add(bflag::TRY_ENTER);
            }
            if leave {
                blk.add(bflag::TRY_LEAVE);
            }
        }
    }
}

fn block_split_top(cfg: &mut MethodCfg, block: BlockId) -> BlockId {
    let off = cfg.block(block).start_offset;
    let nb = cfg.start_new_block(off);
    for pred in cfg.block(block).preds.clone() {
        cfg.remove_connection(pred, block);
        cfg.connect(pred, nb);
        crate::passes::block_splitter::replace_target(cfg, pred, block, nb);
        cfg.update_clean_successors(pred);
    }
    cfg.connect(nb, block);
    cfg.update_clean_successors(nb);
    nb
}

fn replace_connection(cfg: &mut MethodCfg, source: BlockId, old: BlockId, new: BlockId) {
    cfg.remove_connection(source, old);
    cfg.connect(source, new);
    crate::passes::block_splitter::replace_target(cfg, source, old, new);
}

fn init_exc_handlers(cfg: &mut MethodCfg) -> Res<()> {
    let blocks_count = cfg.blocks.len();
    for i in 0..blocks_count {
        let block = cfg.blocks[i];
        let Some(first) = cfg.block(block).insns.first() else {
            continue;
        };
        let Some(handler) = first.exc_handler else {
            continue;
        };
        cfg.block_mut(block).insns[0].exc_handler = None;
        // remove tmp connection
        if let Some(src) = cfg.block(block).tmp_edge {
            cfg.remove_connection(src, block);
            cfg.block_mut(block).tmp_edge = None;
        }
        if cfg.block(block).preds.is_empty() {
            cfg.handlers[handler].handler_block = Some(block);
            cfg.block_mut(block).exc_handler = Some(handler);
            cfg.handlers[handler].blocks.push(block);
            for b in collect_blocks_dominated_by_with_exc_handlers(cfg, block, block) {
                cfg.handlers[handler].blocks.push(b);
            }
        } else {
            let off = cfg.block(block).start_offset;
            let empty = cfg.start_new_block(off);
            cfg.block_mut(empty).add(bflag::SYNTHETIC);
            cfg.block_mut(empty).exc_handler = Some(handler);
            cfg.connect(empty, block);
            cfg.handlers[handler].handler_block = Some(empty);
            cfg.handlers[handler].blocks.push(empty);
        }
        fix_move_exception_insn(cfg, block, handler);
    }
    Ok(())
}

fn fix_move_exception_insn(cfg: &mut MethodCfg, block: BlockId, handler: HandlerId) {
    let arg_type = cfg.handlers[handler].arg_type();
    if let Some(me) = cfg.block_mut(block).insns.last_mut() {
        if me.ty == InsnType::MoveException {
            if let Some(res) = &me.result {
                let reg = res.reg_num;
                me.result = Some(RegisterArg::new(reg, arg_type));
                me.flags |= iflag::DONT_INLINE;
                me.exc_handler = Some(handler);
                cfg.handlers[handler].arg_reg = Some(reg);
            }
        }
    }
}

fn mark_for_remove(cfg: &mut MethodCfg, h: HandlerId) {
    cfg.handlers[h].removed = true;
    for b in cfg.handlers[h].blocks.clone() {
        let blk = cfg.block_mut(b);
        blk.add(bflag::REMOVE);
        blk.add(bflag::DONT_GENERATE);
    }
}

fn clear_exception_handlers(cfg: &mut MethodCfg) {
    let removed: Vec<HandlerId> = cfg
        .handler_list
        .iter()
        .copied()
        .filter(|h| cfg.handlers[*h].removed)
        .collect();
    cfg.handler_list.retain(|h| !removed.contains(h));
}

fn detach_marked_blocks(cfg: &mut MethodCfg) {
    for b in cfg.blocks.clone() {
        if cfg.block(b).contains(bflag::REMOVE) {
            detach_block(cfg, b);
        }
    }
}

fn remove_exc_handler(cfg: &mut MethodCfg, h: HandlerId) {
    mark_for_remove(cfg, h);
    if let Some(hb) = cfg.handlers[h].handler_block {
        cfg.remove_connection(cfg.enter, hb);
    }
}

fn new_try(
    cfg: &mut MethodCfg,
    id: usize,
    handlers: Vec<HandlerId>,
    blocks: Vec<BlockId>,
) -> usize {
    let tid = cfg.try_blocks.len();
    for h in &handlers {
        cfg.handlers[*h].try_block = Some(tid);
    }
    cfg.try_blocks.push(TryBlock {
        id,
        handlers,
        blocks,
        outer: None,
        inner: Vec::new(),
        merged: false,
        top_splitter: None,
    });
    tid
}

fn handler_hash(cfg: &MethodCfg, h: HandlerId) -> i32 {
    // Objects.hash(catchTypes, handlerOffset)
    let eh = &cfg.handlers[h];
    let mut list_hash: i32 = 1;
    for c in &eh.catch_types {
        let ch = java_string_hash(&c.raw_name());
        list_hash = list_hash.wrapping_mul(31).wrapping_add(ch);
    }
    let mut r: i32 = 1;
    r = r.wrapping_mul(31).wrapping_add(list_hash);
    r = r.wrapping_mul(31).wrapping_add(eh.handler_offset);
    r
}

fn prepare_try_blocks(cfg: &mut MethodCfg, clsp: &ClspGraph) -> Res<Vec<usize>> {
    // handler -> protected blocks (HashMap iteration order emulated)
    let mut keys: Vec<HandlerId> = Vec::new();
    let mut by_handler: HashMap<HandlerId, Vec<BlockId>> = HashMap::new();
    for b in cfg.blocks.clone() {
        if let Some(c) = cfg.block(b).catch.clone() {
            for eh in c {
                if !by_handler.contains_key(&eh) {
                    keys.push(eh);
                }
                by_handler.entry(eh).or_default().push(b);
            }
        }
    }
    let hashes: Vec<i32> = keys.iter().map(|k| handler_hash(cfg, *k)).collect();
    let order: Vec<HandlerId> = java_hash_map_order_compute(&hashes)
        .into_iter()
        .map(|i| keys[i])
        .collect();
    if by_handler.is_empty() {
        for h in cfg.handler_list.clone() {
            remove_exc_handler(cfg, h);
        }
    } else {
        for &eh in &order {
            if by_handler[&eh].is_empty() {
                remove_exc_handler(cfg, eh);
            }
        }
    }
    detach_marked_blocks(cfg);
    clear_exception_handlers(cfg);
    if cfg.handler_list.is_empty() {
        return Ok(Vec::new());
    }
    for eh in &order {
        let hb = cfg.handlers[*eh].blocks.clone();
        by_handler
            .get_mut(eh)
            .expect("handler")
            .retain(|b| !hb.contains(b));
    }
    let mut try_list: Vec<usize> = Vec::new();
    for &eh in &order {
        let blocks = by_handler[&eh].clone();
        let tid = new_try(cfg, try_list.len(), vec![eh], blocks);
        try_list.push(tid);
    }
    if try_list.len() > 1 {
        while combine_try_catch_blocks(cfg, &mut try_list) {}
    }
    check_for_multi_catch(cfg, &try_list);
    // clearTryBlocks
    for &t in &try_list {
        let blocks = cfg.try_blocks[t].blocks.clone();
        cfg.try_blocks[t].blocks = blocks
            .into_iter()
            .filter(|b| !cfg.block(*b).contains(bflag::REMOVE))
            .collect();
    }
    try_list.retain(|t| {
        !(cfg.try_blocks[*t].blocks.is_empty() || cfg.try_blocks[*t].handlers.is_empty())
    });
    clear_exception_handlers(cfg);
    detach_marked_blocks(cfg);
    sort_handlers(cfg, &try_list, clsp);
    Ok(try_list)
}

fn combine_try_catch_blocks(cfg: &mut MethodCfg, try_list: &mut Vec<usize>) -> bool {
    for oi in 0..try_list.len() {
        for ii in 0..try_list.len() {
            let (outer, inner) = (try_list[oi], try_list[ii]);
            if outer == inner || cfg.try_blocks[inner].outer.is_some() {
                continue;
            }
            if check_try_catch_relation(cfg, try_list, outer, inner) {
                return true;
            }
        }
    }
    false
}

fn concat_distinct<T: PartialEq + Clone>(a: &[T], b: &[T]) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for x in a.iter().chain(b) {
        if !out.contains(x) {
            out.push(x.clone());
        }
    }
    out
}

fn check_try_catch_relation(
    cfg: &mut MethodCfg,
    try_list: &mut Vec<usize>,
    outer: usize,
    inner: usize,
) -> bool {
    let (ob, ib) = (
        cfg.try_blocks[outer].blocks.clone(),
        cfg.try_blocks[inner].blocks.clone(),
    );
    if ob == ib {
        let handlers = concat_distinct(
            &cfg.try_blocks[outer].handlers,
            &cfg.try_blocks[inner].handlers,
        );
        let tid = new_try(cfg, try_list.len(), handlers, ob);
        try_list.push(tid);
        try_list.retain(|t| *t != outer && *t != inner);
        return true;
    }
    let handler_blocks: Vec<BlockId> = cfg.try_blocks[inner]
        .handlers
        .iter()
        .flat_map(|h| cfg.handlers[*h].blocks.clone())
        .collect();
    let outer_handlers = cfg.try_blocks[outer].handlers.clone();
    let intersects =
        |cfg: &MethodCfg, b: BlockId| cfg.block(b).catch.as_ref() == Some(&outer_handlers);
    let catch_in_handler = handler_blocks.iter().any(|b| intersects(cfg, *b));
    let catch_in_try = ib.iter().any(|b| intersects(cfg, *b));
    let blocks_outside_handler = ob.iter().any(|b| !handler_blocks.contains(b));
    if catch_in_handler && (catch_in_try || blocks_outside_handler) {
        let merged = concat_distinct(&ob, &ib);
        let outer_h = cfg.try_blocks[outer].handlers.clone();
        cfg.try_blocks[inner]
            .handlers
            .retain(|h| !outer_h.contains(h));
        cfg.try_blocks[inner].outer = Some(outer);
        cfg.try_blocks[outer].inner.push(inner);
        cfg.try_blocks[outer].blocks = merged;
        return false;
    }
    let inner_h = cfg.try_blocks[inner].handlers.clone();
    if cfg.try_blocks[outer]
        .handlers
        .iter()
        .all(|h| inner_h.contains(h))
    {
        let merged = concat_distinct(&ob, &ib);
        let handlers = concat_distinct(&cfg.try_blocks[outer].handlers, &inner_h);
        let tid = new_try(cfg, try_list.len(), handlers, merged);
        try_list.push(tid);
        try_list.retain(|t| *t != outer && *t != inner);
        return true;
    }
    false
}

fn connect_exc_handlers(cfg: &mut MethodCfg, try_blocks: &[usize]) -> Res<()> {
    if try_blocks.is_empty() {
        return Ok(());
    }
    let limit = try_blocks.len() * 3;
    let mut count = 0usize;
    let mut queue: std::collections::VecDeque<usize> = try_blocks.iter().copied().collect();
    while let Some(tb) = queue.pop_front() {
        let complete = wrap_blocks_with_try_catch(cfg, tb)?;
        if !complete {
            queue.push_back(tb);
        }
        count += 1;
        if count - 1 > limit {
            return Err("Try blocks wrapping queue limit reached!".into());
        }
    }
    Ok(())
}

fn wrap_blocks_with_try_catch(cfg: &mut MethodCfg, tb: usize) -> Res<bool> {
    let blocks = cfg.try_blocks[tb].blocks.clone();
    let top = search_top_block(cfg, &blocks)?;
    if cfg.block(top).preds.is_empty() && top != cfg.enter {
        return Ok(false);
    }
    let mut bottom = search_bottom_block(cfg, &blocks);
    let mut split_return: Option<BlockId> = None;
    if let Some(b) = bottom {
        if cfg.block(b).contains(bflag::RETURN) {
            split_return = Some(b);
            let nb = block_split_top(cfg, b);
            cfg.block_mut(nb).add(bflag::SYNTHETIC);
            bottom = Some(nb);
        }
    }
    let top_splitter = get_top_splitter_block(cfg, top);
    cfg.block_mut(top_splitter).add(bflag::EXC_TOP_SPLITTER);
    cfg.block_mut(top_splitter).add(bflag::SYNTHETIC);
    let total_handler_blocks: usize = cfg.try_blocks[tb]
        .handlers
        .iter()
        .map(|h| cfg.handlers[*h].blocks.len())
        .sum();
    let bottom_splitter: Option<BlockId> = match bottom {
        Some(bt) if total_handler_blocks != 0 => {
            let succs = cfg.block(bt).succs.clone();
            let exist = get_block_with_flag(cfg, &succs, bflag::EXC_BOTTOM_SPLITTER);
            let bs = match exist {
                Some(e) => e,
                None => cfg.start_new_block(-1),
            };
            cfg.block_mut(bs).add(bflag::EXC_BOTTOM_SPLITTER);
            cfg.block_mut(bs).add(bflag::SYNTHETIC);
            cfg.connect(bt, bs);
            if let Some(sr) = split_return {
                let bottom_preds = cfg.block(bt).preds.clone();
                for h in cfg.try_blocks[tb].handlers.clone() {
                    let hb = cfg.handlers[h].blocks.clone();
                    let inter: Vec<BlockId> = bottom_preds
                        .iter()
                        .copied()
                        .filter(|b| hb.contains(b))
                        .collect();
                    if !inter.is_empty() && inter.len() == 1 {
                        replace_connection(cfg, inter[0], bt, sr);
                    }
                }
            }
            Some(bs)
        }
        _ => None,
    };
    connect_splitters_and_handlers(cfg, tb, top_splitter, bottom_splitter);
    if let Some(bt) = bottom {
        if let Some(orig) = cfg.block(bt).split_cross {
            let convert: Vec<BlockId> = cfg
                .block(bt)
                .preds
                .clone()
                .into_iter()
                .filter(|b| is_any_path_exists(cfg, bt, *b))
                .collect();
            for b in convert {
                replace_connection(cfg, b, bt, orig);
            }
        }
    }
    for b in &blocks {
        let cur = cfg.block(*b).try_block;
        let set = match cur {
            None => true,
            Some(c) => cfg.try_blocks[c].inner.contains(&tb),
        };
        if set {
            cfg.block_mut(*b).try_block = Some(tb);
        }
    }
    cfg.try_blocks[tb].top_splitter = Some(top_splitter);
    cfg.update_clean_successors(top_splitter);
    if let Some(bs) = bottom_splitter {
        cfg.update_clean_successors(bs);
    }
    Ok(true)
}

fn get_top_splitter_block(cfg: &mut MethodCfg, top: BlockId) -> BlockId {
    if top == cfg.enter {
        let fixed_top = cfg.block(cfg.enter).succs[0];
        return block_split_top(cfg, fixed_top);
    }
    let preds = cfg.block(top).preds.clone();
    if let Some(e) = get_block_with_flag(cfg, &preds, bflag::EXC_TOP_SPLITTER) {
        return e;
    }
    let clean = cfg.block(top).clean_succs().cloned().unwrap_or_default();
    if clean.len() == 1 && cfg.block(top).insns.is_empty() {
        if let Some(other) = get_block_with_flag(cfg, &clean, bflag::EXC_TOP_SPLITTER) {
            if cfg.block(other).preds.len() == 1 {
                return other;
            }
        }
    }
    block_split_top(cfg, top)
}

fn adjust_top_block(cfg: &MethodCfg, top: BlockId) -> BlockId {
    if cfg.block(top).succs.len() == 1 && cfg.block(top).catch.is_none() {
        return cfg.block(top).succs[0];
    }
    top
}

fn search_top_block(cfg: &MethodCfg, blocks: &[BlockId]) -> Res<BlockId> {
    if let Some(top) = get_top_block(cfg, blocks) {
        return Ok(adjust_top_block(cfg, top));
    }
    if let Some(dom) = get_common_dominator(cfg, blocks) {
        if cfg.block(dom).succs.len() == 1 {
            let up = cfg.block(dom).succs[0];
            if blocks.contains(&up) {
                return Ok(up);
            }
        }
        return Ok(adjust_top_block(cfg, dom));
    }
    Err(format!(
        "Failed to find top block for try-catch from: {blocks:?}"
    ))
}

fn search_bottom_block(cfg: &mut MethodCfg, blocks: &[BlockId]) -> Option<BlockId> {
    if let Some(b) = get_bottom_block(cfg, blocks) {
        return Some(b);
    }
    let path_cross = get_path_cross(cfg, blocks)?;
    let mut preds = cfg.block(path_cross).preds.clone();
    preds.retain(|p| !blocks.contains(p));
    let outside: Vec<BlockId> = preds
        .into_iter()
        .filter(|p| !at_least_one_path_exists(cfg, blocks, *p))
        .collect();
    if outside.is_empty() || outside.len() == cfg.block(path_cross).preds.len() {
        return Some(path_cross);
    }
    let split_cross = block_split_top(cfg, path_cross);
    cfg.block_mut(split_cross).add(bflag::SYNTHETIC);
    cfg.block_mut(split_cross).split_cross = Some(path_cross);
    for o in outside {
        replace_connection(cfg, o, split_cross, path_cross);
    }
    Some(split_cross)
}

fn connect_splitters_and_handlers(
    cfg: &mut MethodCfg,
    tb: usize,
    top: BlockId,
    bottom: Option<BlockId>,
) {
    for h in cfg.try_blocks[tb].handlers.clone() {
        if let Some(hb) = cfg.handlers[h].handler_block {
            cfg.connect(top, hb);
            if let Some(b) = bottom {
                cfg.connect(b, hb);
            }
        }
    }
    if let Some(outer) = cfg.try_blocks[tb].outer {
        connect_splitters_and_handlers(cfg, outer, top, bottom);
    }
}

fn remove_monitor_exit_from_exc_handler(cfg: &mut MethodCfg, eh: HandlerId) {
    for exc_block in cfg.handlers[eh].blocks.clone() {
        let insns = &mut cfg.block_mut(exc_block).insns;
        let mut remove = Vec::new();
        for (i, insn) in insns.iter().enumerate() {
            if insn.ty == InsnType::MonitorEnter {
                break;
            }
            if insn.ty == InsnType::MonitorExit {
                remove.push(i);
            }
        }
        for i in remove.into_iter().rev() {
            insns.remove(i);
        }
    }
}

fn check_for_multi_catch(cfg: &mut MethodCfg, try_list: &[usize]) {
    let mut merged = false;
    for &t in try_list {
        if merge_multi_catch(cfg, t) {
            merged = true;
        }
    }
    if merged {
        detach_marked_blocks(cfg);
        clear_exception_handlers(cfg);
    }
}

fn merge_multi_catch(cfg: &mut MethodCfg, t: usize) -> bool {
    let handlers = cfg.try_blocks[t].handlers.clone();
    if handlers.len() < 2 {
        return false;
    }
    for &h in &handlers {
        if cfg.handlers[h].blocks.len() != 1 {
            return false;
        }
        let Some(block) = cfg.handlers[h].handler_block else {
            return false;
        };
        let b = cfg.block(block);
        if b.insns.len() != 1 || b.insns.last().map(|i| i.ty) != Some(InsnType::MoveException) {
            return false;
        }
    }
    let handler_blocks: Vec<BlockId> = handlers
        .iter()
        .filter_map(|h| cfg.handlers[*h].handler_block)
        .collect();
    let mut succ_blocks: Vec<BlockId> = Vec::new();
    for hb in &handler_blocks {
        for s in &cfg.block(*hb).succs {
            if !succ_blocks.contains(s) {
                succ_blocks.push(*s);
            }
        }
    }
    if succ_blocks.len() != 1 {
        return false;
    }
    let succ = succ_blocks[0];
    let preds = &cfg.block(succ).preds;
    if preds.len() != handler_blocks.len() || !preds.iter().all(|p| handler_blocks.contains(p)) {
        return false;
    }
    let mut regs: Vec<u32> = Vec::new();
    for hb in &handler_blocks {
        let Some(res) = cfg.block(*hb).insns.last().and_then(|i| i.result.as_ref()) else {
            return false;
        };
        if !regs.contains(&res.reg_num) {
            regs.push(res.reg_num);
        }
    }
    if regs.len() != 1 {
        return false;
    }
    let result_handler = handlers[0];
    let mut kept = Vec::new();
    for &h in &handlers {
        if h == result_handler {
            kept.push(h);
            continue;
        }
        let types = cfg.handlers[h].catch_types.clone();
        for ty in types {
            cfg.handlers[result_handler].add_catch_type(Some(ty));
        }
        mark_for_remove(cfg, h);
    }
    cfg.try_blocks[t].handlers = kept;
    true
}

fn class_cmp(a: &ClassInfo, b: &ClassInfo) -> i32 {
    match a.raw_name().cmp(&b.raw_name()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// `compareByTypeAndName` with the reversed type comparator.
fn compare_by_type_and_name(clsp: &ClspGraph, first: &ClassInfo, second: &ClassInfo) -> i32 {
    let tc = TypeCompare::new(clsp);
    // reversed comparator: compare(a, b) = comparator.compare(b, a)
    let r = match tc.compare_types(second.arg_type(), first.arg_type()) {
        TypeCompareEnum::Conflict | TypeCompareEnum::ConflictByGeneric => -2,
        TypeCompareEnum::Wider | TypeCompareEnum::WiderByGeneric => -1,
        TypeCompareEnum::Narrow | TypeCompareEnum::NarrowByGeneric => 1,
        _ => 0,
    };
    if r == -2 {
        return class_cmp(first, second);
    }
    r
}

fn sort_handlers(cfg: &mut MethodCfg, try_list: &[usize], clsp: &ClspGraph) {
    for &t in try_list {
        for h in cfg.try_blocks[t].handlers.clone() {
            let mut types = cfg.handlers[h].catch_types.clone();
            java_sort(&mut types, |a, b| compare_by_type_and_name(clsp, a, b));
            cfg.handlers[h].catch_types = types;
        }
        let mut handlers = cfg.try_blocks[t].handlers.clone();
        let snapshot: Vec<(HandlerId, bool, Option<ClassInfo>)> = handlers
            .iter()
            .map(|h| {
                (
                    *h,
                    cfg.handlers[*h].is_catch_all(),
                    cfg.handlers[*h].catch_types.first().cloned(),
                )
            })
            .collect();
        let mut items = snapshot;
        java_sort(&mut items, |first, second| {
            if first.1 {
                return 1;
            }
            if second.1 {
                return -1;
            }
            match (&first.2, &second.2) {
                (Some(a), Some(b)) => compare_by_type_and_name(clsp, a, b),
                _ => 0,
            }
        });
        handlers = items.into_iter().map(|i| i.0).collect();
        cfg.try_blocks[t].handlers = handlers;
    }
}

fn remove_unused_exc_handlers(cfg: &mut MethodCfg, try_blocks: &[usize], sorted: &[BlockId]) {
    for eh in cfg.handler_list.clone() {
        let Some(hb) = cfg.handlers[eh].handler_block else {
            continue;
        };
        if sorted.contains(&hb) {
            continue;
        }
        let processed = try_blocks
            .iter()
            .any(|t| cfg.try_blocks[*t].handlers.contains(&eh));
        if !processed {
            crate::passes::block_processor::remove_unreachable_block(cfg, hb);
        }
    }
}
