//! `SwitchBreakVisitor`: optimize `break` instructions of switch cases.

use std::collections::HashSet;

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::InsnType;
use crate::model::region::{Container, RegionId, RegionKind};
use crate::passes::constructor::MethodEnv;
use crate::passes::regions::if_visitor::if_region_visitor;
use crate::passes::regions::maker::Res;
use crate::passes::regions::switch_maker::build_break_container;
use crate::passes::regions::traversal::{traverse, traverse_method, RegionVisitor};
use crate::passes::regions::utils::{
    cflag, cflag_add, container_insns, container_last_insn, contains_exit_insn, get_container_of,
    get_last_block, get_last_insn_with_block, is_block_like,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    ExtractCommonBreak,
    RemoveUnreachableBreak,
}

struct SwitchVisitor {
    kind: Kind,
    current_switch: RegionId,
    add_break_region: HashSet<RegionId>,
    cleanup_set: HashSet<Container>,
    re_run_requested: bool,
}

impl SwitchVisitor {
    fn new(kind: Kind, current_switch: RegionId) -> Self {
        SwitchVisitor {
            kind,
            current_switch,
            add_break_region: HashSet::new(),
            cleanup_set: HashSet::new(),
            re_run_requested: false,
        }
    }

    fn reset(&mut self) {
        self.re_run_requested = false;
        self.add_break_region.clear();
        self.cleanup_set.clear();
    }

    fn is_break_block(&self, cfg: &MethodCfg, block: Option<Container>) -> bool {
        let Some(block) = block else {
            return false;
        };
        if let Some(last) = container_last_insn(cfg, block) {
            if cfg.insn(last).ty == InsnType::Break {
                return cfg.regions.insn_region_ref.get(&last) == Some(&self.current_switch);
            }
        }
        false
    }

    fn remove_break(&mut self, cfg: &mut MethodCfg, break_block: Container, parent: Container) {
        let Some(last) = container_last_insn(cfg, break_block) else {
            return;
        };
        if cfg.insn(last).ty != InsnType::Break {
            return;
        }
        let now_empty = match break_block {
            Container::Block(b) => {
                cfg.block_mut(b).insns.pop();
                cfg.block(b).insns.is_empty()
            }
            Container::Insns(i) => {
                cfg.regions.insn_conts[i].insns.pop();
                cfg.regions.insn_conts[i].insns.is_empty()
            }
            Container::Region(_) => false,
        };
        if now_empty {
            cflag_add(cfg, break_block, bflag::REMOVE);
            self.cleanup_set.insert(parent);
        }
    }

    fn process_region(&mut self, cfg: &mut MethodCfg, region: RegionId) {
        match self.kind {
            Kind::ExtractCommonBreak => {
                if matches!(
                    cfg.regions.arena[region].kind,
                    RegionKind::If { .. } | RegionKind::TryCatch { .. }
                ) {
                    // if break in all branches extract to parent region
                    self.process_branch_region(cfg, region);
                }
            }
            Kind::RemoveUnreachableBreak => {
                let subs = cfg.regions.sub_blocks(region);
                if let Some(&last) = subs.last() {
                    if is_block_like(last)
                        && self.is_break_block(cfg, Some(last))
                        && is_prev_insn_is_exit(cfg, last, &subs)
                    {
                        self.remove_break(cfg, last, Container::Region(region));
                    }
                }
            }
        }
    }

    fn process_branch_region(&mut self, cfg: &mut MethodCfg, region: RegionId) {
        let Some(parent_region) = cfg.regions.arena[region].parent else {
            return;
        };
        let parent_c = Container::Region(parent_region);
        if cflag(cfg, parent_c, bflag::FALL_THROUGH) {
            // fallthrough case, can't extract break
            return;
        }
        let mut dont_add_common_break = false;
        let last_parent_block = get_last_block(cfg, parent_c);
        if let Some(lpb) = last_parent_block {
            if contains_exit_insn(cfg, lpb) {
                if self.is_break_block(cfg, Some(lpb)) {
                    // parent block already contains 'break'
                    dont_add_common_break = true;
                } else {
                    // can't add 'break' after 'return', 'throw' or 'continue'
                    return;
                }
            }
        }
        let branches = cfg.regions.branches(region);
        // all branches contain exit insns, common break is unreachable
        let mut remove_common_break = true;
        let mut for_break_remove: Vec<(Option<Container>, Container)> = Vec::new();
        for branch in branches {
            let Some(branch) = branch else {
                remove_common_break = false;
                continue;
            };
            let Some((block, last_insn)) = get_last_insn_with_block(cfg, branch) else {
                return;
            };
            let ty = cfg.insn(last_insn).ty;
            if ty == InsnType::Break {
                let parent = get_container_of(cfg, branch, block);
                for_break_remove.push((parent, block));
                remove_common_break = false;
            } else if !matches!(ty, InsnType::Return | InsnType::Throw | InsnType::Continue) {
                remove_common_break = false;
            }
        }
        if !for_break_remove.is_empty() {
            // common 'break' confirmed
            for (parent, block) in for_break_remove {
                if let Some(parent) = parent {
                    self.remove_break(cfg, block, parent);
                }
            }
            if !dont_add_common_break {
                self.add_break_region.insert(parent_region);
                // new 'break' might become 'common' for upper branch region, request to run checks
                // again
                self.re_run_requested = true;
            }
            // removed 'break' may allow to use 'else-if' chain
            cfg.regions.request_if_region_optimize = true;
        }
        if remove_common_break {
            if let Some(lpb) = last_parent_block {
                self.remove_break(cfg, lpb, parent_c);
            }
        }
    }
}

