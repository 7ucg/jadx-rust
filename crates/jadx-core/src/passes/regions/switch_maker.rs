//! `SwitchRegionMaker`

use crate::model::bitset::BitSet;
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{iflag, BlockId, InsnArg, InsnId, InsnKind, InsnNode, InsnType};
use crate::model::region::{CaseInfo, CaseKey, Container, RegionId, RegionKind};
use crate::passes::block_utils::is_path_exists;
use crate::passes::regions::block_ext::{
    bit_set_to_blocks, bit_set_to_one_block, blocks_to_bit_set, collect_blocks_dominated_by,
    dom_frontier, follow_empty,
};
use crate::passes::regions::maker::{RegionMaker, Res};
use crate::passes::regions::traversal::{traverse, RegionVisitor};
use crate::passes::regions::utils::{
    add_to_all, cflag, cflag_add, collect_block_nodes, cs, has_exit_block, is_empty,
    is_pre_exit_block, last_insn,
};

fn npe<T>(v: Option<T>, what: &str) -> Res<T> {
    v.ok_or_else(|| format!("NullPointerException: {what}"))
}

/// `LinkedHashMap<BlockNode, List<Object>>`
type BlocksMap = Vec<(BlockId, Vec<CaseKey>)>;

fn map_add(map: &mut BlocksMap, block: BlockId, key: CaseKey) {
    if let Some(e) = map.iter_mut().find(|(b, _)| *b == block) {
        e.1.push(key);
    } else {
        map.push((block, vec![key]));
    }
}

fn pos(cfg: &MethodCfg, b: BlockId) -> usize {
    cfg.block(b).pos
}

fn post_doms(cfg: &MethodCfg, b: BlockId) -> BitSet {
    cfg.block(b).post_doms.clone().unwrap_or_default()
}

