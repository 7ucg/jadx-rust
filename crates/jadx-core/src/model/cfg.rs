//! Control flow graph: basic blocks, exception handlers and the per-method code state.

use super::bitset::{BitSet, JavaIntSet};
use super::info::ClassInfo;
use super::insn::{BlockId, HandlerId, InsnId, InsnNode, RegisterArg};
use super::ssa::{CodeVar, SsaId, SsaVar};

pub type LoopId = usize;
pub type TryId = usize;

/// `TryCatchBlockAttr`
#[derive(Debug, Clone)]
pub struct TryBlock {
    pub id: usize,
    pub handlers: Vec<HandlerId>,
    pub blocks: Vec<BlockId>,
    pub outer: Option<TryId>,
    pub inner: Vec<TryId>,
    pub merged: bool,
    pub top_splitter: Option<BlockId>,
}

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
    pub const TRY_ENTER: u64 = 1 << 14;
    pub const TRY_LEAVE: u64 = 1 << 15;

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
        (TRY_ENTER, "TRY_ENTER"),
        (TRY_LEAVE, "TRY_LEAVE"),
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
    pub try_block: Option<TryId>,
    /// Argument type of the caught exception (`getArgType`) applied to `move-exception`.
    pub arg_reg: Option<u32>,
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
            try_block: None,
            arg_reg: None,
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

    /// `getArgType`
    pub fn arg_type(&self) -> super::types::ArgType {
        if self.is_catch_all() {
            return super::types::ArgType::throwable();
        }
        if self.catch_types.len() == 1 {
            return self.catch_types[0].arg_type().clone();
        }
        super::types::ArgType::throwable()
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
    /// Instruction ids (indexes into `MethodCfg::insns`) in execution order.
    pub insns: Vec<InsnId>,
    pub preds: Vec<BlockId>,
    pub succs: Vec<BlockId>,
    /// `cleanSuccessors`: `None` = not computed, `Some(None)` = same list object as `succs`
    /// (the reference aliases the list when nothing is removed), `Some(Some(v))` = own copy.
    pub clean: Option<Option<Vec<BlockId>>>,
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
    /// `AType.EXC_CATCH` on the block (common catch attribute of its instructions).
    pub catch: Option<Vec<HandlerId>>,
    /// `AType.TRY_BLOCK`
    pub try_block: Option<TryId>,
    /// `AType.EXC_SPLIT_CROSS`: original path cross block.
    pub split_cross: Option<BlockId>,
    /// `AType.PHI_LIST`: phi instructions of this block (also present in `insns` until hidden).
    pub phi_list: Option<Vec<InsnId>>,
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
            clean: None,
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
            catch: None,
            try_block: None,
            split_cross: None,
            phi_list: None,
        }
    }
    /// Clean successors (`None` if never computed).
    pub fn clean_succs(&self) -> Option<&Vec<BlockId>> {
        match &self.clean {
            None => None,
            Some(None) => Some(&self.succs),
            Some(Some(v)) => Some(v),
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
            && self.catch.is_none()
            && self.try_block.is_none()
            && self.split_cross.is_none()
            && self.phi_list.is_none()
    }
}

/// Per-method code state: instructions, exception handlers and the block graph.
#[derive(Debug, Default)]
pub struct MethodCfg {
    /// Block arena indexed by `cid`; blocks are never removed from the arena.
    pub arena: Vec<BlockNode>,
    /// Instruction arena; blocks reference instructions by id.
    pub insns: Vec<InsnNode>,
    /// `mth.getBasicBlocks()`: live blocks in order.
    pub blocks: Vec<BlockId>,
    pub enter: BlockId,
    pub exit: BlockId,
    /// Handler arena (never shrinks); `handler_list` is the method's live list.
    pub handlers: Vec<ExceptionHandler>,
    pub handler_list: Vec<HandlerId>,
    /// `AFlag.COMPUTE_POST_DOM`
    pub compute_post_dom: bool,
    /// `CodeFeature.SWITCH`
    pub has_switch: bool,
    /// Method returns `void` (original, erased signature).
    pub void_return: bool,
    pub loop_infos: Vec<LoopInfo>,
    /// Registered loops of the method (`mth.getLoops()`).
    pub loops: Vec<LoopId>,
    /// Arena of try/catch blocks; `try_list` holds the final list (`AType.TRY_BLOCKS_LIST`).
    pub try_blocks: Vec<TryBlock>,
    pub try_list: Vec<TryId>,
    /// Register count of the method code (`getRegsCount`).
    pub regs_count: usize,
    /// `getThisArg`
    pub this_arg: Option<RegisterArg>,
    /// `getArgRegs`
    pub arg_regs: Vec<RegisterArg>,
    /// SSA variable arena; `svar_list` is `mth.getSVars()` (live variables in creation order).
    pub svars: Vec<SsaVar>,
    pub svar_list: Vec<SsaId>,
    /// Register arguments not placed in an instruction (duplicates under construction).
    pub floating: Vec<RegisterArg>,
    /// Code variable arena (`InitCodeVariables`).
    pub code_vars: Vec<CodeVar>,
    /// Warning comments attached to the method (`addWarnComment`).
    pub warnings: Vec<String>,
    /// Errors attached to the method (`addError`).
    pub errors: Vec<String>,
    /// Code size in 16-bit units (`getInsnsCount`).
    pub units_count: usize,
    /// Type inference ran (types are meaningful).
    pub types_done: bool,
}

impl MethodCfg {
    pub fn block(&self, id: BlockId) -> &BlockNode {
        &self.arena[id]
    }
    pub fn block_mut(&mut self, id: BlockId) -> &mut BlockNode {
        &mut self.arena[id]
    }

    pub fn insn(&self, id: InsnId) -> &InsnNode {
        &self.insns[id]
    }

    pub fn insn_mut(&mut self, id: InsnId) -> &mut InsnNode {
        &mut self.insns[id]
    }

    /// Add an instruction to the arena (not yet part of any block).
    pub fn add_insn(&mut self, insn: InsnNode) -> InsnId {
        self.insns.push(insn);
        self.insns.len() - 1
    }

    /// Append a copy of `insn` to the arena and to the end of `block`.
    pub fn push_insn(&mut self, block: BlockId, insn: InsnNode) -> InsnId {
        let id = self.add_insn(insn);
        self.arena[block].insns.push(id);
        id
    }

    pub fn last_insn(&self, block: BlockId) -> Option<&InsnNode> {
        self.arena[block].insns.last().map(|i| &self.insns[*i])
    }

    pub fn first_insn(&self, block: BlockId) -> Option<&InsnNode> {
        self.arena[block].insns.first().map(|i| &self.insns[*i])
    }

    pub fn last_insn_mut(&mut self, block: BlockId) -> Option<&mut InsnNode> {
        let id = *self.arena[block].insns.last()?;
        Some(&mut self.insns[id])
    }

    /// Clones of the block's instructions as new arena entries (ids), for block copying.
    pub fn clone_block_insns(&mut self, block: BlockId, flags: u64) -> Vec<InsnId> {
        let ids = self.arena[block].insns.clone();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let mut c = self.insns[id].clone();
            c.flags |= flags;
            out.push(self.add_insn(c));
        }
        out
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
        self.arena[id].clean = Some(if to_remove.is_empty() {
            None
        } else {
            Some(result)
        });
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
