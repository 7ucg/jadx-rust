//! Block graph queries (`BlockUtils`).

use crate::model::bitset::BitSet;
use crate::model::cfg::MethodCfg;
use crate::model::insn::BlockId;

fn traverse_successors_until(
    cfg: &MethodCfg,
    from: BlockId,
    until: BlockId,
    visited: &mut BitSet,
    clean: bool,
) -> bool {
    let nodes: Vec<BlockId> = if clean {
        cfg.block(from).clean_succs().cloned().unwrap_or_default()
    } else {
        cfg.block(from).succs.clone()
    };
    for s in nodes {
        if s == until {
            return true;
        }
        if s == from {
            continue;
        }
        let id = cfg.block(s).pos;
        if !visited.get(id) {
            visited.set(id);
            if is_dominator(cfg, until, s) {
                return true;
            }
            if traverse_successors_until(cfg, s, until, visited, clean) {
                return true;
            }
        }
    }
    false
}

/// `until.isDominator(s)`: `s` dominates `until`.
pub fn is_dominator(cfg: &MethodCfg, block: BlockId, dominator: BlockId) -> bool {
    cfg.block(block)
        .doms
        .as_ref()
        .is_some_and(|d| d.get(cfg.block(dominator).pos))
}

pub fn is_path_exists(cfg: &MethodCfg, start: BlockId, end: BlockId) -> bool {
    if start == end
        || cfg
            .block(start)
            .clean_succs()
            .is_some_and(|c| c.contains(&end))
    {
        return true;
    }
    traverse_successors_until(cfg, start, end, &mut BitSet::new(), true)
}

pub fn is_any_path_exists(cfg: &MethodCfg, start: BlockId, end: BlockId) -> bool {
    if start == end || is_dominator(cfg, end, start) || cfg.block(start).succs.contains(&end) {
        return true;
    }
    traverse_successors_until(cfg, start, end, &mut BitSet::new(), false)
}

pub fn at_least_one_path_exists(cfg: &MethodCfg, starts: &[BlockId], end: BlockId) -> bool {
    starts.iter().any(|s| is_path_exists(cfg, *s, end))
}

pub fn get_top_block(cfg: &MethodCfg, blocks: &[BlockId]) -> Option<BlockId> {
    if blocks.len() == 1 {
        return Some(blocks[0]);
    }
    for &from in blocks {
        let top = blocks
            .iter()
            .all(|&to| from == to || is_any_path_exists(cfg, from, to));
        if top {
            return Some(from);
        }
    }
    None
}

pub fn get_bottom_block(cfg: &MethodCfg, blocks: &[BlockId]) -> Option<BlockId> {
    if blocks.len() == 1 {
        return Some(blocks[0]);
    }
    // attempt 1: a block dominated by every other block
    for &cand in blocks {
        if blocks
            .iter()
            .all(|&from| cand == from || is_dominator(cfg, cand, from))
        {
            return Some(cand);
        }
    }
    // attempt 2: a block with a path from every other block
    blocks.iter().copied().find(|&cand| {
        blocks
            .iter()
            .all(|&from| cand == from || is_any_path_exists(cfg, from, cand))
    })
}

fn new_bits() -> BitSet {
    BitSet::new()
}

fn bit_set_to_one_block(cfg: &MethodCfg, bs: &BitSet) -> Option<BlockId> {
    if bs.cardinality() != 1 {
        return None;
    }
    let pos = bs.next_set_bit(0)?;
    cfg.blocks.get(pos).copied()
}

pub fn get_common_dominator(cfg: &MethodCfg, blocks: &[BlockId]) -> Option<BlockId> {
    let mut doms = new_bits();
    doms.set_range(0, cfg.blocks.len());
    for &b in blocks {
        doms.and(cfg.block(b).doms.as_ref()?);
    }
    let mut combine = new_bits();
    combine.or(&doms);
    for pos in doms.iter() {
        let block = *cfg.blocks.get(pos)?;
        if let Some(idom) = cfg.block(block).idom {
            combine.and_not(cfg.block(idom).doms.as_ref()?);
            combine.clear(cfg.block(idom).pos);
        }
    }
    bit_set_to_one_block(cfg, &combine)
}

