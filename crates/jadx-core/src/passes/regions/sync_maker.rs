//! `SynchronizedRegionMaker` and `ExcHandlersRegionMaker`.

use std::collections::HashSet;

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{iflag, BlockId, InsnArg, InsnId, InsnKind, InsnType};
use crate::model::region::{Container, RegionId, RegionKind};
use crate::passes::block_utils::{get_path_cross_pair, is_path_exists};
use crate::passes::regions::block_ext::{
    arg_equals, bit_set_to_blocks, build_simple_path, dom_frontier, get_top_splitter_for_handler,
    get_try_and_handler_cross_block, java_hash_order,
};
use crate::passes::regions::clean::clean_regions;
use crate::passes::regions::maker::{RegionMaker, Res};
use crate::passes::regions::utils::{
    cs, get_all_region_blocks, get_last_block, is_exit_block, is_region_contains_block, next_block,
};

impl RegionMaker<'_> {
    /// `SynchronizedRegionMaker.process`
    pub fn process_sync(
        &mut self,
        cur_region: RegionId,
        block: BlockId,
        insn: InsnId,
    ) -> Res<Option<BlockId>> {
        let sync = self.cfg.regions.new_region(
            Some(cur_region),
            RegionKind::Sync {
                enter_insn: insn,
                exit_insns: Vec::new(),
                inner: 0,
            },
        );
        let inner = self.cfg.regions.new_plain(Some(sync));
        if let RegionKind::Sync { inner: i, .. } = &mut self.cfg.regions.arena[sync].kind {
            *i = inner;
        }
        self.cfg
            .regions
            .plain_blocks_mut(inner)
            .push(Container::Block(block));
        self.cfg
            .regions
            .plain_blocks_mut(cur_region)
            .push(Container::Region(sync));
        let mut exits: Vec<BlockId> = Vec::new();
        let mut cache: HashSet<BlockId> = HashSet::new();
        let arg = self
            .cfg
            .insn(insn)
            .args
            .first()
            .cloned()
            .ok_or("monitor-enter without argument")?;
        let mut exit_insns: Vec<InsnId> = Vec::new();
        traverse_monitor_exits(
            self.cfg,
            &mut exit_insns,
            &arg,
            block,
            &mut exits,
            &mut cache,
        );
        if let RegionKind::Sync { exit_insns: e, .. } = &mut self.cfg.regions.arena[sync].kind {
            *e = exit_insns.clone();
        }
        for exit_insn in exit_insns {
            if let Some(insn_block) = self.cfg.block_by_insn(exit_insn) {
                self.cfg.block_mut(insn_block).add(bflag::DONT_GENERATE);
            }
            // remove arg from MONITOR_EXIT to allow inline in MONITOR_ENTER
            self.cfg.insn_remove_arg(exit_insn, 0);
            self.cfg.insns[exit_insn].flags |= iflag::DONT_GENERATE;
        }
        let Some(body) = next_block(self.cfg, block) else {
            self.cfg
                .warnings
                .push("Unexpected end of synchronized block".to_string());
            return Ok(None);
        };
        let mut exit: Option<BlockId> = None;
        if exits.len() == 1 {
            exit = next_block(self.cfg, exits[0]);
        } else if exits.len() > 1 {
            cache.clear();
            exit = traverse_monitor_exits_cross(self.cfg, body, &exits, &mut cache);
        }
        self.stack.push(sync)?;
        if let Some(e) = exit {
            self.stack.add_exit(Some(e));
        } else {
            for &exit_block in &exits {
                // don't add exit blocks which leads to method end blocks ('return', 'throw', etc)
                let list = build_simple_path(self.cfg, exit_block);
                if list.is_empty() || !is_exit_block(self.cfg, *list.last().unwrap()) {
                    self.stack.add_exit(Some(exit_block));
                    // we can still try using this as an exit block to make sure it's visited.
                    exit = Some(exit_block);
                }
            }
        }
        let body_region = self.make_region(body)?;
        self.cfg
            .regions
            .plain_blocks_mut(sync)
            .push(Container::Region(body_region));
        self.stack.pop();
        Ok(exit)
    }
}

/// Traverse from monitor-enter thru successors and collect blocks contains monitor-exit
fn traverse_monitor_exits(
    cfg: &MethodCfg,
    exit_insns: &mut Vec<InsnId>,
    arg: &InsnArg,
    block: BlockId,
    exits: &mut Vec<BlockId>,
    visited: &mut HashSet<BlockId>,
) {
    visited.insert(block);
    for &insn in &cfg.block(block).insns {
        let n = cfg.insn(insn);
        if n.ty == InsnType::MonitorExit && !n.args.is_empty() && arg_equals(cfg, &n.args[0], arg) {
            if !exits.contains(&block) {
                exits.push(block);
            }
            exit_insns.push(insn);
            return;
        }
    }
    for &node in &cfg.block(block).succs {
        if !visited.contains(&node) {
            traverse_monitor_exits(cfg, exit_insns, arg, node, exits, visited);
        }
    }
}

