//! `RegionUtils` and block helpers used by the region stages.

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{iflag, BlockId, InsnId, InsnType};
use crate::model::region::{Container, RegionId, RegionKind};

/// `getCleanSuccessors`
pub fn cs(cfg: &MethodCfg, b: BlockId) -> Vec<BlockId> {
    cfg.block(b)
        .clean_succs()
        .cloned()
        .unwrap_or_else(|| cfg.block(b).succs.clone())
}

/// `BlockUtils.getNextBlock`: first clean successor.
pub fn next_block(cfg: &MethodCfg, b: BlockId) -> Option<BlockId> {
    let blk = cfg.block(b);
    match blk.clean_succs() {
        Some(c) => c.first().copied(),
        None => blk.succs.first().copied(),
    }
}

pub fn last_insn(cfg: &MethodCfg, b: BlockId) -> Option<InsnId> {
    cfg.block(b).insns.last().copied()
}

/// `BlockUtils.checkLastInsnType`
pub fn last_insn_is(cfg: &MethodCfg, b: BlockId, ty: InsnType) -> bool {
    last_insn(cfg, b).is_some_and(|i| cfg.insn(i).ty == ty)
}

/// `BlockUtils.isExitBlock(block)`
pub fn is_exit_block_simple(cfg: &MethodCfg, b: BlockId) -> bool {
    let succs = &cfg.block(b).succs;
    if succs.is_empty() {
        return true;
    }
    if succs.len() == 1 {
        return cfg.block(succs[0]).succs.is_empty();
    }
    false
}

/// `BlockUtils.isExitBlock(mth, block)`
pub fn is_exit_block(cfg: &MethodCfg, b: BlockId) -> bool {
    b == cfg.exit || is_exit_block_simple(cfg, b)
}

/// `MethodNode.isPreExitBlock`
pub fn is_pre_exit_block(cfg: &MethodCfg, b: BlockId) -> bool {
    let succs = &cfg.block(b).succs;
    if succs.len() == 1 {
        return succs[0] == cfg.exit;
    }
    cfg.block(cfg.exit).preds.contains(&b)
}

/// `BlockUtils.containsExitInsn`
pub fn contains_exit_insn(cfg: &MethodCfg, c: Container) -> bool {
    let Some(last) = container_last_insn(cfg, c) else {
        return false;
    };
    matches!(
        cfg.insn(last).ty,
        InsnType::Return | InsnType::Throw | InsnType::Break | InsnType::Continue
    )
}

/// Flag check on any container.
pub fn cflag(cfg: &MethodCfg, c: Container, flag: u64) -> bool {
    match c {
        Container::Block(b) => cfg.block(b).contains(flag),
        Container::Insns(i) => cfg.regions.insn_conts[i].flags & flag != 0,
        Container::Region(r) => cfg.regions.arena[r].flags & flag != 0,
    }
}

pub fn cflag_add(cfg: &mut MethodCfg, c: Container, flag: u64) {
    match c {
        Container::Block(b) => cfg.block_mut(b).add(flag),
        Container::Insns(i) => cfg.regions.insn_conts[i].flags |= flag,
        Container::Region(r) => cfg.regions.arena[r].flags |= flag,
    }
}

/// `IBlock.getInstructions` of a block-like container.
pub fn container_insns(cfg: &MethodCfg, c: Container) -> Vec<InsnId> {
    match c {
        Container::Block(b) => cfg.block(b).insns.clone(),
        Container::Insns(i) => cfg.regions.insn_conts[i].insns.clone(),
        Container::Region(_) => Vec::new(),
    }
}

pub fn container_last_insn(cfg: &MethodCfg, c: Container) -> Option<InsnId> {
    match c {
        Container::Block(b) => cfg.block(b).insns.last().copied(),
        Container::Insns(i) => cfg.regions.insn_conts[i].insns.last().copied(),
        Container::Region(_) => None,
    }
}

pub fn is_block_like(c: Container) -> bool {
    !matches!(c, Container::Region(_))
}

