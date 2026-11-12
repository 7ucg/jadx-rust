//! `LoopRegionMaker`
//!
//! Definitions:
//! main loop body - the set of nodes that form a loop in the control flow graph
//! loop exit edge - an edge with a source in the main loop body and a target outside of it
//! outblock - the first node after the entire loop has finished that should be regioned next
//! header block - an IF node that implements the loop condition

use std::collections::VecDeque;

use crate::model::cfg::{bflag, LoopId, MethodCfg};
use crate::model::condition::IfCondition;
use crate::model::insn::{BlockId, InsnId, InsnType};
use crate::model::region::{Container, EdgeInsn, RegionId, RegionKind};
use crate::passes::block_utils::{is_dominator, is_path_exists};
use crate::passes::regions::block_ext::{
    all_paths_blocks_ordered, bit_set_to_blocks, build_simple_path, follow_empty, follow_empty_rev,
    get_dom_frontier_through_edge, insn_deep_equals, is_empty_simple_path, java_hash_order,
    select_other,
};
use crate::passes::regions::if_maker::{make_if_info, merge_nested_if_nodes, IfInfo};
use crate::passes::regions::maker::{RegionMaker, Res};
use crate::passes::regions::utils::{
    contains_exit_insn, cs, is_exit_block, is_pre_exit_block, is_region_contains_block,
    last_insn_is, next_block,
};

/// `Edge`
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Edge {
    pub source: BlockId,
    pub target: BlockId,
    pub synthetic: bool,
}

impl Edge {
    fn new(source: BlockId, target: BlockId) -> Edge {
        Edge {
            source,
            target,
            synthetic: false,
        }
    }
}

fn npe<T>(v: Option<T>, what: &str) -> Res<T> {
    v.ok_or_else(|| format!("NullPointerException: {what}"))
}

/// `LoopInfo.getExitNodes` (in `HashSet` iteration order)
pub fn loop_exit_nodes(cfg: &MethodCfg, l: LoopId) -> Vec<BlockId> {
    let li = &cfg.loop_infos[l];
    let mut nodes: Vec<BlockId> = Vec::new();
    for block in li.blocks.iter() {
        for &s in &cfg.block(block).succs {
            if !li.blocks.contains(s)
                && cfg.block(s).exc_handler.is_none()
                && !nodes.contains(&block)
            {
                nodes.push(block);
            }
        }
    }
    java_hash_order(&nodes)
}

/// `LoopInfo.getExitEdges`
pub fn loop_exit_edges(cfg: &MethodCfg, l: LoopId) -> Vec<Edge> {
    let li = &cfg.loop_infos[l];
    let mut edges = Vec::new();
    for block in li.blocks.iter() {
        for &s in &cfg.block(block).succs {
            if !li.blocks.contains(s) && !cfg.is_exception_handler_path(s) {
                edges.push(Edge::new(block, s));
            }
        }
    }
    edges
}

/// `LoopInfo.hasParent`
pub fn loop_has_parent(cfg: &MethodCfg, l: LoopId, search: LoopId) -> bool {
    let mut parent = cfg.loop_infos[l].parent;
    while let Some(p) = parent {
        if p == search {
            return true;
        }
        parent = cfg.loop_infos[p].parent;
    }
    false
}

/// `MethodNode.getAllLoopsForBlock`
pub fn all_loops_for_block(cfg: &MethodCfg, block: BlockId) -> Vec<LoopId> {
    cfg.loops
        .iter()
        .copied()
        .filter(|l| cfg.loop_infos[*l].blocks.contains(block))
        .collect()
}

/// `LoopInfo.getPreHeader`
pub fn loop_pre_header(cfg: &MethodCfg, l: LoopId) -> Res<BlockId> {
    let li = &cfg.loop_infos[l];
    select_other(cfg, li.end, &cfg.block(li.start).preds)
}

fn is_exit_at_loop_end(cfg: &MethodCfg, exit: BlockId, l: LoopId) -> bool {
    let loop_end = cfg.loop_infos[l].end;
    if exit == loop_end {
        return true;
    }
    let loop_start = cfg.loop_infos[l].start;
    if cfg.block(loop_start).insns.is_empty() && cfg.block(loop_start).succs == [exit] {
        return false;
    }
    cfg.block(loop_end).insns.is_empty() && cfg.block(loop_end).preds == [exit]
}

