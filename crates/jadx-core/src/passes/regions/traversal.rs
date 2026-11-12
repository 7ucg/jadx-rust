//! `DepthRegionTraversal` and the region visitor interfaces.

use crate::model::cfg::MethodCfg;
use crate::model::region::{Container, RegionId};
use crate::passes::regions::maker::Res;

const ITERATIVE_LIMIT_MULTIPLIER: usize = 5;

/// `IRegionVisitor`
pub trait RegionVisitor {
    fn enter_region(&mut self, _cfg: &mut MethodCfg, _region: RegionId) -> Res<bool> {
        Ok(true)
    }

    /// `processBlock`: `block` is a `Container::Block` or `Container::Insns`.
    fn process_block(&mut self, _cfg: &mut MethodCfg, _block: Container) -> Res<()> {
        Ok(())
    }

    fn leave_region(&mut self, _cfg: &mut MethodCfg, _region: RegionId) -> Res<()> {
        Ok(())
    }
}

enum Item {
    Cont(Container),
    Leave,
}

/// `DepthRegionTraversal.traverse`
pub fn traverse(cfg: &mut MethodCfg, start: Container, visitor: &mut dyn RegionVisitor) -> Res<()> {
    let mut stack: Vec<Item> = vec![Item::Cont(start)];
    let mut leave_stack: Vec<RegionId> = Vec::new();
    while let Some(current) = stack.pop() {
        match current {
            Item::Leave => {
                let region = leave_stack.pop().ok_or("empty leave stack")?;
                visitor.leave_region(cfg, region)?;
            }
            Item::Cont(Container::Region(region)) => {
                let visit = visitor.enter_region(cfg, region)?;
                stack.push(Item::Leave);
                leave_stack.push(region);
                if visit {
                    // add in reverse order to keep original order during visit
                    for c in cfg.regions.sub_blocks(region).into_iter().rev() {
                        stack.push(Item::Cont(c));
                    }
                }
            }
            Item::Cont(c) => visitor.process_block(cfg, c)?,
        }
    }
    Ok(())
}

/// `DepthRegionTraversal.traverse(mth, visitor)` from the method region.
pub fn traverse_method(cfg: &mut MethodCfg, visitor: &mut dyn RegionVisitor) -> Res<()> {
    match cfg.regions.method_region {
        Some(r) => traverse(cfg, Container::Region(r), visitor),
        None => Ok(()),
    }
}

/// One step of `traverseIterative`: returns `true` when the visitor changed something.
fn iterative_step(
    cfg: &mut MethodCfg,
    visit: &mut dyn FnMut(&mut MethodCfg, RegionId) -> Res<bool>,
    start: Option<RegionId>,
) -> Res<bool> {
    let mut stack: Vec<RegionId> = Vec::new();
    if let Some(s) = start {
        stack.push(s);
    }
    while let Some(region) = stack.pop() {
        if visit(cfg, region)? {
            return Ok(true);
        }
        // add in reverse order to keep original order during visit
        for c in cfg.regions.sub_blocks(region).into_iter().rev() {
            if let Container::Region(r) = c {
                stack.push(r);
            }
        }
    }
    Ok(false)
}

/// `DepthRegionTraversal.traverseIterative`
pub fn traverse_iterative(
    cfg: &mut MethodCfg,
    visit: &mut dyn FnMut(&mut MethodCfg, RegionId) -> Res<bool>,
) -> Res<()> {
    let limit = ITERATIVE_LIMIT_MULTIPLIER * cfg.blocks.len();
    let mut k = 0;
    loop {
        let start = cfg.regions.method_region;
        let repeat = iterative_step(cfg, visit, start)?;
        let over = k > limit;
        k += 1;
        if over {
            return Err(format!(
                "Iterative traversal limit reached: limit: {limit}, blocks count: {}",
                cfg.blocks.len()
            ));
        }
        if !repeat {
            return Ok(());
        }
    }
}

/// `DepthRegionTraversal.traverseIncludingExcHandlers`
pub fn traverse_including_exc_handlers(
    cfg: &mut MethodCfg,
    visit: &mut dyn FnMut(&mut MethodCfg, RegionId) -> Res<bool>,
) -> Res<()> {
    let limit = ITERATIVE_LIMIT_MULTIPLIER * cfg.blocks.len();
    let mut k = 0;
    loop {
        let start = cfg.regions.method_region;
        let mut repeat = iterative_step(cfg, visit, start)?;
        if !repeat {
            for h in cfg.handler_list.clone() {
                let hr = match cfg.regions.handler_regions.get(&h) {
                    Some(Container::Region(r)) => Some(*r),
                    _ => None,
                };
                repeat = iterative_step(cfg, visit, hr)?;
                if repeat {
                    break;
                }
            }
        }
        let over = k > limit;
        k += 1;
        if over {
            return Err(format!(
                "Iterative traversal limit reached: limit: {limit}, blocks count: {}",
                cfg.blocks.len()
            ));
        }
        if !repeat {
            return Ok(());
        }
    }
}
