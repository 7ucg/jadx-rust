//! Control flow graph: basic blocks, exception handlers and the per-method code state.

use super::bitset::{BitSet, JavaIntSet};
use super::info::ClassInfo;
use super::insn::{BlockId, HandlerId, InsnNode};

pub type LoopId = usize;

/// `LoopInfo`
#[derive(Debug, Clone)]
pub struct LoopInfo {
    pub start: BlockId,
    pub end: BlockId,
    pub blocks: JavaIntSet,
    pub id: usize,
    pub parent: Option<LoopId>,
}

/// Block flags (subset of the reference `AFlag`).
pub mod bflag {
    pub const MTH_ENTER_BLOCK: u64 = 1 << 0;
    pub const MTH_EXIT_BLOCK: u64 = 1 << 1;
    pub const SYNTHETIC: u64 = 1 << 2;
    pub const RETURN: u64 = 1 << 3;
    pub const REMOVE: u64 = 1 << 4;
    pub const LOOP_START: u64 = 1 << 5;
    pub const LOOP_END: u64 = 1 << 6;
    pub const EXC_TOP_SPLITTER: u64 = 1 << 7;
    pub const EXC_BOTTOM_SPLITTER: u64 = 1 << 8;
    pub const ADDED_TO_REGION: u64 = 1 << 9;
    pub const DONT_GENERATE: u64 = 1 << 10;
    pub const FALL_THROUGH: u64 = 1 << 11;
    pub const ORIG_RETURN: u64 = 1 << 12;
    pub const IGNORE_THROW_SPLIT: u64 = 1 << 13;

    pub const NAMES: &[(u64, &str)] = &[
        (MTH_ENTER_BLOCK, "MTH_ENTER_BLOCK"),
        (MTH_EXIT_BLOCK, "MTH_EXIT_BLOCK"),
        (SYNTHETIC, "SYNTHETIC"),
        (RETURN, "RETURN"),
        (REMOVE, "REMOVE"),
        (LOOP_START, "LOOP_START"),
        (LOOP_END, "LOOP_END"),
        (EXC_TOP_SPLITTER, "EXC_TOP_SPLITTER"),
        (EXC_BOTTOM_SPLITTER, "EXC_BOTTOM_SPLITTER"),
        (ADDED_TO_REGION, "ADDED_TO_REGION"),
        (DONT_GENERATE, "DONT_GENERATE"),
        (FALL_THROUGH, "FALL_THROUGH"),
        (ORIG_RETURN, "ORIG_RETURN"),
        (IGNORE_THROW_SPLIT, "IGNORE_THROW_SPLIT"),
    ];
}

#[derive(Debug, Clone)]
pub struct ExceptionHandler {
    pub catch_types: Vec<ClassInfo>,
    pub handler_offset: i32,
    pub handler_block: Option<BlockId>,
    pub blocks: Vec<BlockId>,
    pub is_finally: bool,
    pub removed: bool,
}

impl ExceptionHandler {
    pub fn build(addr: i32, ty: Option<ClassInfo>) -> Self {
        let mut eh = ExceptionHandler {
            catch_types: Vec::new(),
            handler_offset: addr,
            handler_block: None,
            blocks: Vec::new(),
            is_finally: false,
            removed: false,
        };
        eh.add_catch_type(ty);
        eh
    }

    /// Returns `true` when the set of catch types changed.
    pub fn add_catch_type(&mut self, ty: Option<ClassInfo>) -> bool {
        match ty {
            Some(t) => {
                if self.catch_types.contains(&t) {
                    return false;
                }
                self.catch_types.push(t);
                true
            }
            None => {
                if !self.catch_types.is_empty() {
                    self.catch_types.clear();
                    return true;
                }
                false
            }
        }
    }

    pub fn is_catch_all(&self) -> bool {
        self.catch_types.is_empty()
            || self
                .catch_types
                .iter()
                .any(|c| c.full_name() == "java.lang.Throwable")
    }
}

#[derive(Debug, Clone)]
pub struct BlockNode {
    pub cid: BlockId,
    pub pos: usize,
    pub start_offset: i32,
    pub insns: Vec<InsnNode>,
    pub preds: Vec<BlockId>,
    pub succs: Vec<BlockId>,
    pub clean_succs: Option<Vec<BlockId>>,
    pub flags: u64,
    /// `AType.EXC_HANDLER`
    pub exc_handler: Option<HandlerId>,
    /// `AType.TMP_EDGE`
    pub tmp_edge: Option<BlockId>,
    /// Dominators by block position (`None` = not computed).
    pub doms: Option<BitSet>,
    pub post_doms: Option<BitSet>,
    pub dom_frontier: Option<BitSet>,
    pub idom: Option<BlockId>,
    pub ipost_dom: Option<BlockId>,
    pub dominates_on: Vec<BlockId>,
    /// `AType.LOOP` attribute list (loops this block starts or ends).
    pub loops: Vec<LoopId>,
}