impl RegionMaker<'_> {
    /// `SwitchRegionMaker.process`
    pub fn process_switch(
        &mut self,
        current_region: RegionId,
        block: BlockId,
        insn: InsnId,
    ) -> Res<Option<BlockId>> {
        // map case blocks to keys
        let (keys, target_blocks, def_case) = match &self.cfg.insn(insn).kind {
            InsnKind::Switch {
                data,
                target_blocks,
                def_block,
                ..
            } => (
                data.as_ref().map(|d| d.keys.clone()).unwrap_or_default(),
                target_blocks.clone(),
                *def_block,
            ),
            _ => return Err("not a switch".to_string()),
        };
        let target_blocks = npe(target_blocks, "switch target blocks")?;
        let mut blocks_map: BlocksMap = Vec::new();
        for (i, tb) in target_blocks.iter().enumerate() {
            map_add(&mut blocks_map, *tb, CaseKey::Int(keys[i]));
        }
        if let Some(def) = def_case {
            map_add(&mut blocks_map, def, CaseKey::Default);
        }
        let sw = self.cfg.regions.new_region(
            Some(current_region),
            RegionKind::Switch {
                header: block,
                cases: Vec::new(),
            },
        );
        self.cfg.regions.insn_region_ref.insert(insn, sw);
        self.cfg
            .regions
            .plain_blocks_mut(current_region)
            .push(Container::Region(sw));
        self.stack.push(sw)?;
        let out = self.calc_switch_out(block, insn)?;
        self.stack.add_exit(out);
        self.add_cases(sw, block, out, blocks_map)?;
        self.remove_empty_cases(insn, sw, def_case, out);
        self.stack.pop();
        Ok(out)
    }

    fn add_cases(
        &mut self,
        sw: RegionId,
        header: BlockId,
        out: Option<BlockId>,
        mut blocks_map: BlocksMap,
    ) -> Res<()> {
        let mut fall_through_cases: Vec<(BlockId, Option<BlockId>)> = Vec::new();
        if let Some(out) = out {
            // detect fallthrough cases
            let mut case_blocks = blocks_to_bit_set(self.cfg, blocks_map.iter().map(|(b, _)| b));
            case_blocks.clear(pos(self.cfg, out));
            for successor in self.cfg.block(header).succs.clone() {
                let df = dom_frontier(self.cfg, successor);
                if intersects(&df, &case_blocks) {
                    let fall_through_block =
                        self.get_one_intersection_block(out, &case_blocks, &df);
                    put_ft(&mut fall_through_cases, successor, fall_through_block);
                }
            }
            // check fallthrough cases order
            if !fall_through_cases.is_empty()
                && is_bad_cases_order(&blocks_map, &fall_through_cases)
            {
                let new_map = re_order_switch_cases(&blocks_map, &fall_through_cases);
                if is_bad_cases_order(&new_map, &fall_through_cases) {
                    self.cfg.warnings.push(
                        "Can't fix incorrect switch cases order, some code will duplicate"
                            .to_string(),
                    );
                    fall_through_cases.clear();
                } else {
                    blocks_map = new_map;
                }
            }
        }
        for (case_block, keys_list) in blocks_map {
            let case_region;
            if self.stack.contains_exit(case_block) {
                case_region = self.cfg.regions.new_plain(self.stack.peek_region());
            } else {
                let next = fall_through_cases
                    .iter()
                    .find(|(b, _)| *b == case_block)
                    .and_then(|(_, n)| *n);
                self.stack.add_exit(next);
                case_region = self.make_region(case_block)?;
                self.stack.remove_exit(next);
                if let Some(next) = next {
                    self.cfg.block_mut(next).add(bflag::FALL_THROUGH);
                    self.cfg.regions.arena[case_region].flags |= bflag::FALL_THROUGH;
                }
            }
            if let RegionKind::Switch { cases, .. } = &mut self.cfg.regions.arena[sw].kind {
                cases.push(CaseInfo {
                    keys: keys_list,
                    container: Container::Region(case_region),
                });
            }
        }
        Ok(())
    }

    fn get_one_intersection_block(
        &self,
        out: BlockId,
        case_blocks: &BitSet,
        fall_through_set: &BitSet,
    ) -> Option<BlockId> {
        let mut case_exits = BitSet::new();
        case_exits.or(fall_through_set);
        case_exits.clear(pos(self.cfg, out));
        case_exits.and(case_blocks);
        bit_set_to_one_block(self.cfg, &case_exits)
    }

    fn calc_switch_out(&mut self, block: BlockId, insn: InsnId) -> Res<Option<BlockId>> {
        // union of case blocks dominance frontier
        // works if no fallthrough cases and no returns inside switch
        let mut outs = BitSet::new();
        for s in cs(self.cfg, block) {
            if self.cfg.block(s).contains(bflag::LOOP_END) {
                // loop end dom frontier is loop start, ignore it
                continue;
            }
            outs.or(&dom_frontier(self.cfg, s));
        }
        outs.clear(pos(self.cfg, block));
        outs.clear(pos(self.cfg, self.cfg.exit));
        let mut out: Option<BlockId> = None;
        if outs.cardinality() == 1 {
            // single exit
            out = bit_set_to_one_block(self.cfg, &outs);
        } else {
            // several switch exits
            // possible 'return', 'continue' or fallthrough in one of the cases
            let loop_info = self
                .cfg
                .loops
                .iter()
                .copied()
                .find(|l| self.cfg.loop_infos[*l].blocks.contains(block));
            if let Some(l) = loop_info {
                let (ls, le) = (self.cfg.loop_infos[l].start, self.cfg.loop_infos[l].end);
                outs.and_not(&post_doms(self.cfg, ls));
                outs.and_not(&post_doms(self.cfg, le));
                if outs.cardinality() == 2 && outs.get(pos(self.cfg, le)) {
                    // insert 'continue' for cases lead to loop end
                    // expect only 2 exits: loop end and switch out
                    let mut out_list = bit_set_to_blocks(self.cfg, &outs);
                    out_list.retain(|b| *b != le);
                    let possible_out = if out_list.len() == 1 {
                        Some(out_list[0])
                    } else {
                        None
                    };
                    if let Some(po) = possible_out {
                        if self.insert_continue_in_switch(block, po, le) {
                            outs.clear(pos(self.cfg, le));
                            out = Some(po);
                        }
                    }
                }
                if outs.is_empty() {
                    // all exits inside switch, keep inside to exit from loop
                    return Ok(Some(self.cfg.exit));
                }
            }
            if out.is_none() {
                let im_post_dom = npe(self.cfg.block(block).ipost_dom, "iPostDom")?;
                if outs.get(pos(self.cfg, im_post_dom)) {
                    out = Some(im_post_dom);
                } else {
                    outs.and_not(&post_doms(self.cfg, block));
                    out = bit_set_to_one_block(self.cfg, &outs);
                }
            }
        }
        if let Some(o) = out {
            if is_pre_exit_block(self.cfg, o) {
                // include 'return' or 'throw' in case blocks
                out = Some(self.cfg.exit);
            }
        }
        let im_post_dom = self.cfg.block(block).ipost_dom;
        if out.is_none() && im_post_dom == Some(self.cfg.exit) {
            // all exits inside switch
            // check if all returns are equals and should be treated as single out block
            return Ok(Some(self.all_same_returns()));
        }
        let def_target = match &self.cfg.insn(insn).kind {
            InsnKind::Switch { def_block, .. } => *def_block,
            _ => None,
        };
        if let Some(ipd) = im_post_dom {
            if Some(ipd) == def_target
                && cs(self.cfg, block).contains(&ipd)
                && dom_frontier(self.cfg, block).get(pos(self.cfg, ipd))
            {
                // add exit to stop on empty 'default' block
                self.stack.add_exit(Some(ipd));
            }
        }
        if out.is_none() {
            self.cfg.warnings.push(format!(
                "Failed to find 'out' block for switch in {block}. Please report as an issue."
            ));
            // fallback option; should work in most cases
            out = im_post_dom;
        }
        if let Some(o) = out {
            if self.is_processed(o) {
                // 'out' block already processed, prevent endless loop
                // in this case it might be that 'out' is the LOOP_START of a loop and occurs before
                // 'block' just try the immediate post dominator as a fallback
                self.cfg.warnings.push(format!(
                    "Switch 'out' block {o} for {block} already processed. Defaulting to fallback option."
                ));
                out = self.cfg.block(block).ipost_dom;
            }
        }
        Ok(out)
    }

    fn all_same_returns(&mut self) -> BlockId {
        let exit_block = self.cfg.exit;
        let preds = self.cfg.block(exit_block).preds.clone();
        let count = preds.len();
        if count == 1 {
            return preds[0];
        }
        if self.cfg.void_return {
            for &pred in &preds {
                match last_insn(self.cfg, pred) {
                    Some(i) if self.cfg.insn(i).ty == InsnType::Return => {}
                    _ => return exit_block,
                }
            }
        } else {
            let mut return_args: Vec<InsnArg> = Vec::new();
            for &pred in &preds {
                match last_insn(self.cfg, pred) {
                    Some(i) if self.cfg.insn(i).ty == InsnType::Return => {
                        match self.cfg.insn(i).args.first() {
                            Some(a) => return_args.push(a.clone()),
                            None => return exit_block,
                        }
                    }
                    _ => return exit_block,
                }
            }
            let first_arg = return_args[0].clone();
            if let InsnArg::Register(reg) = &first_arg {
                for arg in return_args.iter().skip(1) {
                    match arg {
                        InsnArg::Register(r) if same_code_var(self.cfg, r, reg) => {}
                        _ => return exit_block,
                    }
                }
            } else {
                for arg in return_args.iter().skip(1) {
                    if !crate::passes::regions::block_ext::arg_equals(self.cfg, arg, &first_arg) {
                        return exit_block;
                    }
                }
            }
        }
        // confirmed
        self.stack.add_exits(&preds);
        // ignore other returns
        for &block in preds.iter().skip(1) {
            self.cfg.block_mut(block).add(bflag::REMOVE);
            self.cfg.block_mut(block).add(bflag::ADDED_TO_REGION);
        }
        preds[0]
    }

    /// Remove empty case blocks:
    /// 1. single 'default' case
    /// 2. filler cases if switch is 'packed' and 'default' case is empty
    fn remove_empty_cases(
        &mut self,
        insn: InsnId,
        sw: RegionId,
        def_case: Option<BlockId>,
        out_block: Option<BlockId>,
    ) {
        let cases = match &self.cfg.regions.arena[sw].kind {
            RegionKind::Switch { cases, .. } => cases.clone(),
            _ => return,
        };
        let default_case_is_empty = match def_case {
            None => true,
            Some(_) => cases.iter().any(|c| {
                c.keys.contains(&CaseKey::Default) && self.can_remove(c.container, out_block)
            }),
        };
        if !default_case_is_empty {
            return;
        }
        let packed = match &self.cfg.insn(insn).kind {
            InsnKind::Switch { packed, .. } => *packed,
            _ => false,
        };
        for (idx, case_info) in cases.iter().enumerate().rev() {
            let _ = idx;
            let _ = case_info;
        }
        let mut remove_ids: Vec<usize> = Vec::new();
        for (i, case_info) in cases.iter().enumerate() {
            if self.can_remove(case_info.container, out_block)
                && (case_info.keys.contains(&CaseKey::Default) || packed)
            {
                // Remove case and mark all blocks as don't generate
                add_to_all(self.cfg, case_info.container, bflag::DONT_GENERATE);
                remove_ids.push(i);
            }
        }
        if let RegionKind::Switch { cases, .. } = &mut self.cfg.regions.arena[sw].kind {
            let mut i = 0;
            cases.retain(|_| {
                let keep = !remove_ids.contains(&i);
                i += 1;
                keep
            });
        }
    }

    /// Check container is empty and all paths through container are empty up until `out_block`
    fn can_remove(&self, container: Container, out_block: Option<BlockId>) -> bool {
        if is_empty(self.cfg, Some(container)) {
            match container {
                // Base case - empty path from block node to outBlock
                Container::Block(b) => return Some(follow_empty(self.cfg, b)) == out_block,
                // Recursive case - every subBlock can be removed
                Container::Region(r) => {
                    return self
                        .cfg
                        .regions
                        .sub_blocks(r)
                        .into_iter()
                        .all(|s| self.can_remove(s, out_block));
                }
                Container::Insns(_) => {}
            }
        }
        false
    }

    fn insert_continue_in_switch(
        &mut self,
        switch_block: BlockId,
        switch_out: BlockId,
        loop_end: BlockId,
    ) -> bool {
        let mut inserted = false;
        for case_block in cs(self.cfg, switch_block) {
            if dom_frontier(self.cfg, case_block).get(pos(self.cfg, loop_end))
                && case_block != switch_out
            {
                // search predecessor of loop end on path from this successor
                let list = collect_blocks_dominated_by(self.cfg, case_block, case_block);
                let contains_out = list.contains(&switch_out)
                    || self
                        .cfg
                        .block(switch_out)
                        .preds
                        .iter()
                        .any(|p| list.contains(p));
                if contains_out {
                    // 'continue' not needed
                } else {
                    for p in self.cfg.block(loop_end).preds.clone() {
                        if list.contains(&p) || p == case_block {
                            if self.cfg.block(p).contains(bflag::SYNTHETIC) {
                                let cont = self.new_plain_insn(InsnType::Continue);
                                self.cfg.block_mut(p).insns.push(cont);
                                inserted = true;
                            }
                            break;
                        }
                    }
                }
            }
        }
        inserted
    }
}

