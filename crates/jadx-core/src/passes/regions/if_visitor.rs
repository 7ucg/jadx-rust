//! `IfRegionVisitor` and `ReturnVisitor`.

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::condition::{CondMode, IfCondition};
use crate::model::insn::InsnType;
use crate::model::region::{Container, RegionId, RegionKind};
use crate::passes::constructor::MethodEnv;
use crate::passes::regions::maker::Res;
use crate::passes::regions::ternary_mod::ternary_mod;
use crate::passes::regions::traversal::{traverse_iterative, traverse_method, RegionVisitor};
use crate::passes::regions::utils::{
    cflag, cflag_add, container_insns, get_last_insn, has_break_insn, has_exit_block,
    has_exit_edge, insns_count, is_empty, is_exit_container,
};

/// `IfRegionVisitor.process`
pub fn if_region_visitor(cfg: &mut MethodCfg, env: &MethodEnv) -> Res<()> {
    if cfg.blocks.is_empty() {
        return Ok(());
    }
    ternary_mod(cfg, env)?;
    traverse_method(cfg, &mut ProcessIfRegionVisitor)?;
    traverse_iterative(cfg, &mut |cfg, region| {
        if matches!(cfg.regions.arena[region].kind, RegionKind::If { .. }) {
            return remove_redundant_else_block(cfg, region);
        }
        Ok(false)
    })
}

struct ProcessIfRegionVisitor;

impl RegionVisitor for ProcessIfRegionVisitor {
    fn enter_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
        if matches!(cfg.regions.arena[region].kind, RegionKind::If { .. }) {
            order_branches(cfg, region);
            mark_else_if_chains(cfg, region);
        }
        Ok(true)
    }
}

fn branches(cfg: &MethodCfg, region: RegionId) -> (Option<Container>, Option<Container>) {
    match &cfg.regions.arena[region].kind {
        RegionKind::If { then_r, else_r, .. } => (*then_r, *else_r),
        _ => (None, None),
    }
}

/// `IfRegion.invert` when the else region exists.
fn invert_if_region(cfg: &mut MethodCfg, region: RegionId) {
    let (then_r, else_r) = branches(cfg, region);
    if else_r.is_none() {
        return;
    }
    let cond = match &cfg.regions.arena[region].kind {
        RegionKind::If { cond, .. } => cond.clone(),
        _ => return,
    };
    let new_cond = cond.map(|c| IfCondition::invert(cfg, &c));
    if let RegionKind::If {
        cond,
        then_r: t,
        else_r: e,
        ..
    } = &mut cfg.regions.arena[region].kind
    {
        *cond = new_cond;
        *t = else_r;
        *e = then_r;
    }
}

/// `ConditionRegion.simplifyCondition`
fn simplify_condition(cfg: &mut MethodCfg, region: RegionId) -> bool {
    let cond = match &cfg.regions.arena[region].kind {
        RegionKind::If { cond: Some(c), .. } => c.clone(),
        _ => return false,
    };
    let (updated, changed) = IfCondition::simplify(cfg, &cond);
    if changed {
        if let RegionKind::If { cond, .. } = &mut cfg.regions.arena[region].kind {
            *cond = Some(updated);
        }
        return true;
    }
    false
}

fn is_simple_exit_block(cfg: &MethodCfg, c: Option<Container>) -> bool {
    let Some(c) = c else {
        return false;
    };
    if cflag(cfg, c, bflag::RETURN) || is_exit_container(cfg, c) {
        return true;
    }
    if let Container::Region(r) = c {
        let subs = cfg.regions.sub_blocks(r);
        return subs.len() == 1 && is_exit_container(cfg, subs[0]);
    }
    false
}