impl BlockNode {
    pub fn new(cid: BlockId, pos: usize, start_offset: i32) -> Self {
        BlockNode {
            cid,
            pos,
            start_offset,
            insns: Vec::with_capacity(2),
            preds: Vec::new(),
            succs: Vec::new(),
            clean_succs: None,
            flags: 0,
            exc_handler: None,
            tmp_edge: None,
            doms: Some(BitSet::new()),
            post_doms: Some(BitSet::new()),
            dom_frontier: None,
            idom: None,
            ipost_dom: None,
            dominates_on: Vec::new(),
            loops: Vec::new(),
        }
    }
    pub fn contains(&self, flag: u64) -> bool {
        self.flags & flag != 0
    }
    pub fn add(&mut self, flag: u64) {
        self.flags |= flag;
    }
    /// `isAttrStorageEmpty`
    pub fn attrs_empty(&self) -> bool {
        self.flags == 0
            && self.exc_handler.is_none()
            && self.tmp_edge.is_none()
            && self.loops.is_empty()
    }
}

/// Per-method code state: instructions, exception handlers and the block graph.
#[derive(Debug, Default)]
pub struct MethodCfg {
    /// Block arena indexed by `cid`; blocks are never removed from the arena.
    pub arena: Vec<BlockNode>,
    /// `mth.getBasicBlocks()`: live blocks in order.
    pub blocks: Vec<BlockId>,
    pub enter: BlockId,
    pub exit: BlockId,
    pub handlers: Vec<ExceptionHandler>,
    /// `AFlag.COMPUTE_POST_DOM`
    pub compute_post_dom: bool,
    /// `CodeFeature.SWITCH`
    pub has_switch: bool,
    /// Method returns `void` (original, erased signature).
    pub void_return: bool,
    pub loop_infos: Vec<LoopInfo>,
    /// Registered loops of the method (`mth.getLoops()`).
    pub loops: Vec<LoopId>,
}

impl MethodCfg {
    pub fn block(&self, id: BlockId) -> &BlockNode {
        &self.arena[id]
    }
    pub fn block_mut(&mut self, id: BlockId) -> &mut BlockNode {
        &mut self.arena[id]
    }

    /// `startNewBlock`: new block appended to the live list.
    pub fn start_new_block(&mut self, offset: i32) -> BlockId {
        let cid = self.arena.len();
        let pos = self.blocks.len();
        self.arena.push(BlockNode::new(cid, pos, offset));
        self.blocks.push(cid);
        cid
    }

    pub fn connect(&mut self, from: BlockId, to: BlockId) {
        if !self.arena[from].succs.contains(&to) {
            self.arena[from].succs.push(to);
        }
        if !self.arena[to].preds.contains(&from) {
            self.arena[to].preds.push(from);
        }
    }

    pub fn remove_connection(&mut self, from: BlockId, to: BlockId) {
        self.arena[from].succs.retain(|b| *b != to);
        self.arena[to].preds.retain(|b| *b != from);
    }

    pub fn update_block_positions(&mut self) {
        for (i, id) in self.blocks.clone().into_iter().enumerate() {
            self.arena[id].pos = i;
        }
    }

    /// `BlockNode.cleanSuccessors`: successors without exception handler paths and loop back edges.
    pub fn update_clean_successors(&mut self, id: BlockId) {
        let succs = self.arena[id].succs.clone();
        let mut to_remove: Vec<BlockId> = Vec::new();
        for b in &succs {
            if self.is_exception_handler_path(*b) {
                to_remove.push(*b);
            }
        }
        if self.arena[id].contains(bflag::LOOP_END) {
            for l in &self.arena[id].loops {
                to_remove.push(self.loop_infos[*l].start);
            }
        }
        let result: Vec<BlockId> = succs
            .into_iter()
            .filter(|b| !to_remove.contains(b))
            .collect();
        self.arena[id].clean_succs = Some(result);
    }

    pub fn is_exception_handler_path(&self, b: BlockId) -> bool {
        let blk = &self.arena[b];
        if blk.exc_handler.is_some()
            || blk.contains(bflag::EXC_BOTTOM_SPLITTER)
            || blk.contains(bflag::REMOVE)
        {
            return true;
        }
        if blk.contains(bflag::SYNTHETIC) {
            return blk.succs.len() == 1 && self.arena[blk.succs[0]].exc_handler.is_some();
        }
        false
    }
}