fn subs(cfg: &MethodCfg, r: RegionId) -> Vec<Container> {
    cfg.regions.sub_blocks(r)
}

/// `RegionUtils.hasExitEdge`
pub fn has_exit_edge(cfg: &MethodCfg, c: Container) -> bool {
    match c {
        Container::Region(r) if cfg.regions.is_branch_region(r) => {
            // all branches must have exit edge
            for br in cfg.regions.branches(r) {
                match br {
                    Some(b) if has_exit_edge(cfg, b) => {}
                    _ => return false,
                }
            }
            true
        }
        Container::Region(r) => match subs(cfg, r).last() {
            Some(l) => has_exit_edge(cfg, *l),
            None => false,
        },
        _ => contains_exit_insn(cfg, c),
    }
}

/// `RegionUtils.getFirstInsn`
pub fn get_first_insn(cfg: &MethodCfg, c: Container) -> Option<InsnId> {
    match c {
        Container::Region(r) => {
            if cfg.regions.is_branch_region(r) {
                return None;
            }
            subs(cfg, r).first().and_then(|f| get_first_insn(cfg, *f))
        }
        _ => container_insns(cfg, c).first().copied(),
    }
}

/// `RegionUtils.getFirstBlockNode`
pub fn get_first_block_node(cfg: &MethodCfg, c: Container) -> Option<BlockId> {
    match c {
        Container::Block(b) => Some(b),
        Container::Insns(_) => None,
        Container::Region(r) => match &cfg.regions.arena[r].kind {
            RegionKind::If { cond_blocks, .. } | RegionKind::Loop { cond_blocks, .. } => {
                cond_blocks.first().copied()
            }
            RegionKind::TryCatch { try_r, .. } => get_first_block_node(cfg, *try_r),
            RegionKind::Switch { header, .. } => Some(*header),
            _ => subs(cfg, r)
                .into_iter()
                .find_map(|s| get_first_block_node(cfg, s)),
        },
    }
}

/// `RegionUtils.getLastInsn`
pub fn get_last_insn(cfg: &MethodCfg, c: Container) -> Option<InsnId> {
    match c {
        Container::Region(r) => {
            if cfg.regions.is_branch_region(r) {
                return None;
            }
            subs(cfg, r).last().and_then(|f| get_last_insn(cfg, *f))
        }
        _ => container_last_insn(cfg, c),
    }
}

/// `RegionUtils.getLastInsnWithBlock`: the instruction and its (block-like) container.
pub fn get_last_insn_with_block(cfg: &MethodCfg, c: Container) -> Option<(Container, InsnId)> {
    match c {
        Container::Region(r) => {
            if cfg.regions.is_branch_region(r) {
                let branches: Vec<Container> =
                    cfg.regions.branches(r).into_iter().flatten().collect();
                if branches.len() == 1 {
                    return get_last_insn_with_block(cfg, branches[0]);
                }
                return None;
            }
            let all = subs(cfg, r);
            let last = all.last()?;
            get_last_insn_with_block(cfg, *last)
        }
        _ => container_last_insn(cfg, c).map(|i| (c, i)),
    }
}

/// `RegionUtils.getLastBlock`
pub fn get_last_block(cfg: &MethodCfg, c: Container) -> Option<Container> {
    match c {
        Container::Region(r) => {
            if cfg.regions.is_branch_region(r) {
                return None;
            }
            let all = subs(cfg, r);
            let last = all.last()?;
            get_last_block(cfg, *last)
        }
        _ => Some(c),
    }
}

/// `RegionUtils.isExitBlock`
pub fn is_exit_container(cfg: &MethodCfg, c: Container) -> bool {
    match c {
        Container::Block(b) => is_exit_block(cfg, b),
        _ => false,
    }
}

/// `RegionUtils.hasExitBlock`
pub fn has_exit_block(cfg: &MethodCfg, c: Option<Container>) -> bool {
    match c {
        None => false,
        Some(c) => has_exit_block_root(cfg, c, c),
    }
}

