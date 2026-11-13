//! `CleanRegions`

use crate::model::cfg::MethodCfg;
use crate::model::region::{Container, RegionId, RegionKind};
use crate::passes::regions::maker::Res;
use crate::passes::regions::traversal::{traverse_method, RegionVisitor};
use crate::passes::regions::utils::cflag;

struct RemoveRegionVisitor;

fn can_remove_region(cfg: &MethodCfg, c: Container) -> bool {
    if cflag(cfg, c, crate::model::cfg::bflag::DONT_GENERATE) {
        return true;
    }
    match c {
        Container::Block(b) => cfg.block(b).insns.is_empty(),
        Container::Insns(_) => false,
        Container::Region(r) => {
            if let RegionKind::Loop { header: None, .. } = &cfg.regions.arena[r].kind {
                // keep empty endless loops
                return false;
            }
            cfg.regions
                .sub_blocks(r)
                .into_iter()
                .all(|s| can_remove_region(cfg, s))
        }
    }
}

impl RegionVisitor for RemoveRegionVisitor {
    fn enter_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
        if let RegionKind::Plain { blocks } = &cfg.regions.arena[region].kind {
            let keep: Vec<Container> = blocks
                .iter()
                .copied()
                .filter(|c| !can_remove_region(cfg, *c))
                .collect();
            if let RegionKind::Plain { blocks } = &mut cfg.regions.arena[region].kind {
                *blocks = keep;
            }
        }
        Ok(true)
    }
}

/// `CleanRegions.process`
pub fn clean_regions(cfg: &mut MethodCfg) -> Res<()> {
    if cfg.blocks.is_empty() {
        return Ok(());
    }
    traverse_method(cfg, &mut RemoveRegionVisitor)
}