fn is_if_region(cfg: &MethodCfg, c: Option<Container>) -> bool {
    match c {
        Some(Container::Region(r)) => {
            if matches!(cfg.regions.arena[r].kind, RegionKind::If { .. }) {
                return true;
            }
            let subs = cfg.regions.sub_blocks(r);
            subs.len() == 1
                && matches!(
                    subs[0],
                    Container::Region(s) if matches!(cfg.regions.arena[s].kind, RegionKind::If { .. })
                )
        }
        _ => false,
    }
}

fn order_branches(cfg: &mut MethodCfg, region: RegionId) {
    let (then_r, else_r) = branches(cfg, region);
    if is_empty(cfg, else_r) {
        return;
    }
    if is_empty(cfg, then_r) {
        invert_if_region(cfg, region);
        return;
    }
    if simplify_condition(cfg, region) {
        let is_not = matches!(
            &cfg.regions.arena[region].kind,
            RegionKind::If { cond: Some(c), .. } if c.mode == CondMode::Not
        );
        if is_not {
            invert_if_region(cfg, region);
        }
    }
    let (then_r, else_r) = branches(cfg, region);
    let then_size = then_r.map_or(0, |c| insns_count(cfg, c));
    let else_size = else_r.map_or(0, |c| insns_count(cfg, c));
    if is_simple_exit_block(cfg, else_r) {
        if is_simple_exit_block(cfg, then_r) && else_size < then_size {
            invert_if_region(cfg, region);
            return;
        }
        if else_size == 1 {
            let last_region = has_exit_edge(cfg, Container::Region(region));
            if last_region && cfg.void_return {
                let last_else_insn = else_r.and_then(|c| get_last_insn(cfg, c));
                if last_else_insn.is_some_and(|i| cfg.insn(i).ty == InsnType::Throw) {
                    // move `throw` into `then` block
                    invert_if_region(cfg, region);
                }
                // else: single return at method end will be removed later
                return;
            }
            if then_size > 2 && !(last_region && then_size < 4) {
                invert_if_region(cfg, region);
                return;
            }
        }
    }
    let then_exit = has_exit_block(cfg, then_r);
    let else_exit = has_exit_block(cfg, else_r);
    if else_exit && (!then_exit || else_size < then_size) {
        invert_if_region(cfg, region);
        return;
    }
    // move 'if' from 'then' branch to make 'else if' chain
    if is_if_region(cfg, then_r) && !is_if_region(cfg, else_r) && !then_exit {
        invert_if_region(cfg, region);
        return;
    }
    // move 'break' into 'then' branch
    if else_r.is_some_and(|c| has_break_insn(cfg, c)) {
        invert_if_region(cfg, region);
    }
}

/// Mark if-else-if chains
fn mark_else_if_chains(cfg: &mut MethodCfg, region: RegionId) {
    let (then_r, else_r) = branches(cfg, region);
    if is_simple_exit_block(cfg, then_r) {
        return;
    }
    if let Some(Container::Region(els)) = else_r {
        if let RegionKind::Plain { blocks } = &cfg.regions.arena[els].kind {
            if blocks.len() == 1 {
                if let Container::Region(sub) = blocks[0] {
                    if matches!(cfg.regions.arena[sub].kind, RegionKind::If { .. }) {
                        cflag_add(cfg, Container::Region(sub), bflag::ELSE_IF_CHAIN);
                        cflag_add(cfg, Container::Region(els), bflag::ELSE_IF_CHAIN);
                    }
                }
            }
        }
    }
}

