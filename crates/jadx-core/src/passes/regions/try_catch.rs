//! `ProcessTryCatchRegions`: extract blocks to separate try/catch regions.

use crate::model::bitset::java_sort;
use crate::model::cfg::{bflag, MethodCfg, TryId};
use crate::model::region::{Container, RegionId, RegionKind};
use crate::passes::regions::maker::Res;
use crate::passes::regions::traversal::traverse_including_exc_handlers;
use crate::passes::regions::utils::{has_path_through_block, is_path_exists_to_container};

/// `ProcessTryCatchRegions.process`
pub fn process_try_catch_regions(cfg: &mut MethodCfg) -> Res<()> {
    if cfg.handler_list.is_empty() {
        return Ok(());
    }
    let mut try_blocks = collect_try_catch_blocks(cfg);
    if try_blocks.is_empty() {
        return Ok(());
    }
    traverse_including_exc_handlers(cfg, &mut |cfg, region| {
        let changed = check_and_wrap(cfg, &mut try_blocks, region)?;
        Ok(changed && !try_blocks.is_empty())
    })
}

fn collect_try_catch_blocks(cfg: &MethodCfg) -> Vec<TryId> {
    let mut list = cfg.try_list.clone();
    // move parent try block to top
    java_sort(&mut list, |a, b| {
        if a == b {
            0
        } else if cfg.try_blocks[*a].outer == Some(*b) {
            1
        } else {
            -1
        }
    });
    list
}

fn check_and_wrap(cfg: &mut MethodCfg, try_blocks: &mut Vec<TryId>, region: RegionId) -> Res<bool> {
    // search top splitter block in this region (don't need to go deeper)
    for i in 0..try_blocks.len() {
        let tb = try_blocks[i];
        let top_splitter = cfg.try_blocks[tb].top_splitter;
        let Some(ts) = top_splitter else {
            continue;
        };
        if cfg
            .regions
            .sub_blocks(region)
            .contains(&Container::Block(ts))
        {
            if !wrap_blocks(cfg, Some(region), tb, ts) {
                cfg.warnings
                    .push(format!("Can't wrap try/catch for region: {region}"));
            }
            try_blocks.remove(i);
            return Ok(true);
        }
    }
    Ok(false)
}

/// Extract all block dominated by `dominator` to separate region and mark as try/catch block
fn wrap_blocks(
    cfg: &mut MethodCfg,
    replace_region: Option<RegionId>,
    tb: TryId,
    dominator: usize,
) -> bool {
    let Some(replace_region) = replace_region else {
        return false;
    };
    match &cfg.regions.arena[replace_region].kind {
        RegionKind::Loop { body, .. } => {
            let body = *body;
            return wrap_blocks(cfg, body, tb, dominator);
        }
        RegionKind::If { .. } | RegionKind::Switch { .. } | RegionKind::TryCatch { .. } => {
            let parent = cfg.regions.arena[replace_region].parent;
            return wrap_blocks(cfg, parent, tb, dominator);
        }
        _ => {}
    }
    let try_region = cfg.regions.new_plain(Some(replace_region));
    let sub_blocks = cfg.regions.sub_blocks(replace_region);
    // traverse the enclosing region for blocks that have a path from the dominator but don't have
    // a path from any of the exception handlers i.e. they are not before the end of the try block
    // so should be inside the try block.
    for cont in &sub_blocks {
        if has_path_through_block(cfg, dominator, *cont) {
            if is_handler_path(cfg, tb, *cont) {
                // this block/region has a path from an exception handler so is after the end of
                // the try block
                continue;
            }
            cfg.regions.plain_add(try_region, *cont);
        }
    }
    let try_subs = cfg.regions.sub_blocks(try_region);
    if try_subs.is_empty() {
        return false;
    }
    let try_catch_region = cfg.regions.new_region(
        Some(replace_region),
        RegionKind::TryCatch {
            try_r: Container::Region(try_region),
            catches: Vec::new(),
            finally_r: None,
            try_block: Some(tb),
        },
    );
    cfg.regions.arena[try_region].parent = Some(try_catch_region);
    // setTryCatchBlock
    let mut catches = Vec::new();
    let mut finally_r = None;
    for &h in &cfg.try_blocks[tb].handlers {
        if let Some(hr) = cfg.regions.handler_regions.get(&h).copied() {
            if cfg.handlers[h].is_finally {
                finally_r = Some(hr);
            } else {
                catches.push((h, hr));
            }
        }
    }
    if let RegionKind::TryCatch {
        catches: c,
        finally_r: f,
        ..
    } = &mut cfg.regions.arena[try_catch_region].kind
    {
        *c = catches;
        *f = finally_r;
    }
    // replace first node by region
    let first_node = try_subs[0];
    if !cfg.regions.replace_sub_block(
        replace_region,
        first_node,
        Container::Region(try_catch_region),
    ) {
        return false;
    }
    if let RegionKind::Plain { blocks } = &mut cfg.regions.arena[replace_region].kind {
        blocks.retain(|b| !try_subs.contains(b));
    }
    // fix parents for tryRegion sub blocks
    for cont in &try_subs {
        cfg.regions.set_parent(*cont, Some(try_region));
    }
    true
}

fn is_handler_path(cfg: &MethodCfg, tb: TryId, container: Container) -> bool {
    for &h in &cfg.try_blocks[tb].handlers {
        if let Some(hb) = cfg.handlers[h].handler_block {
            if !cfg.block(hb).contains(bflag::REMOVE)
                && is_path_exists_to_container(cfg, hb, container)
            {
                return true;
            }
        }
    }
    false
}
