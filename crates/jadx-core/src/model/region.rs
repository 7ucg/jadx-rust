//! Region tree: structured containers built from the block graph.
//!
//! Regions live in an arena owned by the method ([`Regions`]); containers reference blocks and
//! instructions by id, so the tree needs no shared ownership. Parent links are kept as ids.

use std::collections::HashMap;

use super::cfg::{LoopId, TryId};
use super::condition::IfCondition;
use super::insn::{BlockId, HandlerId, InsnId};

pub type RegionId = usize;
pub type InsnContId = usize;

/// `IContainer`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    /// A basic block (`BlockNode`).
    Block(BlockId),
    /// A list of instructions outside of the block graph (`InsnContainer`).
    Insns(InsnContId),
    Region(RegionId),
}

/// Key of a `switch` case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseKey {
    Int(i32),
    Str(String),
    Default,
}

impl CaseKey {
    pub fn text(&self) -> String {
        match self {
            CaseKey::Int(i) => i.to_string(),
            CaseKey::Str(s) => s.clone(),
            CaseKey::Default => "default".to_string(),
        }
    }
}

/// `SwitchRegion.CaseInfo`
#[derive(Debug, Clone)]
pub struct CaseInfo {
    pub keys: Vec<CaseKey>,
    pub container: Container,
}

impl CaseInfo {
    pub fn is_default_case(&self) -> bool {
        self.keys.len() == 1 && self.keys[0] == CaseKey::Default
    }
}

/// `LoopType`
#[derive(Debug, Clone, PartialEq)]
pub enum LoopType {
    /// `for (init; cond; incr)`
    For { init: InsnId, incr: InsnId },
    /// `for (var : iterable)`: `var_arg_insn` holds the loop variable as result, `iterable_insn`
    /// the iterated value as its argument (`REGION_ARG` instructions).
    ForEach {
        var_arg_insn: InsnId,
        iterable_insn: InsnId,
    },
}

#[derive(Debug, Clone)]
pub enum RegionKind {
    /// `Region`: a plain sequence of containers.
    Plain { blocks: Vec<Container> },
    /// `IfRegion`
    If {
        cond: Option<IfCondition>,
        cond_blocks: Vec<BlockId>,
        then_r: Option<Container>,
        else_r: Option<Container>,
    },
    /// `LoopRegion`
    Loop {
        info: LoopId,
        cond: Option<IfCondition>,
        cond_blocks: Vec<BlockId>,
        header: Option<BlockId>,
        condition_at_end: bool,
        pre_condition: Option<BlockId>,
        body: Option<RegionId>,
        ty: Option<LoopType>,
    },
    /// `SwitchRegion`
    Switch {
        header: BlockId,
        cases: Vec<CaseInfo>,
    },
    /// `SynchronizedRegion`: `inner` is a plain region holding the sub blocks.
    Sync {
        enter_insn: InsnId,
        exit_insns: Vec<InsnId>,
        inner: RegionId,
    },
    /// `TryCatchRegion`
    TryCatch {
        try_r: Container,
        catches: Vec<(HandlerId, Container)>,
        finally_r: Option<Container>,
        try_block: Option<TryId>,
    },
}

#[derive(Debug, Clone)]
pub struct Region {
    pub parent: Option<RegionId>,
    /// Flags (`bflag` bits: `DONT_GENERATE`, `FALL_THROUGH`, `ELSE_IF_CHAIN`...).
    pub flags: u64,
    /// `AType.EXC_HANDLER` attribute of a handler region.
    pub exc_handler: Option<HandlerId>,
    pub kind: RegionKind,
}

/// `InsnContainer`
#[derive(Debug, Clone, Default)]
pub struct InsnContainer {
    pub insns: Vec<InsnId>,
    pub flags: u64,
}

/// `EdgeInsnAttr`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeInsn {
    pub start: BlockId,
    pub end: BlockId,
    pub insn: InsnId,
}

/// All region related state of a method.
#[derive(Debug, Clone, Default)]
pub struct Regions {
    pub arena: Vec<Region>,
    pub insn_conts: Vec<InsnContainer>,
    /// `mth.getRegion()`
    pub method_region: Option<RegionId>,
    /// `ExceptionHandler.getHandlerRegion()`, by handler id.
    pub handler_regions: HashMap<HandlerId, Container>,
    /// `AType.EDGE_INSN` per block.
    pub edge_insns: HashMap<BlockId, Vec<EdgeInsn>>,
    /// `AType.LOOP` on `break` instructions.
    pub insn_loops: HashMap<InsnId, Vec<LoopId>>,
    /// `AType.LOOP_LABEL` on `break` instructions and loop start blocks.
    pub insn_loop_label: HashMap<InsnId, LoopId>,
    pub block_loop_label: HashMap<BlockId, LoopId>,
    /// `RegionRefAttr` on switch and `break` instructions.
    pub insn_region_ref: HashMap<InsnId, RegionId>,
}