fn remove_redundant_else_block(cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
    let (then_r, else_r) = branches(cfg, region);
    let Some(else_r) = else_r else {
        return Ok(false);
    };
    if !has_exit_block(cfg, then_r) {
        return Ok(false);
    }
    let last_then_insn = then_r.and_then(|c| get_last_insn(cfg, c));
    if last_then_insn.is_some_and(|i| cfg.insn(i).ty == InsnType::Throw) {
        // always omit else after 'throw'
    } else if cfg.void_return {
        // code style check:
        // will remove 'return;' from 'then' and 'else' with one instruction
        let then_size = then_r.map_or(0, |c| insns_count(cfg, c));
        // keep small blocks with same or 'similar' size unchanged
        if then_size < 5 {
            let else_size = insns_count(cfg, else_r);
            let range = if cflag(cfg, else_r, bflag::ELSE_IF_CHAIN) {
                4
            } else {
                2
            };
            if then_size == else_size
                || (then_size * range > else_size && then_size < else_size * range)
            {
                return Ok(false);
            }
        }
    }
    let parent = cfg.regions.arena[region]
        .parent
        .ok_or("NullPointerException: if region parent")?;
    let new_region = cfg.regions.new_plain(Some(parent));
    if cfg.regions.replace_sub_block(
        parent,
        Container::Region(region),
        Container::Region(new_region),
    ) {
        cfg.regions.plain_add(new_region, Container::Region(region));
        cfg.regions.plain_add(new_region, else_r);
        if let RegionKind::If { else_r, .. } = &mut cfg.regions.arena[region].kind {
            *else_r = None;
        }
        return Ok(true);
    }
    Ok(false)
}

// ---- ReturnVisitor ------------------------------------------------------------------------------

/// `ReturnVisitor`: remove unnecessary return instructions for void methods.
pub fn return_visitor(cfg: &mut MethodCfg) -> Res<()> {
    if cfg.void_return {
        traverse_method(cfg, &mut ReturnRemover { stack: Vec::new() })?;
    }
    Ok(())
}

struct ReturnRemover {
    stack: Vec<RegionId>,
}

impl RegionVisitor for ReturnRemover {
    fn enter_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
        self.stack.push(region);
        Ok(!matches!(
            cfg.regions.arena[region].kind,
            RegionKind::Switch { .. }
        ))
    }

    fn leave_region(&mut self, _cfg: &mut MethodCfg, _region: RegionId) -> Res<()> {
        self.stack.pop();
        Ok(())
    }

    fn process_block(&mut self, cfg: &mut MethodCfg, container: Container) -> Res<()> {
        let Container::Block(block) = container else {
            return Ok(());
        };
        if cfg.block(block).contains(bflag::RETURN)
            && cfg.block(block).insns.len() == 1
            && self.block_not_in_loop(cfg, block)
            && self.no_trail_instructions(cfg, block)
        {
            cfg.block_mut(block).insns.remove(0);
            cfg.block_mut(block).flags &= !bflag::RETURN;
        }
        Ok(())
    }
}

impl ReturnRemover {
    fn block_not_in_loop(&self, cfg: &MethodCfg, block: usize) -> bool {
        if cfg.loops.is_empty() {
            return true;
        }
        if cfg
            .loops
            .iter()
            .any(|l| cfg.loop_infos[*l].blocks.contains(block))
        {
            return false;
        }
        !self
            .stack
            .iter()
            .any(|r| matches!(cfg.regions.arena[*r].kind, RegionKind::Loop { .. }))
    }

    /// Check that there are no code after this block in regions structure
    fn no_trail_instructions(&self, cfg: &MethodCfg, block: usize) -> bool {
        let mut cur = Container::Block(block);
        for &region in self.stack.iter().rev() {
            // ignore paths on other branches
            if cfg.regions.is_branch_region(region) {
                cur = Container::Region(region);
                continue;
            }
            let subs = cfg.regions.sub_blocks(region);
            for sub in subs.iter().rev() {
                if *sub == cur {
                    break;
                } else if !is_container_empty(cfg, *sub) {
                    return false;
                }
            }
            cur = Container::Region(region);
        }
        true
    }
}

/// Check if container not contains instructions, don't count one 'return' instruction.
fn is_container_empty(cfg: &MethodCfg, c: Container) -> bool {
    match c {
        Container::Region(r) => cfg
            .regions
            .sub_blocks(r)
            .into_iter()
            .all(|s| is_container_empty(cfg, s)),
        _ => container_insns(cfg, c).is_empty() || cflag(cfg, c, bflag::RETURN),
    }
}
