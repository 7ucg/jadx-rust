//! Block graph queries (`BlockUtils`).

use crate::model::bitset::BitSet;
use crate::model::cfg::MethodCfg;
use crate::model::insn::{BlockId, InsnId};

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

/// The set of block positions reachable from `start` through its *clean* successors via at least
/// one edge - exactly the set `traverse_successors_until(.., start, .., clean = true)` walks.
/// `start` itself is excluded unless a cycle leads back to it through other blocks (matching the
/// traversal, which skips its own `from`). Computed once so many `is_path_exists(start, x)`
/// queries that share one `start` become O(1) bitset lookups instead of a DFS each (see
/// [`path_exists_from_reachable`]).
pub fn clean_succ_reachable(cfg: &MethodCfg, start: BlockId) -> BitSet {
    let start_pos = cfg.block(start).pos;
    let mut r = BitSet::new();
    let mut stack: Vec<BlockId> = cfg.block(start).clean_succs().cloned().unwrap_or_default();
    while let Some(b) = stack.pop() {
        let pos = cfg.block(b).pos;
        if pos == start_pos || r.get(pos) {
            continue;
        }
        r.set(pos);
        if let Some(succs) = cfg.block(b).clean_succs() {
            for &s in succs {
                let sp = cfg.block(s).pos;
                if sp != start_pos && !r.get(sp) {
                    stack.push(s);
                }
            }
        }
    }
    r
}