impl Regions {
    pub fn new_region(&mut self, parent: Option<RegionId>, kind: RegionKind) -> RegionId {
        self.arena.push(Region {
            parent,
            flags: 0,
            exc_handler: None,
            kind,
        });
        self.arena.len() - 1
    }

    /// `new Region(parent)`
    pub fn new_plain(&mut self, parent: Option<RegionId>) -> RegionId {
        self.new_region(parent, RegionKind::Plain { blocks: Vec::new() })
    }

    pub fn new_insn_container(&mut self, insns: Vec<InsnId>) -> Container {
        self.insn_conts.push(InsnContainer { insns, flags: 0 });
        Container::Insns(self.insn_conts.len() - 1)
    }

    pub fn region(&self, id: RegionId) -> &Region {
        &self.arena[id]
    }

    pub fn region_mut(&mut self, id: RegionId) -> &mut Region {
        &mut self.arena[id]
    }

    pub fn set_parent(&mut self, c: Container, parent: Option<RegionId>) {
        if let Container::Region(r) = c {
            self.arena[r].parent = parent;
        }
    }

    /// `Region.add`: append and update the parent of a region child.
    pub fn plain_add(&mut self, region: RegionId, c: Container) {
        self.set_parent(c, Some(region));
        self.plain_blocks_mut(region).push(c);
    }

    /// Mutable sub block list of a plain region (or the inner region of a synchronized one).
    pub fn plain_blocks_mut(&mut self, region: RegionId) -> &mut Vec<Container> {
        let target = match &self.arena[region].kind {
            RegionKind::Sync { inner, .. } => *inner,
            _ => region,
        };
        match &mut self.arena[target].kind {
            RegionKind::Plain { blocks } => blocks,
            other => panic!("sub block list of a non plain region: {other:?}"),
        }
    }

    /// `getSubBlocks` (a copy).
    pub fn sub_blocks(&self, region: RegionId) -> Vec<Container> {
        match &self.arena[region].kind {
            RegionKind::Plain { blocks } => blocks.clone(),
            RegionKind::If {
                cond_blocks,
                then_r,
                else_r,
                ..
            } => {
                let mut all: Vec<Container> =
                    cond_blocks.iter().map(|b| Container::Block(*b)).collect();
                all.extend(then_r.iter().copied());
                all.extend(else_r.iter().copied());
                all
            }
            RegionKind::Loop {
                pre_condition,
                cond_blocks,
                body,
                ..
            } => {
                let mut all = Vec::new();
                if let Some(p) = pre_condition {
                    all.push(Container::Block(*p));
                }
                all.extend(cond_blocks.iter().map(|b| Container::Block(*b)));
                if let Some(b) = body {
                    all.push(Container::Region(*b));
                }
                all
            }
            RegionKind::Switch { header, cases } => {
                let mut all = vec![Container::Block(*header)];
                all.extend(cases.iter().map(|c| c.container));
                all
            }
            RegionKind::Sync { inner, .. } => self.sub_blocks(*inner),
            RegionKind::TryCatch {
                try_r,
                catches,
                finally_r,
                ..
            } => {
                let mut all = vec![*try_r];
                all.extend(catches.iter().map(|(_, c)| *c));
                all.extend(finally_r.iter().copied());
                all
            }
        }
    }

    /// `IBranchRegion`: if, switch and try/catch regions.
    pub fn is_branch_region(&self, region: RegionId) -> bool {
        matches!(
            self.arena[region].kind,
            RegionKind::If { .. } | RegionKind::Switch { .. } | RegionKind::TryCatch { .. }
        )
    }

    /// `getBranches`: `None` entries are missing branches.
    pub fn branches(&self, region: RegionId) -> Vec<Option<Container>> {
        match &self.arena[region].kind {
            RegionKind::If { then_r, else_r, .. } => vec![*then_r, *else_r],
            RegionKind::Switch { cases, .. } => cases.iter().map(|c| Some(c.container)).collect(),
            RegionKind::TryCatch { .. } => self.sub_blocks(region).into_iter().map(Some).collect(),
            _ => Vec::new(),
        }
    }

    /// `replaceSubBlock`
    pub fn replace_sub_block(&mut self, region: RegionId, old: Container, new: Container) -> bool {
        match &mut self.arena[region].kind {
            RegionKind::Plain { blocks } => {
                if let Some(i) = blocks.iter().position(|b| *b == old) {
                    blocks[i] = new;
                    self.set_parent(new, Some(region));
                    true
                } else {
                    false
                }
            }
            RegionKind::If { then_r, else_r, .. } => {
                if *then_r == Some(old) {
                    *then_r = Some(new);
                    self.set_parent(new, Some(region));
                    true
                } else if *else_r == Some(old) {
                    *else_r = Some(new);
                    self.set_parent(new, Some(region));
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub fn edge_insns_of(&self, block: BlockId) -> &[EdgeInsn] {
        self.edge_insns.get(&block).map_or(&[], |v| v.as_slice())
    }
}
