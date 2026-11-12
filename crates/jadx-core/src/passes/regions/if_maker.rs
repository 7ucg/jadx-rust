//! `IfRegionMaker` and `IfInfo`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::model::bitset::BitSet;
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::condition::{CondMode, IfCondition};
use crate::model::insn::{BlockId, InsnId, InsnKind, InsnType};
use crate::model::region::{Container, RegionId, RegionKind};
use crate::passes::block_utils::{is_back_edge, is_dominator, is_path_exists};
use crate::passes::regions::block_ext::{
    bit_set_to_blocks, bit_set_to_one_block, dom_frontier, follow_empty, follow_empty_rev,
    get_bottom_block, get_path_cross2, get_try_and_handler_cross_block, is_duplicate_block_path,
    is_equal_paths, is_equal_return_blocks, is_follow_back_edge, skip_synthetic_predecessor,
};
use crate::passes::regions::maker::{RegionMaker, Res};
use crate::passes::regions::utils::{cs, last_insn};

/// State shared between the copies of an `IfInfo` (the reference shares the collections).
#[derive(Default)]
pub struct Shared {
    pub merged: BitSet,
    pub skip: Vec<BlockId>,
    pub force_inline: Vec<InsnId>,
}

#[derive(Clone)]
pub struct IfInfo {
    pub cond: IfCondition,
    pub then_block: Option<BlockId>,
    pub else_block: Option<BlockId>,
    pub shared: Rc<RefCell<Shared>>,
    pub out_block: Option<BlockId>,
}

impl IfInfo {
    fn new(cond: IfCondition, then_block: Option<BlockId>, else_block: Option<BlockId>) -> IfInfo {
        IfInfo {
            cond,
            then_block,
            else_block,
            shared: Rc::new(RefCell::new(Shared::default())),
            out_block: None,
        }
    }

    /// `new IfInfo(info, thenBlock, elseBlock)`: shares the collections.
    fn derive(info: &IfInfo, then_block: Option<BlockId>, else_block: Option<BlockId>) -> IfInfo {
        IfInfo {
            cond: info.cond.clone(),
            then_block,
            else_block,
            shared: info.shared.clone(),
            out_block: None,
        }
    }

    /// `IfInfo.invert`
    pub fn invert(cfg: &mut MethodCfg, info: &IfInfo) -> IfInfo {
        IfInfo {
            cond: IfCondition::invert(cfg, &info.cond),
            then_block: info.else_block,
            else_block: info.then_block,
            shared: info.shared.clone(),
            out_block: None,
        }
    }

    pub fn merged_blocks(&self, cfg: &MethodCfg) -> Vec<BlockId> {
        bit_set_to_blocks(cfg, &self.shared.borrow().merged)
    }

    pub fn merged_contains(&self, cfg: &MethodCfg, b: BlockId) -> bool {
        self.shared.borrow().merged.get(cfg.block(b).pos)
    }

    pub fn merged_size(&self) -> usize {
        self.shared.borrow().merged.cardinality()
    }

    pub fn first_if_block(&self, cfg: &MethodCfg) -> BlockId {
        let p = self
            .shared
            .borrow()
            .merged
            .next_set_bit(0)
            .expect("merged blocks");
        cfg.blocks[p]
    }

    fn merge(&self, others: &[&IfInfo]) {
        for o in others {
            let (m, s, f) = {
                let b = o.shared.borrow();
                (b.merged.clone(), b.skip.clone(), b.force_inline.clone())
            };
            let mut me = self.shared.borrow_mut();
            me.merged.or(&m);
            for x in s {
                if !me.skip.contains(&x) {
                    me.skip.push(x);
                }
            }
            me.force_inline.extend(f);
        }
    }
}

fn npe<T>(v: Option<T>, what: &str) -> Res<T> {
    v.ok_or_else(|| format!("NullPointerException: {what}"))
}

/// `IfRegionMaker.makeIfInfo`
pub fn make_if_info(cfg: &mut MethodCfg, if_block: BlockId) -> Option<IfInfo> {
    let last = last_insn(cfg, if_block)?;
    if cfg.insn(last).ty != InsnType::If {
        return None;
    }
    let (then_b, else_b) = match &cfg.insn(last).kind {
        InsnKind::If {
            then_block,
            else_block,
            ..
        } => (*then_block, *else_block),
        _ => return None,
    };
    let cond = IfCondition::from_if_node(cfg, last);
    let info = IfInfo::new(cond, then_b, else_b);
    let pos = cfg.block(if_block).pos;
    info.shared.borrow_mut().merged.set(pos);
    Some(info)
}