/// [`is_path_exists`] answered from a precomputed [`clean_succ_reachable`] set for `start`.
/// Equivalent to `is_path_exists(cfg, start, end)` by construction: `is_path_exists` is true iff
/// `start == end`, or `end` is clean-successor-reachable from `start`, or some clean-successor-
/// reachable block dominates `end` (the dominator short-circuit inside `traverse_successors_until`,
/// `s.isDominator(end)` ⇔ `end.doms` contains `s`). `reach` must be `clean_succ_reachable(cfg, start)`.
pub fn path_exists_from_reachable(
    cfg: &MethodCfg,
    start: BlockId,
    end: BlockId,
    reach: &BitSet,
) -> bool {
    if start == end {
        return true;
    }
    if reach.get(cfg.block(end).pos) {
        return true;
    }
    if let Some(doms) = cfg.block(end).doms.as_ref() {
        let mut inter = doms.clone();
        inter.and(reach);
        if !inter.is_empty() {
            return true;
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

/// `getFirstInsn`
pub fn get_first_insn(cfg: &MethodCfg, block: BlockId) -> Option<InsnId> {
    cfg.block(block).insns.first().copied()
}

/// `getLastInsn`
pub fn get_last_insn(cfg: &MethodCfg, block: BlockId) -> Option<InsnId> {
    cfg.block(block).insns.last().copied()
}

/// `isBackEdge`
pub fn is_back_edge(cfg: &MethodCfg, from: BlockId, to: BlockId) -> bool {
    if cfg
        .block(from)
        .clean_succs()
        .is_some_and(|c| c.contains(&to))
    {
        return false;
    }
    cfg.block(from).succs.contains(&to)
}

/// `isAllBlocksEmpty`
pub fn is_all_blocks_empty(cfg: &MethodCfg, blocks: &[BlockId]) -> bool {
    blocks.iter().all(|b| cfg.block(*b).insns.is_empty())
}

fn next_block_on_empty_path(
    cfg: &MethodCfg,
    block: BlockId,
    reverse: bool,
    clean_only: bool,
) -> Option<BlockId> {
    let b = cfg.block(block);
    let clean_count = b.clean_succs().map_or(0, |c| c.len());
    if !b.insns.is_empty() || (!reverse && b.preds.len() > 1) || (reverse && clean_count > 1) {
        return None;
    }
    let next: &[BlockId] = if reverse {
        &b.preds
    } else if clean_only {
        b.clean_succs().map_or(&[][..], |c| c.as_slice())
    } else {
        &b.succs
    };
    if next.len() != 1 {
        return None;
    }
    Some(next[0])
}

/// `followEmptyPath(start, reverse, cleanOnly)`
pub fn follow_empty_path(
    cfg: &MethodCfg,
    mut start: BlockId,
    reverse: bool,
    clean_only: bool,
) -> BlockId {
    while let Some(next) = next_block_on_empty_path(cfg, start, reverse, clean_only) {
        start = next;
    }
    start
}

/// `followEmptyUpPathWithinSet`
pub fn follow_empty_up_path_within_set(
    cfg: &MethodCfg,
    start: BlockId,
    traversable: &[BlockId],
) -> Vec<BlockId> {
    let mut results = Vec::new();
    let mut traversed: Vec<BlockId> = Vec::new();
    follow_empty_up_path_rec(cfg, &mut results, start, traversable, &mut traversed);
    results
}

fn follow_empty_up_path_rec(
    cfg: &MethodCfg,
    results: &mut Vec<BlockId>,
    mut start: BlockId,
    traversable: &[BlockId],
    traversed: &mut Vec<BlockId>,
) {
    let preds: Vec<BlockId> = cfg
        .block(start)
        .preds
        .iter()
        .copied()
        .filter(|p| traversable.contains(p))
        .collect();
    for pred in preds {
        if !traversable.contains(&pred) || traversed.contains(&pred) {
            continue;
        }
        traversed.push(pred);
        if cfg.block(pred).insns.is_empty() {
            follow_empty_up_path_rec(cfg, results, start, traversable, traversed);
        } else {
            results.push(pred);
        }
        start = pred;
    }
}

fn collect_until(
    cfg: &MethodCfg,
    blocks: &mut Vec<BlockId>,
    current: BlockId,
    clean: bool,
    stop: &dyn Fn(BlockId) -> bool,
) {
    if blocks.contains(&current) {
        return;
    }
    blocks.push(current);
    if stop(current) {
        return;
    }
    let succs: Vec<BlockId> = if clean {
        cfg.block(current)
            .clean_succs()
            .cloned()
            .unwrap_or_default()
    } else {
        cfg.block(current).succs.clone()
    };
    for s in succs {
        collect_until(cfg, blocks, s, clean, stop);
    }
}

/// `collectAllSuccessorsUntil`: depth first, the block matching `stop` is included but not
/// expanded.
pub fn collect_all_successors_until(
    cfg: &MethodCfg,
    start: BlockId,
    clean: bool,
    stop: &dyn Fn(BlockId) -> bool,
) -> Vec<BlockId> {
    let mut blocks = Vec::new();
    collect_until(cfg, &mut blocks, start, clean, stop);
    blocks
}

/// `collectAllPredecessors`
pub fn collect_all_predecessors(cfg: &MethodCfg, start: BlockId) -> Vec<BlockId> {
    let mut visited = vec![false; cfg.arena.len()];
    visited[start] = true;
    let mut queue = vec![start];
    let mut list = Vec::new();
    while let Some(current) = queue.pop() {
        for &n in cfg.block(current).preds.iter().rev() {
            if !visited[n] {
                visited[n] = true;
                queue.push(n);
            }
        }
        list.push(current);
    }
    list
}

/// `getBottomCommonPredecessor`: the bottom block of the blocks that precede every block of
/// `blocks` (the intermediate `HashSet` iteration order is emulated).
pub fn get_bottom_common_predecessor(cfg: &MethodCfg, blocks: &[BlockId]) -> Option<BlockId> {
    let first = *blocks.first()?;
    let initial = collect_all_predecessors(cfg, first);
    let mut by_all: Vec<BlockId> = initial.clone();
    for &next in &blocks[1..] {
        let preds = collect_all_predecessors(cfg, next);
        by_all.retain(|b| preds.contains(b));
    }
    let ordered = java_hash_set_order(&by_all, initial.len());
    get_bottom_block(cfg, &ordered)
}

/// Iteration order of a `java.util.HashSet` of blocks (hash code = block id) created by
/// `new HashSet<>(collection)` from a collection of `initial_size` elements; `items` are the
/// elements in their insertion order.
pub fn java_hash_set_order(items: &[BlockId], initial_size: usize) -> Vec<BlockId> {
    let wanted = ((initial_size as f32 / 0.75) as usize + 1).max(16);
    let cap = wanted.next_power_of_two();
    let mut v: Vec<BlockId> = items.to_vec();
    v.sort_by_key(|x| (x ^ (x >> 16)) & (cap - 1));
    v
}
