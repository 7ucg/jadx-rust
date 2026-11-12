//! Block traversal and dominator computation (`BlockUtils` DFS, `DominatorTree`, `PostDominatorTree`).

use std::collections::VecDeque;

use crate::model::bitset::BitSet;
use crate::model::cfg::MethodCfg;
use crate::model::insn::BlockId;

/// `BlockUtils.visitDFS` / `visitReverseDFS`: pre-order DFS preserving successor order.
pub fn dfs_order(cfg: &MethodCfg, start: BlockId, reverse: bool) -> Vec<BlockId> {
    let mut visited = vec![false; cfg.arena.len()];
    let mut queue: VecDeque<BlockId> = VecDeque::new();
    queue.push_back(start);
    visited[start] = true;
    let mut out = Vec::new();
    while let Some(cur) = queue.pop_back() {
        let next = if reverse {
            &cfg.arena[cur].preds
        } else {
            &cfg.arena[cur].succs
        };
        for &n in next.iter().rev() {
            if !visited[n] {
                visited[n] = true;
                queue.push_back(n);
            }
        }
        out.push(cur);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DomError(pub String);

/// Cooper-Harvey-Kennedy iteration on blocks in `sorted` order; positions must equal indexes.
fn build(
    cfg: &MethodCfg,
    sorted: &[BlockId],
    use_preds: bool,
) -> Result<Vec<Option<BlockId>>, DomError> {
    let n = sorted.len();
    let mut doms: Vec<Option<BlockId>> = vec![None; n];
    doms[0] = Some(sorted[0]);
    let pos = |b: BlockId| cfg.arena[b].pos;
    let mut changed = true;
    while changed {
        changed = false;
        for block_id in 1..n {
            let b = sorted[block_id];
            let preds = if use_preds {
                &cfg.arena[b].preds
            } else {
                &cfg.arena[b].succs
            };
            let mut picked = None;
            let mut new_idom = None;
            for &p in preds {
                let id = pos(p);
                if id < n && doms[id].is_some() {
                    new_idom = Some(p);
                    picked = Some(id);
                    break;
                }
            }
            let Some(mut new_idom) = new_idom else {
                return Err(DomError(format!("No immediate dominator for block: {b}")));
            };
            for &pb in preds {
                let pid = pos(pb);
                if Some(pid) == picked {
                    continue;
                }
                if pid < n && doms[pid].is_some() {
                    new_idom = intersect(cfg, sorted, &doms, pb, new_idom);
                }
            }
            if doms[block_id] != Some(new_idom) {
                doms[block_id] = Some(new_idom);
                changed = true;
            }
        }
    }
    Ok(doms)
}

fn intersect(
    cfg: &MethodCfg,
    sorted: &[BlockId],
    doms: &[Option<BlockId>],
    b1: BlockId,
    b2: BlockId,
) -> BlockId {
    let mut f1 = cfg.arena[b1].pos;
    let mut f2 = cfg.arena[b2].pos;
    while f1 != f2 {
        while f1 > f2 {
            f1 = cfg.arena[doms[f1].expect("dominator")].pos;
        }
        while f2 > f1 {
            f2 = cfg.arena[doms[f2].expect("dominator")].pos;
        }
    }
    sorted[f1]
}

fn collect_doms(cfg: &MethodCfg, doms: &[Option<BlockId>], idom: BlockId, post: bool) -> BitSet {
    let mut bs = BitSet::new();
    let mut next = idom;
    loop {
        let id = cfg.arena[next].pos;
        if bs.get(id) {
            break;
        }
        bs.set(id);
        // the reference reads the forward dominators here even when building post dominators
        let _ = post;
        let cur = &cfg.arena[next].doms;
        if let Some(c) = cur {
            bs.or(c);
            break;
        }
        next = doms[id].expect("dominator");
    }
    bs
}

/// `DominatorTree.compute`: reorders `cfg.blocks` in DFS order and fills doms / idom.
pub fn compute_dominators(cfg: &mut MethodCfg) -> Result<(), DomError> {
    let sorted = dfs_order(cfg, cfg.enter, false);
    if sorted.len() != cfg.blocks.len() {
        return Err(DomError("Found unreachable blocks".into()));
    }
    cfg.blocks = sorted.clone();
    cfg.update_block_positions();
    let doms = build(cfg, &sorted, true)?;
    let enter = sorted[0];
    cfg.arena[enter].doms = Some(BitSet::new());
    cfg.arena[enter].idom = None;
    for i in 1..sorted.len() {
        let block = sorted[i];
        let idom = doms[i].expect("idom");
        cfg.arena[block].idom = Some(idom);
        cfg.arena[idom].dominates_on.push(block);
        let mut bs = collect_doms(cfg, &doms, idom, false);
        bs.clear(i);
        cfg.arena[block].doms = Some(bs);
    }
    Ok(())
}

pub fn compute_dominance_frontier(cfg: &mut MethodCfg) {
    let blocks = cfg.blocks.clone();
    for &b in &blocks {
        cfg.arena[b].dom_frontier = None;
    }
    for &b in &blocks {
        let preds = cfg.arena[b].preds.clone();
        if preds.len() >= 2 {
            let idom = cfg.arena[b].idom;
            for p in preds {
                let mut runner = Some(p);
                while runner != idom {
                    let Some(r) = runner else { break };
                    let pos = cfg.arena[b].pos;
                    cfg.arena[r]
                        .dom_frontier
                        .get_or_insert_with(BitSet::new)
                        .set(pos);
                    runner = cfg.arena[r].idom;
                }
            }
        }
    }
    for &b in &blocks {
        let empty = cfg.arena[b]
            .dom_frontier
            .as_ref()
            .is_none_or(|d| d.is_empty());
        if empty {
            cfg.arena[b].dom_frontier = Some(BitSet::new());
        }
    }
}

/// `PostDominatorTree.compute` (only when the method requests it).
pub fn compute_post_dominators(cfg: &mut MethodCfg) {
    if !cfg.compute_post_dom {
        return;
    }
    let mth_blocks_count = cfg.blocks.len();
    let sorted = dfs_order(cfg, cfg.exit, true);
    let blocks_count = sorted.len();
    let mut pos_mapping = vec![0usize; mth_blocks_count.max(blocks_count)];
    for (i, b) in sorted.iter().enumerate() {
        pos_mapping[i] = cfg.arena[*b].pos;
    }
    for (i, b) in sorted.iter().enumerate() {
        cfg.arena[*b].pos = i;
    }
    let result = build(cfg, &sorted, false);
    if let Ok(post_doms) = result {
        let first = sorted[0];
        cfg.arena[first].post_doms = Some(BitSet::new());
        cfg.arena[first].ipost_dom = None;
        for i in 1..blocks_count {
            let block = sorted[i];
            let ipd = post_doms[i].expect("post dom");
            cfg.arena[block].ipost_dom = Some(ipd);
            let bs = collect_doms(cfg, &post_doms, ipd, true);
            cfg.arena[block].post_doms = Some(bs);
        }
        for i in 1..blocks_count {
            let block = sorted[i];
            let mut bs = BitSet::new();
            if let Some(pd) = cfg.arena[block].post_doms.clone() {
                for n in pd.iter() {
                    bs.set(pos_mapping[n]);
                }
            }
            bs.clear(pos_mapping[i]);
            cfg.arena[block].post_doms = Some(bs);
        }
    }
    cfg.update_block_positions();
}