impl RegionMaker<'_> {
    /// `LoopRegion.checkPreCondition`
    fn check_pre_condition(&self, pre_condition: BlockId, cond: &Option<IfCondition>) -> bool {
        let cfg = &*self.cfg;
        let insns = cfg.block(pre_condition).insns.clone();
        if insns.is_empty() {
            return true;
        }
        let Some(cond) = cond else {
            return false;
        };
        let cond_args = cond.register_args(cfg);
        if cond_args.is_empty() {
            return false;
        }
        let size = insns.len();
        for (i, &insn_id) in insns.iter().enumerate() {
            let insn = cfg.insn(insn_id);
            let Some(res) = &insn.result else {
                return false;
            };
            let Some(v) = res.svar else {
                return false;
            };
            if cfg.svars[v].uses.len() > 1 {
                return false;
            }
            let res_reg = (res.reg_num, res.svar);
            let mut found = false;
            // search result arg in other insns
            for _j in (i + 1)..size {
                let n = cfg.insn(insns[i]);
                for a in &n.args {
                    if let crate::model::insn::InsnArg::Register(r) = a {
                        if (r.reg_num, r.svar) == res_reg {
                            found = true;
                        }
                    }
                }
            }
            // or in if insn
            if !found
                && cond_args.iter().any(|a| {
                    let r = cfg.reg(*a);
                    (r.reg_num, r.svar) == res_reg
                })
            {
                found = true;
            }
            if !found {
                return false;
            }
        }
        true
    }

    /// `LoopRegionMaker.process`
    pub fn process_loop(&mut self, cur_region: RegionId, l: LoopId) -> Res<Option<BlockId>> {
        let loop_start = self.cfg.loop_infos[l].start;
        let loop_end = self.cfg.loop_infos[l].end;
        let mut exit_set = loop_exit_nodes(self.cfg, l);
        // set exit blocks scan order priority
        // this can help if loop has several exits (after using 'break' or 'return' in loop)
        let mut exit_blocks: Vec<BlockId> = Vec::new();
        let next_start = next_block(self.cfg, loop_start);
        if let Some(ns) = next_start {
            if let Some(p) = exit_set.iter().position(|b| *b == ns) {
                exit_set.remove(p);
                exit_blocks.push(ns);
            }
        }
        if let Some(p) = exit_set.iter().position(|b| *b == loop_start) {
            exit_set.remove(p);
            exit_blocks.push(loop_start);
        }
        if let Some(p) = exit_set.iter().position(|b| *b == loop_end) {
            exit_set.remove(p);
            exit_blocks.push(loop_end);
        }
        exit_blocks.extend(exit_set);
        let loop_region = self.make_loop_region(cur_region, l, &exit_blocks)?;
        let Some(loop_region) = loop_region else {
            let exit = self.make_endless_loop(cur_region, l, loop_start)?;
            self.insert_continue(l);
            return Ok(exit);
        };
        self.cfg
            .regions
            .plain_blocks_mut(cur_region)
            .push(Container::Region(loop_region));
        let outer_region = self.stack.peek_region();
        self.stack.push(loop_region)?;
        let header = match &self.cfg.regions.arena[loop_region].kind {
            RegionKind::Loop { header, .. } => npe(*header, "loop header")?,
            _ => unreachable!(),
        };
        let mut cond_info = self.build_if_info_for_loop(header)?;
        let in_loop = match cond_info.then_block {
            Some(t) => self.cfg.loop_infos[l].blocks.contains(t),
            None => false,
        };
        if !in_loop {
            // invert loop condition if 'then' points to exit
            cond_info = IfInfo::invert(self.cfg, &cond_info);
        }
        let merged = cond_info.merged_blocks(self.cfg);
        if let RegionKind::Loop {
            cond, cond_blocks, ..
        } = &mut self.cfg.regions.arena[loop_region].kind
        {
            *cond = Some(cond_info.cond.clone());
            *cond_blocks = merged.clone();
        }
        // prevent if's merge with loop condition
        for b in &merged {
            self.cfg.block_mut(*b).add(bflag::ADDED_TO_REGION);
        }
        exit_blocks.retain(|b| !merged.contains(b));
        if !exit_blocks.is_empty() {
            // Blocks associated with the loop condition
            let loop_condition_blocks = merged.clone();
            for exit_edge in loop_exit_edges(self.cfg, l) {
                // An exit edge from the loop condition blocks
                if loop_condition_blocks.contains(&exit_edge.source) {
                    let out_block = follow_empty(self.cfg, exit_edge.target);
                    for pred in self.cfg.block(out_block).preds.clone() {
                        // Restarting search through exit edges from the beginning ("top")
                        for top in loop_exit_edges(self.cfg, l) {
                            if !loop_condition_blocks.contains(&top.source)
                                && (is_path_exists(self.cfg, top.target, pred)
                                    || top.target == out_block)
                            {
                                self.insert_loop_break(
                                    l,
                                    out_block,
                                    top.source,
                                    Edge::new(pred, out_block),
                                );
                            }
                        }
                    }
                    // Exit edge found - no need to check further regardless of break outcome
                    break;
                }
            }
        }
        let condition_at_end = match &self.cfg.regions.arena[loop_region].kind {
            RegionKind::Loop {
                condition_at_end, ..
            } => *condition_at_end,
            _ => unreachable!(),
        };
        let out: Option<BlockId>;
        if condition_at_end {
            let then_block = cond_info.then_block;
            let o = if then_block == Some(loop_end) || then_block == Some(loop_start) {
                cond_info.else_block
            } else {
                then_block
            };
            let o = follow_empty(self.cfg, npe(o, "loop out block")?);
            out = Some(o);
            self.cfg.block_mut(loop_start).loops.clear();
            self.cfg.block_mut(loop_end).add(bflag::ADDED_TO_REGION);
            self.stack.add_exit(Some(loop_end));
            self.clear_block_processed_state(loop_start);
            let body = self.make_region(loop_start)?;
            self.set_loop_body(loop_region, Some(body));
            self.cfg.block_mut(loop_start).loops.push(l);
            self.cfg.block_mut(loop_end).flags &= !bflag::ADDED_TO_REGION;
        } else {
            let mut o = cond_info.else_block; // Following Jadx convention, this must be the next synthetic block
            if let (Some(outer), Some(ob)) = (outer_region, o) {
                if self.cfg.block(ob).contains(bflag::LOOP_START)
                    && !self.cfg.block(ob).loops.contains(&l)
                    && is_region_contains_block(self.cfg, Container::Region(outer), ob)
                {
                    // exit to already processed outer loop
                    o = None;
                }
            }
            out = o;
            self.stack.add_exit(out);
            let loop_body = cond_info.then_block;
            let body = if loop_body == Some(loop_start) {
                // empty loop body
                self.cfg.regions.new_plain(Some(loop_region))
            } else {
                self.make_region(npe(loop_body, "loop body")?)?
            };
            // add blocks from loop start to first condition block
            let condition_block = cond_info.first_if_block(self.cfg);
            if loop_start != condition_block {
                let mut blocks = all_paths_blocks_ordered(self.cfg, loop_start, condition_block);
                blocks.retain(|b| *b != condition_block);
                for block in blocks {
                    if self.cfg.block(block).insns.is_empty()
                        && !self.cfg.block(block).contains(bflag::ADDED_TO_REGION)
                        && !is_region_contains_block(self.cfg, Container::Region(body), block)
                    {
                        self.cfg
                            .regions
                            .plain_blocks_mut(body)
                            .push(Container::Block(block));
                    }
                }
            }
            self.set_loop_body(loop_region, Some(body));
        }
        self.stack.pop();
        self.insert_continue(l);
        Ok(out)
    }

    fn set_loop_body(&mut self, region: RegionId, b: Option<RegionId>) {
        if let RegionKind::Loop { body, .. } = &mut self.cfg.regions.arena[region].kind {
            *body = b;
        }
    }

    /// Select loop exit and construct `LoopRegion`
    fn make_loop_region(
        &mut self,
        cur_region: RegionId,
        l: LoopId,
        exit_blocks: &[BlockId],
    ) -> Res<Option<RegionId>> {
        for &block in exit_blocks {
            // Ignore blocks that lead to exception handlers
            if self.cfg.block(block).exc_handler.is_some() {
                continue;
            }
            // Ignore blocks that do not branch based on an if statement
            if !last_insn_is(self.cfg, block, InsnType::If) {
                continue;
            }
            // Skip any nested if statements
            let loops = &self.cfg.block(block).loops;
            if !loops.is_empty() && loops[0] != l {
                // skip nested loop condition
                continue;
            }
            let (loop_start, _loop_end) =
                (self.cfg.loop_infos[l].start, self.cfg.loop_infos[l].end);
            let exit_at_loop_end = is_exit_at_loop_end(self.cfg, block, l);
            // new LoopRegion(curRegion, loop, block, exitAtLoopEnd)
            let cond = IfCondition::from_if_block(self.cfg, block);
            let region = self.cfg.regions.new_region(
                Some(cur_region),
                RegionKind::Loop {
                    info: l,
                    cond,
                    cond_blocks: vec![block],
                    header: Some(block),
                    condition_at_end: exit_at_loop_end,
                    pre_condition: None,
                    body: None,
                    ty: None,
                },
            );
            let mut found;
            if block == loop_start
                || exit_at_loop_end
                || is_empty_simple_path(self.cfg, loop_start, block)
            {
                found = true;
            } else if self.cfg.block(block).preds.contains(&loop_start) {
                let cond = match &self.cfg.regions.arena[region].kind {
                    RegionKind::Loop { cond, .. } => cond.clone(),
                    _ => unreachable!(),
                };
                if let RegionKind::Loop { pre_condition, .. } =
                    &mut self.cfg.regions.arena[region].kind
                {
                    *pre_condition = Some(loop_start);
                }
                // if we can't merge pre-condition this is not correct header
                found = self.check_pre_condition(loop_start, &cond);
            } else {
                found = false;
            }
            if found {
                let list = all_loops_for_block(self.cfg, block);
                if list.len() >= 2 {
                    // bad condition if successors going out of all loops
                    let mut all_outer = true;
                    for outer_block in cs(self.cfg, block) {
                        let mut out_loop_list = all_loops_for_block(self.cfg, outer_block);
                        out_loop_list.retain(|x| *x != l);
                        if !out_loop_list.is_empty() {
                            // goes to outer loop
                            all_outer = false;
                            break;
                        }
                    }
                    if all_outer {
                        found = false;
                    }
                }
            }
            if found && !self.check_loop_exits(l, block)? {
                found = false;
            }
            if found {
                return Ok(Some(region));
            }
        }
        // no exit found => endless loop
        Ok(None)
    }

    /// Check that the exits suggested by treating `main_exit_block` as the header block
    /// are consistent with a loop condition
    fn check_loop_exits(&mut self, l: LoopId, main_exit_block: BlockId) -> Res<bool> {
        let exit_edges = loop_exit_edges(self.cfg, l);
        if exit_edges.len() < 2 {
            return Ok(true);
        }
        // If the header selected does not have an exit edge, raise an exception
        let Some(main_edge) = exit_edges.iter().find(|e| e.source == main_exit_block) else {
            return Err(format!(
                "Not found exit edge by exit block: {main_exit_block}"
            ));
        };
        let main_out_block = main_edge.target;
        let first_work = follow_empty(self.cfg, main_out_block);
        let first_insns = &self.cfg.block(first_work).insns;
        // If there is a direct path to a return from the header, all exits are inside the loop
        if first_insns.len() == 1 && self.cfg.insn(first_insns[0]).ty == InsnType::Return {
            return Ok(true);
        }
        // Otherwise the exit must lead to a valid out block
        self.valid_out_block(first_work, l)
    }

    /// An out block is valid if every exit path passes through it or doesn't cross any other exit
    /// path (permitting one block of duplication)
    fn valid_out_block(&mut self, out_block: BlockId, l: LoopId) -> Res<bool> {
        let exit_edges = loop_exit_edges(self.cfg, l);
        let mut edges_to_check: VecDeque<Edge> = exit_edges.into_iter().collect();
        while let Some(exit_edge) = edges_to_check.pop_front() {
            let exit_block = exit_edge.target;
            // Get the dominance frontier of exitEdge.getTarget() only along paths through exitEdge
            let dominance_frontier = if !exit_edge.synthetic {
                bit_set_to_blocks(
                    self.cfg,
                    &get_dom_frontier_through_edge(self.cfg, exit_edge.target),
                )
            } else {
                bit_set_to_blocks(
                    self.cfg,
                    &crate::passes::regions::block_ext::dom_frontier(self.cfg, exit_edge.target),
                )
            };
            if is_dominator(self.cfg, out_block, exit_block) || out_block == exit_block {
                // Accept if the loop exit block is a dominator of the suggested out block
                continue;
            }
            for crossing in dominance_frontier {
                if crossing == out_block {
                    // Accept if the crossing is at the outblock
                    continue;
                }
                if is_exit_block(self.cfg, crossing) {
                    // Accept if the crossing is at the method end
                    continue;
                }
                // Find the first block after the crossing with instructions
                let mut first_instruction_block = crossing;
                if self.cfg.block(crossing).insns.is_empty() {
                    first_instruction_block = follow_empty(self.cfg, crossing);
                }
                // Return false if the crossing doesn't satisfy any relevant edge case
                if !(self.via_valid_unclean_successor(exit_block, crossing, l)
                    || self.no_work_before_end(first_instruction_block, out_block)
                    || self.one_block_of_work_before_end(first_instruction_block, out_block)
                    || self.is_nested_if_cross(crossing, &mut edges_to_check)?
                    || self.is_outer_outblock(crossing, l))
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn via_valid_unclean_successor(
        &self,
        exit_block: BlockId,
        crossing: BlockId,
        l: LoopId,
    ) -> bool {
        // Return true if the path from exitBlock is to an exception handler or via a backwards loop
        // edge with continue
        if is_path_exists(self.cfg, exit_block, crossing) {
            // This case does not apply if there is a path via clean successors
            return false;
        }
        // If to a loop start check if the backwards edge has a branch without a valid continue
        if self.cfg.block(crossing).contains(bflag::LOOP_START) {
            // Find the outer loop containing the loop start
            let mut parent = self.cfg.loop_infos[l].parent;
            let mut outer_loop = None;
            while let Some(p) = parent {
                if self.cfg.loop_infos[p].start == crossing {
                    outer_loop = Some(p);
                    break;
                }
                parent = self.cfg.loop_infos[p].parent;
            }
            if let Some(outer) = outer_loop {
                let loop_end = self.cfg.loop_infos[outer].end;
                let predecessors = self.cfg.block(loop_end).preds.clone();
                if predecessors.len() > 1 {
                    let outer_exit_nodes = loop_exit_nodes(self.cfg, outer);
                    for &predecessor in &predecessors {
                        // Do not accept if a predecessor to the loop end reachable from the exit
                        // would not have a continue inserted
                        if is_path_exists(self.cfg, exit_block, predecessor)
                            && !can_insert_continue(
                                self.cfg,
                                predecessor,
                                &predecessors,
                                loop_end,
                                &outer_exit_nodes,
                            )
                        {
                            return false;
                        }
                    }
                } else {
                    // Do not accept if no continues would be placed
                    return false;
                }
            }
        }
        // Accept if all branches have a valid continue or if not to a loop start (to an exception
        // handler)
        true
    }

    fn no_work_before_end(&self, first_instruction_block: BlockId, out_block: BlockId) -> bool {
        // Return true if there is no work between the crossing and an exit block
        is_exit_block(self.cfg, first_instruction_block) || first_instruction_block == out_block
    }

    fn one_block_of_work_before_end(
        &self,
        first_instruction_block: BlockId,
        out_block: BlockId,
    ) -> bool {
        // Return true if down every path there is no more than one block of work between the
        // crossing and an exit block
        let clean = cs(self.cfg, first_instruction_block);
        if clean.is_empty() {
            return false;
        }
        for s in clean {
            let next = follow_empty(self.cfg, s);
            if !is_exit_block(self.cfg, next) && next != out_block {
                return false;
            }
        }
        true
    }

    fn is_nested_if_cross(&mut self, crossing: BlockId, edges: &mut VecDeque<Edge>) -> Res<bool> {
        // Return true if the crossing is due to merged control flow after a nested if
        let predecessors = self.cfg.block(crossing).preds.clone();
        if predecessors.is_empty() {
            return Err("IndexOutOfBoundsException: crossing predecessors".to_string());
        }
        // Find a predecessor that dominates all other predecessors
        let mut possible_first_if = follow_empty_rev(self.cfg, predecessors[0], true);
        for &predecessor in &predecessors {
            // Follow the predecessor up to the first node with instructions
            let possible_if = follow_empty_rev(self.cfg, predecessor, true);
            if is_dominator(self.cfg, crossing, possible_if) {
                possible_first_if = possible_if;
            }
        }
        // This case does not apply if a merged if cannot be made
        let Some(current_if) = make_if_info(self.cfg, possible_first_if) else {
            return Ok(false);
        };
        let Some(merged_if) = merge_nested_if_nodes(self.cfg, &current_if)? else {
            return Ok(false);
        };
        // Accept if following every predecessor path from the crossing reaches a merged if node
        for &predecessor in &predecessors {
            let possible_if = follow_empty_rev(self.cfg, predecessor, true);
            if !merged_if.merged_contains(self.cfg, possible_if) {
                return Ok(false);
            }
        }
        // If this crossing is the result of merged ifs, check the next crossing after this one
        let place_holder = Edge {
            source: crossing,
            target: crossing,
            synthetic: true,
        };
        if !edges
            .iter()
            .any(|e| e.source == place_holder.source && e.target == place_holder.target)
        {
            edges.push_back(place_holder);
        }
        Ok(true)
    }

    fn is_outer_outblock(&self, crossing: BlockId, l: LoopId) -> bool {
        // Return true if the crossing is the outblock for an outer loop and is jumped to using a
        // labelled break
        for e in self.cfg.regions.edge_insns_of(crossing) {
            if self.cfg.insn(e.insn).ty == InsnType::Break {
                if let Some(loops) = self.cfg.regions.insn_loops.get(&e.insn) {
                    for &broken_from in loops {
                        // If it is for a parent of the current loop
                        if loop_has_parent(self.cfg, l, broken_from) && e.end == crossing {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn make_endless_loop(
        &mut self,
        cur_region: RegionId,
        l: LoopId,
        loop_start: BlockId,
    ) -> Res<Option<BlockId>> {
        let loop_region = self.cfg.regions.new_region(
            Some(cur_region),
            RegionKind::Loop {
                info: l,
                cond: None,
                cond_blocks: Vec::new(),
                header: None,
                condition_at_end: false,
                pre_condition: None,
                body: None,
                ty: None,
            },
        );
        self.cfg
            .regions
            .plain_blocks_mut(cur_region)
            .push(Container::Region(loop_region));
        self.cfg.block_mut(loop_start).loops.clear();
        self.clear_block_processed_state(loop_start);
        self.stack.push(loop_region)?;
        let mut out: Option<BlockId> = None;
        // insert 'break' for exits
        let exit_edges = loop_exit_edges(self.cfg, l);
        if exit_edges.len() == 1 {
            let exit_edge = exit_edges[0];
            let exit = exit_edge.target;
            if self.insert_loop_break(l, exit, exit_edge.source, exit_edge) {
                if let Some(nb) = next_block(self.cfg, exit) {
                    self.stack.add_exit(Some(nb));
                    out = Some(nb);
                }
            }
        } else {
            'loop0: for exit_edge in &exit_edges {
                let exit = exit_edge.target;
                let mut blocks = bit_set_to_blocks(
                    self.cfg,
                    &get_dom_frontier_through_edge(self.cfg, exit_edge.target),
                );
                // Only select the method exit if there is no other valid outblock
                let method_exit = self.cfg.exit;
                if let Some(p) = blocks.iter().position(|b| *b == method_exit) {
                    blocks.remove(p);
                    blocks.push(method_exit);
                }
                for block in blocks {
                    if is_path_exists(self.cfg, exit, block) {
                        if self.valid_out_block(block, l)? {
                            out = Some(block);
                            break 'loop0;
                        }
                    } else if self.cfg.block(block).contains(bflag::LOOP_START)
                        && self.valid_out_block(exit, l)?
                    {
                        // Special case if there is no joining control flow before an outer loop
                        // back edge
                        out = Some(exit);
                        break 'loop0;
                    }
                }
            }
            // Add breaks
            self.stack.add_exit(out);
            if let Some(o) = out {
                if o != self.cfg.exit {
                    // Add a break on every incoming edge where the predecessor is reachable from
                    // the loop
                    for predecessor in self.cfg.block(o).preds.clone() {
                        for exit_edge in loop_exit_edges(self.cfg, l) {
                            let target = exit_edge.target;
                            if is_path_exists(self.cfg, target, predecessor) || target == o {
                                self.insert_loop_break(
                                    l,
                                    o,
                                    exit_edge.source,
                                    Edge::new(predecessor, o),
                                );
                            }
                        }
                    }
                }
            }
        }
        let body = self.make_region(loop_start)?;
        let loop_end = self.cfg.loop_infos[l].end;
        if !is_region_contains_block(self.cfg, Container::Region(body), loop_end)
            && self.cfg.block(loop_end).exc_handler.is_none()
            && !self.in_exception_handler_blocks(loop_end)
        {
            self.cfg
                .regions
                .plain_blocks_mut(body)
                .push(Container::Block(loop_end));
        }
        self.set_loop_body(loop_region, Some(body));
        if out.is_none() {
            let next = next_block(self.cfg, loop_end);
            out = match next {
                Some(n) if !is_region_contains_block(self.cfg, Container::Region(body), n) => {
                    Some(n)
                }
                _ => None,
            };
        }
        self.stack.pop();
        self.cfg.block_mut(loop_start).loops.push(l);
        Ok(out)
    }

    fn in_exception_handler_blocks(&self, loop_end: BlockId) -> bool {
        if self.cfg.handler_list.is_empty() {
            return false;
        }
        self.cfg
            .handler_list
            .iter()
            .any(|h| self.cfg.handlers[*h].blocks.contains(&loop_end))
    }

    fn can_insert_break(&self, exit: BlockId) -> bool {
        if contains_exit_insn(self.cfg, Container::Block(exit)) {
            return false;
        }
        let simple_path = build_simple_path(self.cfg, exit);
        if let Some(&last) = simple_path.last() {
            if self.cfg.block(last).contains(bflag::MTH_EXIT_BLOCK)
                || self.cfg.block(last).contains(bflag::RETURN)
                || is_pre_exit_block(self.cfg, last)
            {
                return false;
            }
        }
        // check if there no outer switch (TODO: very expensive check)
        let paths = all_paths_blocks_ordered(self.cfg, self.cfg.enter, exit);
        for block in paths {
            if last_insn_is(self.cfg, block, InsnType::Switch) {
                return false;
            }
        }
        true
    }

    /// Insert a break instruction where `exit_edge` meets `loop_exit`
    fn insert_loop_break(
        &mut self,
        l: LoopId,
        loop_exit: BlockId,
        block_on_loop: BlockId,
        exit_edge: Edge,
    ) -> bool {
        let mut exit = Some(exit_edge.target);
        let mut insert_edge: Option<Edge> = None;
        let mut confirm = false;
        // process special cases:
        // 1. jump to outer loop
        let exit_end = follow_empty(self.cfg, exit_edge.target);
        for &loop_at_end in &self.cfg.block(exit_end).loops.clone() {
            if loop_at_end != l && loop_has_parent(self.cfg, l, loop_at_end) {
                insert_edge = Some(exit_edge);
                confirm = true;
                break;
            }
        }
        if !confirm {
            // Start search from the next edge if the target is simple (e.g. first node after loop
            // exit)
            let is_simple = follow_empty(self.cfg, exit_edge.target) != exit_edge.target;
            let mut insert_block = if is_simple {
                None
            } else {
                Some(exit_edge.source)
            };
            let mut visited: Vec<BlockId> = Vec::new();
            while let Some(e) = exit {
                if visited.contains(&e) {
                    break;
                }
                visited.push(e);
                if let Some(ib) = insert_block {
                    if is_path_exists(self.cfg, loop_exit, e) {
                        // found cross
                        if self.can_insert_break(ib) {
                            insert_edge = Some(Edge::new(ib, e));
                            confirm = true;
                            break;
                        }
                        return false;
                    }
                }
                insert_block = Some(e);
                let clean = cs(self.cfg, e);
                exit = if clean.len() == 1 {
                    Some(clean[0])
                } else {
                    None
                };
            }
        }
        if !confirm {
            return false;
        }
        let Some(insert_edge) = insert_edge else {
            return false;
        };
        let break_insn = self.new_plain_insn(InsnType::Break);
        self.cfg.regions.insn_loops.insert(break_insn, vec![l]);
        add_edge_insn(self.cfg, insert_edge.source, insert_edge.target, break_insn);
        self.stack.add_exit(exit);
        // add label to 'break' if needed
        if let Some(e) = exit {
            self.add_break_label(block_on_loop, e, break_insn);
        }
        true
    }

    /// Adds a label to a break instruction if reaching the exit from the loop involves leaving
    /// multiple loops
    fn add_break_label(&mut self, block_on_loop: BlockId, exit: BlockId, break_insn: InsnId) {
        if !all_loops_for_block(self.cfg, exit).is_empty() {
            return;
        }
        let in_loops = all_loops_for_block(self.cfg, block_on_loop);
        if in_loops.len() < 2 {
            return;
        }
        // search for parent loop
        let Some(parent_loop) = in_loops
            .iter()
            .copied()
            .find(|l| self.cfg.loop_infos[*l].parent.is_none())
        else {
            return;
        };
        if self.cfg.loop_infos[parent_loop].end != exit
            && !loop_exit_nodes(self.cfg, parent_loop).contains(&exit)
        {
            self.cfg
                .regions
                .insn_loop_label
                .insert(break_insn, parent_loop);
            let start = self.cfg.loop_infos[parent_loop].start;
            self.cfg.regions.block_loop_label.insert(start, parent_loop);
        }
    }

    fn insert_continue(&mut self, l: LoopId) {
        let loop_end = self.cfg.loop_infos[l].end;
        let predecessors = self.cfg.block(loop_end).preds.clone();
        if predecessors.len() <= 1 {
            return;
        }
        let exit_nodes = loop_exit_nodes(self.cfg, l);
        for &pred in &predecessors {
            if can_insert_continue(self.cfg, pred, &predecessors, loop_end, &exit_nodes) {
                let cont = self.new_plain_insn(InsnType::Continue);
                self.cfg.block_mut(pred).insns.push(cont);
            }
        }
    }
}

/// `EdgeInsnAttr.addEdgeInsn`
pub fn add_edge_insn(cfg: &mut MethodCfg, start: BlockId, end: BlockId, insn: InsnId) {
    for b in [start, end] {
        let exists = cfg
            .regions
            .edge_insns_of(b)
            .iter()
            .any(|x| x.start == start && x.end == end && insn_deep_equals(cfg, x.insn, insn));
        if !exists {
            cfg.regions
                .edge_insns
                .entry(b)
                .or_default()
                .push(EdgeInsn { start, end, insn });
        }
    }
}

fn can_insert_continue(
    cfg: &MethodCfg,
    pred: BlockId,
    predecessors: &[BlockId],
    loop_end: BlockId,
    loop_exit_nodes: &[BlockId],
) -> bool {
    if !cfg.block(pred).contains(bflag::SYNTHETIC) || last_insn_is(cfg, pred, InsnType::Continue) {
        return false;
    }
    let preds = &cfg.block(pred).preds;
    if preds.is_empty() {
        return false;
    }
    let code_pred = preds[0];
    if cfg.block(code_pred).contains(bflag::ADDED_TO_REGION) {
        return false;
    }
    if is_dominator(cfg, loop_end, code_pred) || loop_exit_nodes.contains(&code_pred) {
        return false;
    }
    if is_dominated_on_blocks(cfg, code_pred, predecessors) {
        return false;
    }
    // if we've already inserted a break, don't also insert a continue in the same spot
    if cfg
        .regions
        .edge_insns_of(pred)
        .iter()
        .any(|e| cfg.insn(e.insn).ty == InsnType::Break)
    {
        return false;
    }
    loop_exit_nodes
        .iter()
        .any(|exit| is_path_exists(cfg, code_pred, *exit))
}

fn is_dominated_on_blocks(cfg: &MethodCfg, dom: BlockId, blocks: &[BlockId]) -> bool {
    blocks.iter().all(|n| is_dominator(cfg, *n, dom))
}