/// `IfRegionMaker.searchNestedIf`
pub fn search_nested_if(cfg: &mut MethodCfg, info: &IfInfo) -> Res<IfInfo> {
    Ok(merge_nested_if_nodes(cfg, info)?.unwrap_or_else(|| info.clone()))
}

/// `IfRegionMaker.confirmMerge`
pub fn confirm_merge(cfg: &mut MethodCfg, info: &IfInfo) {
    if info.merged_size() > 1 {
        let first = info.first_if_block(cfg);
        for b in info.merged_blocks(cfg) {
            if b != first {
                cfg.block_mut(b).add(bflag::ADDED_TO_REGION);
            }
        }
    }
    let skip = std::mem::take(&mut info.shared.borrow_mut().skip);
    for b in skip {
        cfg.block_mut(b).add(bflag::ADDED_TO_REGION);
    }
    let force = info.shared.borrow().force_inline.clone();
    for i in force {
        cfg.insns[i].flags |= crate::model::insn::iflag::FORCE_ASSIGN_INLINE;
    }
}

fn can_select_next(cfg: &MethodCfg, info: &IfInfo, block: BlockId) -> bool {
    let preds = &cfg.block(block).preds;
    if preds.len() == 1 {
        return true;
    }
    preds.iter().all(|p| info.merged_contains(cfg, *p))
}

fn get_next_if(cfg: &mut MethodCfg, info: &IfInfo, block: Option<BlockId>) -> Option<IfInfo> {
    let block = block?;
    if !can_select_next(cfg, info, block) {
        return None;
    }
    get_next_if_node_info(cfg, info, Some(block))
}

fn get_next_if_node_info(
    cfg: &mut MethodCfg,
    info: &IfInfo,
    block: Option<BlockId>,
) -> Option<IfInfo> {
    let block = block?;
    if !cfg.block(block).loops.is_empty() || cfg.block(block).contains(bflag::ADDED_TO_REGION) {
        return None;
    }
    if let Some(last) = last_insn(cfg, block) {
        if cfg.insn(last).ty == InsnType::If {
            return make_if_info(cfg, block);
        }
    }
    let next = get_next_block_in_if_successor_chain(cfg, block)?;
    if cfg.block(next).preds.len() != 1 || cfg.block(next).contains(bflag::ADDED_TO_REGION) {
        return None;
    }
    let mut force_inline = Vec::new();
    if !check_insns_inline(cfg, block, next, &mut force_inline) {
        return None;
    }
    match make_if_info(cfg, next) {
        None => get_next_if_node_info(cfg, info, Some(next)),
        Some(next_info) => {
            next_info
                .shared
                .borrow_mut()
                .force_inline
                .extend(force_inline);
            Some(next_info)
        }
    }
}

/// Allow singular successor to block or 2 successors where one is a `EXC_BOTTOM_SPLITTER`
fn get_next_block_in_if_successor_chain(cfg: &MethodCfg, block: BlockId) -> Option<BlockId> {
    let successors = &cfg.block(block).succs;
    if successors.len() > 2 || successors.is_empty() {
        return None;
    }
    let first = successors[0];
    if successors.len() == 1 {
        return Some(first);
    }
    let second = successors[1];
    let first_is_handler = cfg.block(first).contains(bflag::EXC_BOTTOM_SPLITTER);
    let second_is_handler = cfg.block(second).contains(bflag::EXC_BOTTOM_SPLITTER);
    if !first_is_handler && !second_is_handler {
        return None;
    }
    if first_is_handler && second_is_handler {
        return None;
    }
    let candidate = if first_is_handler { second } else { first };
    if cfg.block(candidate).insns.is_empty() {
        return get_next_block_in_if_successor_chain(cfg, candidate);
    }
    Some(candidate)
}

