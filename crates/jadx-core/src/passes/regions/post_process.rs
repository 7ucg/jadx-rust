//! `PostProcessRegions`

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::region::{Container, RegionId, RegionKind};
use crate::passes::regions::maker::Res;
use crate::passes::regions::switch_maker::insert_breaks;
use crate::passes::regions::traversal::{traverse_method, RegionVisitor};

struct PostProcess;

impl RegionVisitor for PostProcess {
    fn leave_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<()> {
        match &cfg.regions.arena[region].kind {
            RegionKind::Loop { .. } => merge_pre_condition(cfg, region),
            RegionKind::Switch { .. } => insert_breaks(cfg, region)?,
            RegionKind::Plain { .. } => insert_edge_insn(cfg, region),
            _ => {}
        }
        Ok(())
    }
}

/// `LoopRegion.mergePreCondition`: move all preCondition block instructions before condition block
/// instructions.
fn merge_pre_condition(cfg: &mut MethodCfg, region: RegionId) {
    let (pre, header) = match &cfg.regions.arena[region].kind {
        RegionKind::Loop {
            pre_condition,
            header,
            ..
        } => (*pre_condition, *header),
        _ => return,
    };
    if let (Some(pre), Some(header)) = (pre, header) {
        let mut merged = std::mem::take(&mut cfg.block_mut(pre).insns);
        merged.extend(cfg.block(header).insns.iter().copied());
        cfg.block_mut(header).insns = merged;
        cfg.block_mut(header)
            .add(bflag::ALLOW_MULTIPLE_INSNS_LOOP_COND);
        if let RegionKind::Loop { pre_condition, .. } = &mut cfg.regions.arena[region].kind {
            *pre_condition = None;
        }
    }
}

/// Insert insn block from edge insn attribute.
fn insert_edge_insn(cfg: &mut MethodCfg, region: RegionId) {
    let subs = cfg.regions.sub_blocks(region);
    let Some(&last) = subs.last() else {
        return;
    };
    let Container::Block(last_block) = last else {
        // edge instructions are only attached to blocks
        return;
    };
    let Some(attr) = cfg.regions.edge_insns_of(last_block).first().copied() else {
        return;
    };
    if attr.start != last_block {
        return;
    }
    if cfg.block(last_block).insns.is_empty() {
        cfg.block_mut(last_block).insns.push(attr.insn);
        return;
    }
    let c = cfg.regions.new_insn_container(vec![attr.insn]);
    cfg.regions.plain_add(region, c);
}

/// `PostProcessRegions.process`
pub fn post_process_regions(cfg: &mut MethodCfg) -> Res<()> {
    traverse_method(cfg, &mut PostProcess)
}
