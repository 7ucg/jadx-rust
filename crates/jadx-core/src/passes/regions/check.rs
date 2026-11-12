//! `CheckRegions`: report blocks that are not part of the region tree.

use std::collections::HashSet;

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::BlockId;
use crate::model::region::{Container, RegionId, RegionKind};
use crate::passes::regions::maker::Res;
use crate::passes::regions::traversal::{traverse_method, RegionVisitor};

struct Collect {
    blocks: HashSet<BlockId>,
}

impl RegionVisitor for Collect {
    fn process_block(&mut self, _cfg: &mut MethodCfg, container: Container) -> Res<()> {
        if let Container::Block(b) = container {
            self.blocks.insert(b);
        }
        Ok(())
    }
}

struct LoopCheck;

impl RegionVisitor for LoopCheck {
    fn enter_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
        if let RegionKind::Loop {
            header: Some(h), ..
        } = &cfg.regions.arena[region].kind
        {
            let h = *h;
            // check loop conditions
            if !cfg.block(h).contains(bflag::ALLOW_MULTIPLE_INSNS_LOOP_COND)
                && cfg.block(h).insns.len() != 1
            {
                cfg.warnings
                    .push(format!("Incorrect condition in loop: {h}"));
            }
        }
        Ok(true)
    }
}

/// `CheckRegions.visit`
pub fn check_regions(cfg: &mut MethodCfg) -> Res<()> {
    if cfg.regions.method_region.is_none() || cfg.blocks.is_empty() || !cfg.errors.is_empty() {
        return Ok(());
    }
    // check if all blocks included in regions
    let mut collect = Collect {
        blocks: HashSet::new(),
    };
    traverse_method(cfg, &mut collect)?;
    if cfg.blocks.len() != collect.blocks.len() {
        for &block in &cfg.blocks.clone() {
            let b = cfg.block(block);
            if !collect.blocks.contains(&block)
                && !b.insns.is_empty()
                && !b.contains(bflag::ADDED_TO_REGION)
                && !b.contains(bflag::DONT_GENERATE)
                && !b.contains(bflag::REMOVE)
            {
                cfg.warnings.push(format!(
                    "Code restructure failed: missing block: {block}, code lost"
                ));
            }
        }
    }
    traverse_method(cfg, &mut LoopCheck)
}