/// Check that all instructions can be inlined
fn check_insns_inline(
    cfg: &MethodCfg,
    block: BlockId,
    next: BlockId,
    force_inline: &mut Vec<InsnId>,
) -> bool {
    let insns = &cfg.block(block).insns;
    if insns.is_empty() {
        return true;
    }
    let mut pass = true;
    for &insn in insns {
        let Some(res) = &cfg.insn(insn).result else {
            return false;
        };
        let Some(v) = res.svar else {
            return false;
        };
        let uses = &cfg.svars[v].uses;
        let use_count = uses.len();
        if use_count == 0 {
            return false;
        }
        let use_place = uses[0].parent_insn();
        let in_block = use_place.is_some_and(|u| cfg.block(block).insns.contains(&u))
            || use_place.is_some_and(|u| cfg.block(next).insns.contains(&u));
        if !in_block {
            return false;
        }
        if use_count > 1 {
            force_inline.push(insn);
        } else {
            // allow only forced assign inline
            pass = false;
        }
    }
    pass
}

fn is_inversion_needed(cfg: &MethodCfg, current: &IfInfo, next: &IfInfo) -> bool {
    is_equal_paths(cfg, current.else_block, next.then_block)
        || is_equal_paths(cfg, current.then_block, next.else_block)
}

fn can_merge(cfg: &MethodCfg, a: &IfInfo, b: &IfInfo, follow_then: bool) -> bool {
    if follow_then {
        is_equal_paths(cfg, a.else_block, b.else_block)
    } else {
        is_equal_paths(cfg, a.then_block, b.then_block)
    }
}

fn check_condition_branches(cfg: &MethodCfg, from: Option<BlockId>, to: Option<BlockId>) -> bool {
    let (Some(from), Some(to)) = (from, to) else {
        return false;
    };
    let c = cs(cfg, from);
    c.len() == 1 && c.contains(&to)
}

/// `IfRegionMaker.mergeNestedIfNodes`
pub fn merge_nested_if_nodes(cfg: &mut MethodCfg, current: &IfInfo) -> Res<Option<IfInfo>> {
    let cur_then = current.then_block;
    let cur_else = current.else_block;
    if cur_then == cur_else {
        return Ok(None);
    }
    if is_follow_back_edge(cfg, cur_then) || is_follow_back_edge(cfg, cur_else) {
        return Ok(None);
    }
    let follow_then_branch;
    let mut next_if = match get_next_if(cfg, current, cur_then) {
        Some(n) => {
            follow_then_branch = true;
            n
        }
        None => match get_next_if(cfg, current, cur_else) {
            Some(n) => {
                follow_then_branch = false;
                n
            }
            None => return Ok(None),
        },
    };
    let assign_inline_needed = !next_if.shared.borrow().force_inline.is_empty();
    if assign_inline_needed {
        for mb in current.merged_blocks(cfg) {
            if cfg.block(mb).contains(bflag::LOOP_START) {
                // don't inline assigns into loop condition
                return Ok(Some(current.clone()));
            }
        }
    }
    if is_inversion_needed(cfg, current, &next_if) {
        // invert current node for match pattern
        next_if = IfInfo::invert(cfg, &next_if);
    }
    let then_path_same = is_equal_paths(cfg, cur_then, next_if.then_block);
    let else_path_same = is_equal_paths(cfg, cur_else, next_if.else_block);
    if !then_path_same && !else_path_same {
        // complex condition, run additional checks
        if check_condition_branches(cfg, cur_then, cur_else)
            || check_condition_branches(cfg, cur_else, cur_then)
        {
            return Ok(None);
        }
        let other = if follow_then_branch {
            cur_else
        } else {
            cur_then
        };
        let other = other.map(|b| follow_empty(cfg, b));
        let first_if = next_if.first_if_block(cfg);
        let exists = match other {
            Some(o) => is_path_exists(cfg, first_if, o),
            None => return Err("NullPointerException: otherBranchBlock".to_string()),
        };
        if !exists {
            return check_for_ternary_in_condition(cfg, current);
        }
        // this is nested conditions with different mode (i.e (a && b) || c),
        // search next condition for merge, get null if failed
        match merge_nested_if_nodes(cfg, &next_if)? {
            Some(tmp) => {
                next_if = tmp;
                if is_inversion_needed(cfg, current, &next_if) {
                    next_if = IfInfo::invert(cfg, &next_if);
                }
                if !can_merge(cfg, current, &next_if, follow_then_branch) {
                    return Ok(Some(current.clone()));
                }
            }
            None => return Ok(Some(current.clone())),
        }
    } else if assign_inline_needed {
        let same_outs =
            (then_path_same && !follow_then_branch) || (else_path_same && follow_then_branch);
        if !same_outs {
            // don't inline assigns inside simple condition
            current.shared.borrow_mut().force_inline.clear();
            return Ok(Some(current.clone()));
        }
    }
    let result = merge_if_info(cfg, current, &next_if, follow_then_branch)?;
    // search next nested if block
    Ok(Some(search_nested_if(cfg, &result)?))
}