/// Traverse from monitor-enter thru successors and search for exit paths cross
fn traverse_monitor_exits_cross(
    cfg: &MethodCfg,
    block: BlockId,
    exits: &[BlockId],
    visited: &mut HashSet<BlockId>,
) -> Option<BlockId> {
    visited.insert(block);
    for node in cs(cfg, block) {
        let mut cross = true;
        for &exit_block in exits {
            if !is_path_exists(cfg, exit_block, node) {
                cross = false;
                break;
            }
        }
        if cross {
            return Some(node);
        }
        if !visited.contains(&node) {
            if let Some(res) = traverse_monitor_exits_cross(cfg, node, exits, visited) {
                return Some(res);
            }
        }
    }
    None
}

/// `SynchronizedRegionMaker.removeSynchronized`
pub fn remove_synchronized(
    cfg: &mut MethodCfg,
    is_static: bool,
    class_type: &crate::model::types::ArgType,
) -> Res<()> {
    let Some(start_region) = cfg.regions.method_region else {
        return Ok(());
    };
    let subs = cfg.regions.sub_blocks(start_region);
    let Some(Container::Region(first)) = subs.first().copied() else {
        return Ok(());
    };
    let (sync_insn, exit_insns, inner) = match &cfg.regions.arena[first].kind {
        RegionKind::Sync {
            enter_insn,
            exit_insns,
            inner,
        } => (*enter_insn, exit_insns.clone(), *inner),
        _ => return Ok(()),
    };
    if can_remove_sync_block(cfg, is_static, class_type, sync_insn) {
        // replace synchronized block with an inner region
        if let RegionKind::Plain { blocks } = &mut cfg.regions.arena[start_region].kind {
            blocks[0] = Container::Region(inner);
        }
        // remove 'monitor-enter' instruction
        cfg.remove_insn(sync_insn)?;
        // remove 'monitor-exit' instruction
        for exit in exit_insns {
            cfg.remove_insn(exit)?;
        }
        // run region cleaner again
        clean_regions(cfg)?;
        // assume that CodeShrinker will be run after this
    }
    Ok(())
}

fn can_remove_sync_block(
    cfg: &mut MethodCfg,
    is_static: bool,
    class_type: &crate::model::types::ArgType,
    sync_insn: InsnId,
) -> bool {
    let Some(sync_arg) = cfg.insn(sync_insn).args.first().cloned() else {
        return false;
    };
    if is_static {
        if let InsnArg::Wrapped(w) = &sync_arg {
            // `syncArg.isConst()`
            if crate::passes::insn_utils::is_const_insn(cfg.insn(w.insn).ty) {
                if let InsnKind::ConstClass(t) = &cfg.insn(w.insn).kind {
                    if cfg.insn(w.insn).ty == InsnType::ConstClass && t == class_type {
                        return true;
                    }
                }
            }
        }
        cfg.warnings.push(
            "In static synchronized method top region not synchronized by class const".to_string(),
        );
    } else {
        if matches!(&sync_arg, InsnArg::Register(r) if r.is_this) {
            return true;
        }
        cfg.warnings
            .push("In synchronized method top region not synchronized by 'this'".to_string());
    }
    false
}

// ---- ExcHandlersRegionMaker ---------------------------------------------------------------------