pub fn get_path_cross(cfg: &MethodCfg, blocks: &[BlockId]) -> Option<BlockId> {
    let mut dom_front = new_bits();
    let mut first = true;
    for &b in blocks {
        let mut tmp = new_bits();
        tmp.set(cfg.block(b).pos);
        if let Some(df) = &cfg.block(b).dom_frontier {
            tmp.or(df);
        }
        if first {
            dom_front.or(&tmp);
            first = false;
        } else {
            dom_front.and(&tmp);
        }
    }
    dom_front.clear(cfg.block(cfg.exit).pos);
    if dom_front.is_empty() {
        return None;
    }
    if let Some(b) = bit_set_to_one_block(cfg, &dom_front) {
        return Some(b);
    }
    let mut excluded = new_bits();
    excluded.set(cfg.block(cfg.exit).pos);
    for l in &cfg.loops {
        excluded.set(cfg.block(cfg.loop_infos[*l].start).pos);
    }
    for h in cfg.handler_list.iter().map(|i| &cfg.handlers[*i]) {
        if let Some(hb) = h.handler_block {
            excluded.set(cfg.block(hb).pos);
        }
    }
    dom_front.and_not(&excluded);
    if let Some(b) = bit_set_to_one_block(cfg, &dom_front) {
        return Some(b);
    }
    let mut combined = new_bits();
    let mut k = cfg.blocks.len() as isize;
    loop {
        for pos in dom_front.iter() {
            if let Some(&blk) = cfg.blocks.get(pos) {
                if let Some(df) = &cfg.block(blk).dom_frontier {
                    if !df.is_empty() {
                        combined.or(df);
                    }
                }
            }
        }
        combined.and_not(&excluded);
        let card = combined.cardinality();
        if card == 1 {
            return bit_set_to_one_block(cfg, &combined);
        }
        if card == 0 {
            return None;
        }
        if k < 0 {
            return None;
        }
        k -= 1;
        dom_front.clear_all();
        dom_front.or(&combined);
        combined.clear_all();
    }
}

fn collect_while_dominates(
    cfg: &MethodCfg,
    dominator: BlockId,
    child: BlockId,
    result: &mut Vec<BlockId>,
    visited: &mut BitSet,
) {
    let cpos = cfg.block(child).pos;
    if visited.get(cpos) {
        return;
    }
    visited.set(cpos);
    for &node in &cfg.block(child).succs {
        if is_dominator(cfg, node, dominator) {
            if !result.contains(&node) {
                result.push(node);
            }
            collect_while_dominates(cfg, dominator, node, result, visited);
        }
    }
}

/// Blocks dominated by `dominator`, starting from `start`, including exception handler edges
/// (insertion ordered, like the reference's `LinkedHashSet`).
pub fn collect_blocks_dominated_by_with_exc_handlers(
    cfg: &MethodCfg,
    dominator: BlockId,
    start: BlockId,
) -> Vec<BlockId> {
    let mut result = Vec::new();
    collect_while_dominates(cfg, dominator, start, &mut result, &mut BitSet::new());
    result
}

pub fn get_block_with_flag(cfg: &MethodCfg, blocks: &[BlockId], flag: u64) -> Option<BlockId> {
    blocks
        .iter()
        .copied()
        .find(|b| cfg.block(*b).contains(flag))
}

/// `getPathCross(mth, b1, b2)`
pub fn get_path_cross_pair(cfg: &MethodCfg, b1: BlockId, b2: BlockId) -> Option<BlockId> {
    if b1 == b2 {
        return Some(b1);
    }
    get_path_cross(cfg, &[b1, b2])
}

/// `DFSIteration`: depth first walk preserving successor order.
pub struct DfsIteration {
    queue: Vec<BlockId>,
    visited: BitSet,
    clean: bool,
}

impl DfsIteration {
    pub fn new(cfg: &MethodCfg, start: BlockId, clean: bool) -> Self {
        let mut visited = BitSet::new();
        visited.set(cfg.block(start).pos);
        DfsIteration {
            queue: vec![start],
            visited,
            clean,
        }
    }

    pub fn next(&mut self, cfg: &MethodCfg) -> Option<BlockId> {
        let current = self.queue.pop()?;
        let next_blocks: Vec<BlockId> = if self.clean {
            cfg.block(current)
                .clean_succs()
                .cloned()
                .unwrap_or_default()
        } else {
            cfg.block(current).succs.clone()
        };
        for &n in next_blocks.iter().rev() {
            let pos = cfg.block(n).pos;
            if !self.visited.get(pos) {
                self.visited.set(pos);
                self.queue.push(n);
            }
        }
        Some(current)
    }
}

/// `collectAllSuccessors`
pub fn collect_all_successors(cfg: &MethodCfg, start: BlockId, clean: bool) -> Vec<BlockId> {
    let mut dfs = DfsIteration::new(cfg, start, clean);
    let mut list = Vec::new();
    while let Some(b) = dfs.next(cfg) {
        list.push(b);
    }
    list
}

/// `getPrevBlockOnPath`: predecessor of `block` on the path from `path_start`.
pub fn get_prev_block_on_path(
    cfg: &MethodCfg,
    block: BlockId,
    path_start: BlockId,
) -> Option<BlockId> {
    let preds = &cfg.block(block).preds;
    if preds.contains(&path_start) {
        return Some(path_start);
    }
    let mut dfs = DfsIteration::new(cfg, path_start, true);
    while let Some(next) = dfs.next(cfg) {
        if preds.contains(&next) {
            return Some(next);
        }
    }
    None
}