fn check_for_ternary_in_condition(cfg: &mut MethodCfg, current: &IfInfo) -> Res<Option<IfInfo>> {
    let next_then = get_next_if(cfg, current, current.then_block);
    let next_else = get_next_if(cfg, current, current.else_block);
    let (Some(next_then), Some(next_else)) = (next_then, next_else) else {
        return Ok(None);
    };
    let df_then = dom_frontier(cfg, next_then.first_if_block(cfg));
    let df_else = dom_frontier(cfg, next_else.first_if_block(cfg));
    if df_then != df_else {
        return Ok(None);
    }
    let next_then = search_nested_if(cfg, &next_then)?;
    let mut next_else = search_nested_if(cfg, &next_else)?;
    if next_then.then_block == next_else.then_block && next_then.else_block == next_else.else_block
    {
        return Ok(Some(merge_ternary_conditions(
            cfg, current, &next_then, &next_else,
        )));
    }
    if next_then.then_block == next_else.else_block && next_then.else_block == next_else.then_block
    {
        next_else = IfInfo::invert(cfg, &next_else);
        return Ok(Some(merge_ternary_conditions(
            cfg, current, &next_then, &next_else,
        )));
    }
    Ok(None)
}

fn merge_ternary_conditions(
    cfg: &mut MethodCfg,
    current: &IfInfo,
    next_then: &IfInfo,
    next_else: &IfInfo,
) -> IfInfo {
    let new_cond = IfCondition::ternary(
        current.cond.clone(),
        next_then.cond.clone(),
        next_else.cond.clone(),
    );
    let result = IfInfo::new(new_cond, next_then.then_block, next_then.else_block);
    result.merge(&[current, next_then, next_else]);
    confirm_merge(cfg, &result);
    result
}

fn merge_if_info(
    cfg: &mut MethodCfg,
    first: &IfInfo,
    second: &IfInfo,
    follow_then: bool,
) -> Res<IfInfo> {
    let (then_block, else_block);
    if follow_then {
        then_block = second.then_block;
        else_block = get_branch_block(cfg, first.else_block, second.else_block, first)?;
    } else {
        then_block = get_branch_block(cfg, first.then_block, second.then_block, first)?;
        else_block = second.else_block;
    }
    let mode = if follow_then {
        CondMode::And
    } else {
        CondMode::Or
    };
    let cond = IfCondition::merge(mode, &first.cond, second.cond.clone());
    let result = IfInfo::new(cond, then_block, else_block);
    result.merge(&[first, second]);
    Ok(result)
}

fn add_skip(info: &IfInfo, b: BlockId) {
    let mut s = info.shared.borrow_mut();
    if !s.skip.contains(&b) {
        s.skip.push(b);
    }
}

fn get_branch_block(
    cfg: &mut MethodCfg,
    first: Option<BlockId>,
    second: Option<BlockId>,
    skip_owner: &IfInfo,
) -> Res<Option<BlockId>> {
    if first == second {
        return Ok(second);
    }
    let (f, s) = (npe(first, "first branch")?, npe(second, "second branch")?);
    if is_equal_return_blocks(cfg, f, s) {
        add_skip(skip_owner, f);
        return Ok(Some(s));
    }
    if is_duplicate_block_path(cfg, f, s) {
        cfg.block_mut(f).add(bflag::REMOVE);
        add_skip(skip_owner, f);
        return Ok(Some(s));
    }
    if let Some(cross) = get_path_cross2(cfg, Some(f), Some(s)) {
        for start in [f, s] {
            for b in crate::passes::regions::block_ext::blocks_on_path(cfg, start, cross) {
                add_skip(skip_owner, b);
            }
        }
        skip_owner.shared.borrow_mut().skip.retain(|b| *b != cross);
        return Ok(Some(cross));
    }
    let first_skip = follow_empty(cfg, f);
    let second_skip = follow_empty(cfg, s);
    if first_skip == second_skip || is_equal_return_blocks(cfg, first_skip, second_skip) {
        add_skip(skip_owner, f);
        add_skip(skip_owner, s);
        for b in crate::passes::regions::block_ext::blocks_on_empty_path(cfg, f) {
            add_skip(skip_owner, b);
        }
        for b in crate::passes::regions::block_ext::blocks_on_empty_path(cfg, s) {
            add_skip(skip_owner, b);
        }
        return Ok(Some(second_skip));
    }
    Err("Unexpected merge pattern".to_string())
}