fn has_exit_block_root(cfg: &MethodCfg, root: Container, c: Container) -> bool {
    match c {
        Container::Block(b) => {
            if is_exit_block_simple(cfg, b) {
                return true;
            }
            is_insn_exit_container(cfg, root, c)
        }
        Container::Insns(_) => is_insn_exit_container(cfg, root, c),
        Container::Region(r) => {
            if cfg.regions.is_branch_region(r) {
                return cfg
                    .regions
                    .branches(r)
                    .into_iter()
                    .all(|b| has_exit_block(cfg, b));
            }
            match subs(cfg, r).last() {
                Some(l) => has_exit_block_root(cfg, root, *l),
                None => false,
            }
        }
    }
}

fn is_insn_exit_container(cfg: &MethodCfg, root: Container, c: Container) -> bool {
    let Some(last) = container_last_insn(cfg, c) else {
        return false;
    };
    let insn = cfg.insn(last);
    match insn.ty {
        InsnType::Return => true,
        InsnType::Throw => {
            // check if after throw execution can continue in current container
            if let Some(handlers) = &insn.catch {
                for h in handlers {
                    if let Some(hb) = cfg.handlers[*h].handler_block {
                        if is_region_contains_block(cfg, root, hb) {
                            return false;
                        }
                    }
                }
            }
            true
        }
        InsnType::Break => {
            if let Some(loops) = cfg.regions.insn_loops.get(&last) {
                for l in loops {
                    let start = cfg.loop_infos[*l].start;
                    if !is_region_contains_block(cfg, root, start) {
                        return true;
                    }
                }
            }
            if let Some(l) = cfg.regions.insn_loop_label.get(&last) {
                let start = cfg.loop_infos[*l].start;
                if !is_region_contains_block(cfg, root, start) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// `RegionUtils.hasBreakInsn`
pub fn has_break_insn(cfg: &MethodCfg, c: Container) -> bool {
    match c {
        Container::Region(r) => match subs(cfg, r).last() {
            Some(l) => has_break_insn(cfg, *l),
            None => false,
        },
        _ => container_last_insn(cfg, c).is_some_and(|i| cfg.insn(i).ty == InsnType::Break),
    }
}

/// `RegionUtils.getFirstSourceLine`: `0` when no instruction of the container has a line.
pub fn get_first_source_line(cfg: &MethodCfg, c: Container) -> u32 {
    match c {
        Container::Block(_) | Container::Insns(_) => container_insns(cfg, c)
            .into_iter()
            .map(|i| cfg.insn(i).source_line)
            .find(|&l| l != 0)
            .unwrap_or(0),
        Container::Region(r) => match &cfg.regions.arena[r].kind {
            // `ConditionRegion.getConditionSourceLine`: `IfRegion`/`LoopRegion` are condition
            // regions before they are (for `IfRegion`) branch regions.
            RegionKind::If { cond_blocks, .. } | RegionKind::Loop { cond_blocks, .. } => {
                cond_blocks
                    .iter()
                    .filter_map(|&b| last_insn(cfg, b))
                    .map(|i| cfg.insn(i).source_line)
                    .find(|&l| l != 0)
                    .unwrap_or(0)
            }
            _ => {
                if cfg.regions.is_branch_region(r) {
                    cfg.regions
                        .branches(r)
                        .into_iter()
                        .flatten()
                        .map(|b| get_first_source_line(cfg, b))
                        .find(|&l| l != 0)
                        .unwrap_or(0)
                } else {
                    subs(cfg, r)
                        .into_iter()
                        .map(|s| get_first_source_line(cfg, s))
                        .find(|&l| l != 0)
                        .unwrap_or(0)
                }
            }
        },
    }
}

/// `RegionUtils.insnsCount`
pub fn insns_count(cfg: &MethodCfg, c: Container) -> usize {
    match c {
        Container::Region(r) => subs(cfg, r).into_iter().map(|s| insns_count(cfg, s)).sum(),
        _ => container_insns(cfg, c)
            .into_iter()
            .filter(|i| !cfg.insn(*i).contains_flag(iflag::DONT_GENERATE))
            .count(),
    }
}

/// `RegionUtils.isEmpty`
pub fn is_empty(cfg: &MethodCfg, c: Option<Container>) -> bool {
    !not_empty(cfg, c)
}

/// `RegionUtils.notEmpty`
pub fn not_empty(cfg: &MethodCfg, c: Option<Container>) -> bool {
    let Some(c) = c else {
        return false;
    };
    match c {
        Container::Region(r) => {
            if matches!(cfg.regions.arena[r].kind, RegionKind::Loop { .. }) {
                return true;
            }
            subs(cfg, r).into_iter().any(|s| not_empty(cfg, Some(s)))
        }
        _ => container_insns(cfg, c)
            .into_iter()
            .any(|i| !cfg.insn(i).contains_flag(iflag::DONT_GENERATE)),
    }
}

/// `RegionUtils.getAllRegionBlocks`
pub fn get_all_region_blocks(cfg: &MethodCfg, c: Container, out: &mut Vec<Container>) {
    match c {
        Container::Region(r) => {
            for s in subs(cfg, r) {
                get_all_region_blocks(cfg, s, out);
            }
        }
        _ => {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
}

/// `RegionUtils.isRegionContainsBlock`
pub fn is_region_contains_block(cfg: &MethodCfg, c: Container, block: BlockId) -> bool {
    match c {
        Container::Block(b) => b == block,
        Container::Insns(_) => false,
        Container::Region(r) => subs(cfg, r)
            .into_iter()
            .any(|s| is_region_contains_block(cfg, s, block)),
    }
}

/// `RegionUtils.getSingleSubBlock`
pub fn get_single_sub_block(cfg: &MethodCfg, c: Container) -> Option<Container> {
    if let Container::Region(r) = c {
        if let RegionKind::Plain { blocks } = &cfg.regions.arena[r].kind {
            if blocks.len() == 1 {
                return Some(ignore_simple_region_wrapper(cfg, blocks[0]));
            }
        }
    }
    None
}

fn ignore_simple_region_wrapper(cfg: &MethodCfg, mut c: Container) -> Container {
    loop {
        match c {
            Container::Region(r) => match &cfg.regions.arena[r].kind {
                RegionKind::Plain { blocks } => {
                    if blocks.len() != 1 {
                        return c;
                    }
                    c = blocks[0];
                }
                _ => return c,
            },
            _ => return c,
        }
    }
}

/// `RegionUtils.isRegionContainsRegion`
pub fn is_region_contains_region(cfg: &MethodCfg, container: Container, region: RegionId) -> bool {
    if container == Container::Region(region) {
        return true;
    }
    let mut cur = region;
    let mut parent = cfg.regions.arena[cur].parent;
    loop {
        match parent {
            Some(p) if container == Container::Region(p) => return true,
            Some(p) => {
                cur = p;
                parent = cfg.regions.arena[cur].parent;
            }
            None => {
                if cfg.regions.arena[cur].exc_handler.is_some() {
                    return is_region_contains_exc_handler_region(cfg, container, cur);
                }
                return false;
            }
        }
    }
}

fn is_region_contains_exc_handler_region(
    cfg: &MethodCfg,
    container: Container,
    region: RegionId,
) -> bool {
    if container == Container::Region(region) {
        return true;
    }
    if let Container::Region(r) = container {
        for b in subs(cfg, r) {
            // TRY_BLOCK is never attached to regions, so only the plain recursion remains
            if is_region_contains_region(cfg, b, region) {
                return true;
            }
        }
    }
    false
}

/// `RegionUtils.getBlockContainer`
pub fn get_block_container(cfg: &MethodCfg, c: Container, block: BlockId) -> Option<Container> {
    get_container_of(cfg, c, Container::Block(block))
}

/// `RegionUtils.getBlockContainer` for any block-like container.
pub fn get_container_of(cfg: &MethodCfg, c: Container, block: Container) -> Option<Container> {
    match c {
        Container::Region(r) => {
            for s in subs(cfg, r) {
                if let Some(res) = get_container_of(cfg, s, block) {
                    return Some(if is_block_like(res) {
                        Container::Region(r)
                    } else {
                        res
                    });
                }
            }
            None
        }
        _ => {
            if c == block {
                Some(c)
            } else {
                None
            }
        }
    }
}

/// `RegionUtils.isBlocksInSameRegion`: two blocks on the same level of one (non branch) region.
pub fn is_blocks_in_same_region(cfg: &MethodCfg, first: BlockId, second: BlockId) -> bool {
    let Some(region) = cfg.regions.method_region else {
        return false;
    };
    if let Some(Container::Region(r)) = get_block_container(cfg, Container::Region(region), first) {
        if cfg.regions.is_branch_region(r) {
            return false;
        }
        return cfg
            .regions
            .sub_blocks(r)
            .contains(&Container::Block(second));
    }
    false
}

/// `RegionUtils.isDominatedBy`
pub fn is_dominated_by(cfg: &MethodCfg, dom: BlockId, c: Container) -> bool {
    if c == Container::Block(dom) {
        return true;
    }
    match c {
        Container::Block(b) => crate::passes::block_utils::is_dominator(cfg, b, dom),
        Container::Insns(_) => false,
        Container::Region(r) => subs(cfg, r)
            .into_iter()
            .all(|s| is_dominated_by(cfg, dom, s)),
    }
}

/// `RegionUtils.hasPathThroughBlock`
pub fn has_path_through_block(cfg: &MethodCfg, block: BlockId, c: Container) -> bool {
    if c == Container::Block(block) {
        return true;
    }
    match c {
        Container::Block(b) => crate::passes::block_utils::is_path_exists(cfg, block, b),
        Container::Insns(_) => false,
        Container::Region(r) => subs(cfg, r)
            .into_iter()
            .any(|s| has_path_through_block(cfg, block, s)),
    }
}

/// `RegionUtils.isPathExists(block, container)`
pub fn is_path_exists_to_container(cfg: &MethodCfg, block: BlockId, c: Container) -> bool {
    match get_first_block_node(cfg, c) {
        Some(first) => crate::passes::block_utils::is_path_exists(cfg, block, first),
        None => false,
    }
}

/// `RegionUtils.visitBlocks`: all block-like containers below `c` (depth first).
pub fn collect_blocks(cfg: &MethodCfg, c: Container, out: &mut Vec<Container>) {
    match c {
        Container::Region(r) => {
            for s in subs(cfg, r) {
                collect_blocks(cfg, s, out);
            }
        }
        _ => out.push(c),
    }
}

/// `RegionUtils.visitBlockNodes`
pub fn collect_block_nodes(cfg: &MethodCfg, c: Container) -> Vec<BlockId> {
    let mut v = Vec::new();
    collect_blocks(cfg, c, &mut v);
    v.into_iter()
        .filter_map(|c| match c {
            Container::Block(b) => Some(b),
            _ => None,
        })
        .collect()
}

/// `RegionUtils.addToAll`
pub fn add_to_all(cfg: &mut MethodCfg, c: Container, flag: u64) {
    let mut v = Vec::new();
    collect_blocks(cfg, c, &mut v);
    for b in v {
        cflag_add(cfg, b, flag);
    }
}

/// `RegionUtils.getNextContainer`
pub fn get_next_container(cfg: &MethodCfg, region: RegionId) -> Option<Container> {
    let parent = cfg.regions.arena[region].parent?;
    let all = subs(cfg, parent);
    let idx = all.iter().position(|s| *s == Container::Region(region))?;
    all.get(idx + 1).copied()
}

/// `RegionUtils.getLoopsStartInRegion`
pub fn get_loops_start_in_region(cfg: &MethodCfg, c: Container) -> Vec<usize> {
    let mut loops = Vec::new();
    for b in collect_block_nodes(cfg, c) {
        if cfg.block(b).contains(bflag::LOOP_START) {
            loops.extend(cfg.block(b).loops.iter().copied());
        }
    }
    loops
}