impl RegionMaker<'_> {
    /// `ExcHandlersRegionMaker.process`
    pub fn process_exc_handlers(&mut self) -> Res<()> {
        if self.cfg.handler_list.is_empty() {
            return Ok(());
        }
        let out = self.collect_handler_regions()?;
        if let (Some(out), Some(mr)) = (out, self.cfg.regions.method_region) {
            self.cfg.regions.plain_add(mr, Container::Region(out));
        }
        Ok(())
    }

    fn collect_handler_regions(&mut self) -> Res<Option<RegionId>> {
        let tcs = self.cfg.try_list.clone();
        for &tc in &tcs {
            let handlers = self.cfg.try_blocks[tc].handlers.clone();
            let mut blocks: Vec<BlockId> = Vec::new();
            let mut splitters: Vec<BlockId> = Vec::new();
            for &h in &handlers {
                if let Some(hb) = self.cfg.handlers[h].handler_block {
                    blocks.push(hb);
                    let ts = get_top_splitter_for_handler(self.cfg, hb)?;
                    if !splitters.contains(&ts) {
                        splitters.push(ts);
                    }
                } else {
                    self.cfg
                        .warnings
                        .push("No exception handler block".to_string());
                }
            }
            let mut exits: Vec<BlockId> = Vec::new();
            for &splitter in &splitters {
                for &handler in &blocks {
                    if self.cfg.block(handler).contains(bflag::REMOVE) {
                        continue;
                    }
                    let s = &self.cfg.block(splitter).succs;
                    if s.is_empty() {
                        continue;
                    }
                    let ss = s[0];
                    let cross = get_path_cross_pair(self.cfg, ss, handler);
                    if let Some(cross) = cross {
                        if cross != ss && cross != handler && !exits.contains(&cross) {
                            exits.push(cross);
                        }
                    }
                }
            }
            for &h in &handlers {
                self.process_exc_handler(h, &exits)?;
            }
        }
        self.process_handlers_out_blocks(&tcs)
    }

    /// Search handlers successor blocks aren't included in any region.
    fn process_handlers_out_blocks(&mut self, tcs: &[usize]) -> Res<Option<RegionId>> {
        let Some(mth_region) = self.cfg.regions.method_region else {
            return Ok(None);
        };
        let mut all_region_blocks: Vec<Container> = Vec::new();
        get_all_region_blocks(
            self.cfg,
            Container::Region(mth_region),
            &mut all_region_blocks,
        );
        let mut successor_blocks: Vec<BlockId> = Vec::new();
        for &tc in tcs {
            for &h in &self.cfg.try_blocks[tc].handlers.clone() {
                if let Some(region) = self.cfg.regions.handler_regions.get(&h).copied() {
                    if let Some(Container::Block(last)) = get_last_block(self.cfg, region) {
                        for &s in &self.cfg.block(last).succs {
                            if !successor_blocks.contains(&s) {
                                successor_blocks.push(s);
                            }
                        }
                    }
                    get_all_region_blocks(self.cfg, region, &mut all_region_blocks);
                }
            }
        }
        let ordered = java_hash_order(&successor_blocks);
        let remaining: Vec<BlockId> = ordered
            .into_iter()
            .filter(|b| !all_region_blocks.contains(&Container::Block(*b)))
            .collect();
        if remaining.is_empty() {
            return Ok(None);
        }
        let exc_out_region = self.cfg.regions.new_plain(Some(mth_region));
        for block in remaining {
            self.stack.clear();
            self.stack.push(exc_out_region)?;
            let r = self.make_region(block)?;
            self.cfg
                .regions
                .plain_add(exc_out_region, Container::Region(r));
        }
        Ok(Some(exc_out_region))
    }

    fn process_exc_handler(&mut self, handler: usize, exits: &[BlockId]) -> Res<()> {
        let Some(start) = self.cfg.handlers[handler].handler_block else {
            return Ok(());
        };
        self.stack.clear();
        let dom;
        if self.cfg.handlers[handler].is_finally {
            dom = get_top_splitter_for_handler(self.cfg, start)?;
        } else {
            dom = start;
            self.stack.add_exits(exits);
        }
        if self.cfg.block(dom).contains(bflag::REMOVE) {
            return Ok(());
        }
        let mut handler_exits: Vec<BlockId> = Vec::new();
        match get_try_and_handler_cross_block(self.cfg, handler)? {
            // ensure frontier's other predecessors comes from try end
            Some(out) => handler_exits.push(out),
            // fallback to simple frontier
            None => handler_exits.extend(bit_set_to_blocks(self.cfg, &dom_frontier(self.cfg, dom))),
        }
        let in_loop = self
            .cfg
            .loops
            .iter()
            .any(|l| self.cfg.loop_infos[*l].blocks.contains(start));
        let mth_region = self.cfg.regions.method_region;
        for exit in handler_exits {
            if (!in_loop || is_path_exists(self.cfg, start, exit))
                && mth_region.is_some_and(|mr| {
                    is_region_contains_block(self.cfg, Container::Region(mr), exit)
                })
            {
                self.stack.add_exit(Some(exit));
            }
        }
        let region = self.make_region(start)?;
        self.cfg
            .regions
            .handler_regions
            .insert(handler, Container::Region(region));
        match self.cfg.block(start).exc_handler {
            None => self.cfg.warnings.push(format!(
                "Missing exception handler attribute for start block: {start}"
            )),
            Some(h) => self.cfg.regions.arena[region].exc_handler = Some(h),
        }
        Ok(())
    }
}