/// `IfRegionMaker.findOutBlock`
pub fn find_out_block(
    cfg: &MethodCfg,
    then_block: Option<BlockId>,
    else_block: Option<BlockId>,
) -> Option<BlockId> {
    if then_block == else_block {
        return then_block;
    }
    let (then_block, else_block) = (then_block?, else_block?);
    let exit_pos = cfg.block(cfg.exit).pos;
    let mut then_df = dom_frontier(cfg, then_block);
    then_df.set(cfg.block(then_block).pos);
    let mut else_df = dom_frontier(cfg, else_block);
    else_df.set(cfg.block(else_block).pos);
    let mut inter = BitSet::new();
    inter.or(&then_df);
    inter.and(&else_df);
    inter.clear(exit_pos);
    // Attempt one: there's a unique block in the intersection of dom frontiers
    if let Some(one) = bit_set_to_one_block(cfg, &inter) {
        return Some(one);
    }
    let mut union = BitSet::new();
    union.or(&dom_frontier(cfg, then_block));
    union.or(&dom_frontier(cfg, else_block));
    union.clear(exit_pos);
    // Attempt two: look for a suitable block in the union.
    let mut candidates = BitSet::new();
    for cand in bit_set_to_blocks(cfg, &union) {
        if is_candidate_for_out_block(cfg, then_block, else_block, cand) {
            candidates.set(cfg.block(cand).pos);
        }
    }
    if let Some(bottom) = get_bottom_block(cfg, &bit_set_to_blocks(cfg, &candidates), true) {
        return Some(bottom);
    }
    // Attempt three: fallback to path cross again
    get_path_cross2(cfg, Some(then_block), Some(else_block))
}

fn is_candidate_for_out_block(
    cfg: &MethodCfg,
    then_block: BlockId,
    else_block: BlockId,
    candidate: BlockId,
) -> bool {
    if cfg.block(candidate).preds.len() < 2 {
        return false;
    }
    let mut cov_then = BitSet::new();
    let mut cov_else = BitSet::new();
    if candidate == else_block {
        cov_else.set(cfg.block(candidate).pos);
    }
    if candidate == then_block {
        cov_then.set(cfg.block(candidate).pos);
    }
    for &pred in &cfg.block(candidate).preds {
        if is_path_exists(cfg, then_block, pred) {
            cov_then.set(cfg.block(pred).pos);
        }
        if is_path_exists(cfg, else_block, pred) {
            cov_else.set(cfg.block(pred).pos);
        }
    }
    if cov_else.cardinality() == 0 || cov_then.cardinality() == 0 {
        return false;
    }
    let else_pred = bit_set_to_one_block(cfg, &cov_else);
    let then_pred = bit_set_to_one_block(cfg, &cov_then);
    if else_pred.is_some() && else_pred == then_pred {
        return false;
    }
    true
}

fn is_bad_branch_block(cfg: &MethodCfg, info: &IfInfo, block: BlockId) -> bool {
    // check if block at end of loop edge
    if cfg.block(block).contains(bflag::LOOP_START) && cfg.block(block).preds.len() == 1 {
        let pred = cfg.block(block).preds[0];
        if cfg.block(pred).contains(bflag::LOOP_END) {
            let start_loops = &cfg.block(block).loops;
            let end_loops = &cfg.block(pred).loops;
            for s in start_loops {
                for e in end_loops {
                    if s == e {
                        return true;
                    }
                }
            }
        }
    }
    // if branch block itself is outblock
    if let Some(out) = info.out_block {
        return block == out;
    }
    !all_paths_from_if(cfg, block, info)
}