fn put_ft(map: &mut Vec<(BlockId, Option<BlockId>)>, k: BlockId, v: Option<BlockId>) {
    if let Some(e) = map.iter_mut().find(|(b, _)| *b == k) {
        e.1 = v;
    } else {
        map.push((k, v));
    }
}

fn get_ft(map: &[(BlockId, Option<BlockId>)], k: BlockId) -> Option<BlockId> {
    map.iter().find(|(b, _)| *b == k).and_then(|(_, v)| *v)
}

fn intersects(a: &BitSet, b: &BitSet) -> bool {
    a.iter().any(|i| b.get(i))
}

fn is_bad_cases_order(blocks_map: &BlocksMap, ft: &[(BlockId, Option<BlockId>)]) -> bool {
    let mut next_case_block: Option<BlockId> = None;
    for (case_block, _) in blocks_map {
        if let Some(n) = next_case_block {
            if *case_block != n {
                return true;
            }
        }
        next_case_block = get_ft(ft, *case_block);
    }
    next_case_block.is_some()
}

fn re_order_switch_cases(blocks_map: &BlocksMap, ft: &[(BlockId, Option<BlockId>)]) -> BlocksMap {
    let mut list: Vec<BlockId> = blocks_map.iter().map(|(b, _)| *b).collect();
    crate::model::bitset::java_sort(&mut list, |a, b| {
        let next_a = get_ft(ft, *a);
        if let Some(na) = next_a {
            if *b == na {
                return -1;
            }
        } else if get_ft(ft, *b) == Some(*a) {
            return 1;
        }
        0
    });
    list.into_iter()
        .map(|k| {
            let keys = blocks_map
                .iter()
                .find(|(b, _)| *b == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            (k, keys)
        })
        .collect()
}

fn same_code_var(
    cfg: &MethodCfg,
    a: &crate::model::insn::RegisterArg,
    b: &crate::model::insn::RegisterArg,
) -> bool {
    let (Some(x), Some(y)) = (a.svar, b.svar) else {
        return false;
    };
    cfg.svars[x].code_var == cfg.svars[y].code_var
}

// ---- insert breaks (PostProcessRegions) ---------------------------------------------------------

struct BreakInserter {
    switch_region: RegionId,
    case_container: Container,
    case_blocks: Vec<BlockId>,
}

impl RegionVisitor for BreakInserter {
    fn leave_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<()> {
        let mut insert_break = false;
        if Container::Region(region) == self.case_container {
            // top region
            insert_break = true;
        } else {
            let subs = cfg.regions.sub_blocks(region);
            if let Some(Container::Block(last_block)) = subs.last() {
                for successor in &cfg.block(*last_block).succs {
                    if !self.case_blocks.contains(successor) {
                        insert_break = true;
                        break;
                    }
                }
            }
        }
        if insert_break && can_append_break(cfg, region) {
            let c = build_break_container(cfg, self.switch_region);
            if matches!(
                cfg.regions.arena[region].kind,
                RegionKind::Plain { .. } | RegionKind::Sync { .. }
            ) {
                cfg.regions.plain_blocks_mut(region).push(c);
            }
        }
        Ok(())
    }
}

/// `SwitchRegionMaker.insertBreaks`
pub fn insert_breaks(cfg: &mut MethodCfg, sw: RegionId) -> Res<()> {
    let cases = match &cfg.regions.arena[sw].kind {
        RegionKind::Switch { cases, .. } => cases.clone(),
        _ => return Ok(()),
    };
    for case_info in cases {
        let case_blocks = collect_block_nodes(cfg, case_info.container);
        let mut v = BreakInserter {
            switch_region: sw,
            case_container: case_info.container,
            case_blocks,
        };
        traverse(cfg, case_info.container, &mut v)?;
    }
    Ok(())
}

/// `SwitchRegionMaker.canAppendBreak`
pub fn can_append_break(cfg: &MethodCfg, region: RegionId) -> bool {
    !cflag(cfg, Container::Region(region), bflag::FALL_THROUGH)
        && !has_exit_block(cfg, Some(Container::Region(region)))
}

/// `SwitchRegionMaker.buildBreakContainer`
pub fn build_break_container(cfg: &mut MethodCfg, switch_region: RegionId) -> Container {
    let mut n = InsnNode::plain(InsnType::Break);
    n.flags |= iflag::SYNTHETIC;
    let id = cfg.add_insn(n);
    cfg.regions.insn_region_ref.insert(id, switch_region);
    cfg.regions.new_insn_container(vec![id])
}

#[allow(dead_code)]
fn unused(cfg: &mut MethodCfg, c: Container) {
    cflag_add(cfg, c, 0);
    let _ = is_path_exists;
}