fn is_prev_insn_is_exit(cfg: &MethodCfg, break_block: Container, subs: &[Container]) -> bool {
    let insns = container_insns(cfg, break_block);
    let prev = if insns.len() > 1 {
        // check prev insn in same block
        Some(insns[insns.len() - 2])
    } else if subs.len() > 1 {
        let prev = subs[subs.len() - 2];
        if is_block_like(prev) {
            container_insns(cfg, prev).last().copied()
        } else {
            None
        }
    } else {
        None
    };
    prev.is_some_and(|i| {
        matches!(
            cfg.insn(i).ty,
            InsnType::Return | InsnType::Throw | InsnType::Continue | InsnType::Break
        )
    })
}

impl RegionVisitor for SwitchVisitor {
    fn enter_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
        self.process_region(cfg, region);
        Ok(true)
    }

    fn leave_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<()> {
        if self.add_break_region.remove(&region) {
            let c = build_break_container(cfg, self.current_switch);
            if matches!(
                cfg.regions.arena[region].kind,
                RegionKind::Plain { .. } | RegionKind::Sync { .. }
            ) {
                cfg.regions.plain_blocks_mut(region).push(c);
            }
        }
        if self.cleanup_set.remove(&Container::Region(region)) {
            if let RegionKind::Plain { blocks } = &cfg.regions.arena[region].kind {
                let keep: Vec<Container> = blocks
                    .iter()
                    .copied()
                    .filter(|c| !cflag(cfg, *c, bflag::REMOVE))
                    .collect();
                if let RegionKind::Plain { blocks } = &mut cfg.regions.arena[region].kind {
                    *blocks = keep;
                }
            }
        }
        Ok(())
    }
}

/// `IterativeSwitchRegionVisitor`
struct IterativeSwitch {
    kind: Kind,
}

impl RegionVisitor for IterativeSwitch {
    fn leave_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<()> {
        if !matches!(cfg.regions.arena[region].kind, RegionKind::Switch { .. }) {
            return Ok(());
        }
        let mut visitor = SwitchVisitor::new(self.kind, region);
        let mut k = 0;
        loop {
            let mut run_again = false;
            traverse(cfg, Container::Region(region), &mut visitor)?;
            if visitor.re_run_requested {
                visitor.reset();
                run_again = true;
            }
            let over = k > 20;
            k += 1;
            if over {
                // 20 nested 'if' are not expected
                cfg.warnings.push(
                    "Unexpected iteration count in SwitchBreakVisitor. Please report as an issue"
                        .to_string(),
                );
                break;
            }
            if !run_again {
                break;
            }
        }
        Ok(())
    }
}

/// `SwitchBreakVisitor.visit`
pub fn switch_break_visitor(cfg: &mut MethodCfg, env: &MethodEnv) -> Res<()> {
    if cfg.has_switch {
        traverse_method(
            cfg,
            &mut IterativeSwitch {
                kind: Kind::ExtractCommonBreak,
            },
        )?;
        traverse_method(
            cfg,
            &mut IterativeSwitch {
                kind: Kind::RemoveUnreachableBreak,
            },
        )?;
        process_if_requested(cfg, env)?;
    }
    Ok(())
}

/// `IfRegionVisitor.processIfRequested`
pub fn process_if_requested(cfg: &mut MethodCfg, env: &MethodEnv) -> Res<()> {
    if cfg.regions.request_if_region_optimize {
        let r = if_region_visitor(cfg, env);
        cfg.regions.request_if_region_optimize = false;
        return r;
    }
    Ok(())
}