fn all_paths_from_if(cfg: &MethodCfg, block: BlockId, info: &IfInfo) -> bool {
    for &pred in &cfg.block(block).preds {
        if cfg.block(pred).contains(bflag::LOOP_END) {
            // ignore loop back edge
            continue;
        }
        let top = skip_synthetic_predecessor(cfg, pred);
        if !info.merged_contains(cfg, top) {
            return false;
        }
    }
    true
}

impl RegionMaker<'_> {
    /// `IfRegionMaker.process`
    pub fn process_if(
        &mut self,
        current_region: RegionId,
        block: BlockId,
        ifnode: InsnId,
    ) -> Res<Option<BlockId>> {
        if self.cfg.block(block).contains(bflag::ADDED_TO_REGION) {
            // block already included in other 'if' region
            return Ok(match &self.cfg.insn(ifnode).kind {
                InsnKind::If { then_block, .. } => *then_block,
                _ => None,
            });
        }
        let Some(mut current_if) = make_if_info(self.cfg, block) else {
            return Ok(None);
        };
        match merge_nested_if_nodes(self.cfg, &current_if)? {
            Some(merged) => current_if = merged,
            None => {
                // invert simple condition (compiler often do it)
                // ensure that we only ever invert once
                if !self.cfg.block(block).contains(bflag::DONT_INVERT) {
                    current_if = IfInfo::invert(self.cfg, &current_if);
                    self.cfg.block_mut(block).add(bflag::DONT_INVERT);
                }
            }
        }
        match self.restructure_if(block, &current_if)? {
            Some(modified) => current_if = modified,
            None => {
                if current_if.merged_size() <= 1 {
                    return Ok(None);
                }
                let Some(fresh) = make_if_info(self.cfg, block) else {
                    return Err("NullPointerException: makeIfInfo".to_string());
                };
                match self.restructure_if(block, &fresh)? {
                    Some(r) => current_if = r,
                    None => {
                        // all attempts failed
                        return Ok(None);
                    }
                }
            }
        }
        confirm_merge(self.cfg, &current_if);
        let cond_blocks = current_if.merged_blocks(self.cfg);
        let if_region = self.cfg.regions.new_region(
            Some(current_region),
            RegionKind::If {
                cond: Some(current_if.cond.clone()),
                cond_blocks,
                then_r: None,
                else_r: None,
            },
        );
        self.cfg
            .regions
            .plain_blocks_mut(current_region)
            .push(Container::Region(if_region));
        let out_block = current_if.out_block;
        self.stack.push(if_region)?;
        self.stack.add_exit(out_block);
        let then_region = match current_if.then_block {
            None => {
                // empty then block, not normal, but maybe correct
                self.cfg.regions.new_plain(Some(if_region))
            }
            Some(tb) => self.make_region(tb)?,
        };
        self.set_then(if_region, Some(Container::Region(then_region)));
        let else_region = match current_if.else_block {
            Some(eb) if !self.stack.contains_exit(eb) => Some(self.make_region(eb)?),
            _ => None,
        };
        self.set_else(if_region, else_region.map(Container::Region));
        // insert edge insns in new 'else' branch
        if else_region.is_none() {
            if let Some(out) = out_block {
                let edge_insns = self.cfg.regions.edge_insns_of(out).to_vec();
                if !edge_insns.is_empty() {
                    let mut instructions = Vec::new();
                    for e in &edge_insns {
                        if e.end == out {
                            let start = follow_empty_rev(self.cfg, e.start, true);
                            if current_if.merged_contains(self.cfg, start) {
                                instructions.push(e.insn);
                            }
                        }
                    }
                    if !instructions.is_empty() {
                        let else_r = self.cfg.regions.new_plain(Some(if_region));
                        let c = self.cfg.regions.new_insn_container(instructions);
                        self.cfg.regions.plain_add(else_r, c);
                        self.set_else(if_region, Some(Container::Region(else_r)));
                    }
                }
            }
        }
        self.stack.pop();
        Ok(out_block)
    }

    fn set_then(&mut self, r: RegionId, c: Option<Container>) {
        if let RegionKind::If { then_r, .. } = &mut self.cfg.regions.arena[r].kind {
            *then_r = c;
        }
    }

    fn set_else(&mut self, r: RegionId, c: Option<Container>) {
        if let RegionKind::If { else_r, .. } = &mut self.cfg.regions.arena[r].kind {
            *else_r = c;
        }
    }

    /// `IfRegionMaker.buildIfInfo(LoopRegion)`
    pub fn build_if_info_for_loop(&mut self, header: BlockId) -> Res<IfInfo> {
        let Some(cond_info) = make_if_info(self.cfg, header) else {
            return Err("NullPointerException: loop header condition".to_string());
        };
        let cond_info = search_nested_if(self.cfg, &cond_info)?;
        confirm_merge(self.cfg, &cond_info);
        Ok(cond_info)
    }

    fn restructure_if(&mut self, block: BlockId, info: &IfInfo) -> Res<Option<IfInfo>> {
        let mut info = info.clone();
        let then_block = info.then_block;
        let else_block = info.else_block;
        if then_block == else_block {
            let mut if_info = IfInfo::derive(&info, None, None);
            if_info.out_block = then_block;
            return Ok(Some(if_info));
        }
        let tb = npe(then_block, "thenBlock")?;
        let eb = npe(else_block, "elseBlock")?;
        // select 'then', 'else' and 'exit' blocks
        if self.cfg.block(tb).contains(bflag::RETURN) && self.cfg.block(eb).contains(bflag::RETURN)
        {
            info.out_block = None;
            return Ok(Some(info));
        }
        // init outblock, which will be used in isBadBranchBlock to compare with branch block
        info.out_block = find_out_block(self.cfg, then_block, else_block);
        let bad_then = is_bad_branch_block(self.cfg, &info, tb);
        let bad_else = is_bad_branch_block(self.cfg, &info, eb);
        if bad_then && bad_else {
            return Ok(None);
        }
        if bad_else {
            info = IfInfo::derive(&info, then_block, None);
            info.out_block = Some(eb);
        } else if bad_then {
            info = IfInfo::invert(self.cfg, &info);
            info = IfInfo::derive(&info, else_block, None);
            info.out_block = Some(tb);
        }
        // getPathCross may not find outBlock (e.g. one branch has return, outBlock definitely is
        // null), so should check further
        if info.out_block.is_none() {
            let scope_then = self.find_scope_out_block(info.then_block)?;
            let scope_else = self.find_scope_out_block(info.else_block)?;
            if scope_then.is_none() && scope_else.is_some() {
                info.out_block = scope_else;
            } else if scope_then.is_some() && scope_else.is_none() {
                info.out_block = scope_then;
            } else if scope_then.is_some() && scope_then == scope_else {
                info.out_block = scope_then;
            }
        }
        if let Some(out) = info.out_block {
            if is_back_edge(self.cfg, block, out) {
                info.out_block = None;
            }
        }
        Ok(Some(info))
    }

    /// if startBlock is in a (try) scope, find the scope end as outBlock
    fn find_scope_out_block(&self, start_block: Option<BlockId>) -> Res<Option<BlockId>> {
        let Some(start) = start_block else {
            return Ok(None);
        };
        let dom_frontiers = bit_set_to_blocks(self.cfg, &dom_frontier(self.cfg, start));
        let mut scope_out = None;
        // find handler from domFrontier(could be scope end), if domFrontier is handler
        // and its topSplitter dominates branch block, then branch should end
        for df in dom_frontiers {
            let Some(h) = self.cfg.block(df).exc_handler else {
                continue;
            };
            let Some(tb) = self.cfg.handlers[h].try_block else {
                continue;
            };
            let Some(top_splitter) = self.cfg.try_blocks[tb].top_splitter else {
                return Err("NullPointerException: topSplitter".to_string());
            };
            if is_dominator(self.cfg, start, top_splitter) {
                scope_out = get_try_and_handler_cross_block(self.cfg, h)?;
                break;
            }
        }
        if let Some(scope) = scope_out {
            // check if out block still inside scope limited by 'exit' blocks
            for exit in self.stack.exits() {
                if is_path_exists(self.cfg, exit, scope) {
                    return Ok(None);
                }
            }
        }
        Ok(scope_out)
    }
}
