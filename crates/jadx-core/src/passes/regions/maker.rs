//! `RegionMaker` and `RegionStack`: pack blocks into regions.

use std::collections::HashSet;

use crate::model::bitset::BitSet;
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{BlockId, InsnId, InsnNode, InsnType};
use crate::model::region::{Container, RegionId};
use crate::passes::constructor::MethodEnv;
use crate::passes::regions::utils::{last_insn, next_block};

pub type Res<T> = Result<T, String>;

const REGIONS_STACK_LIMIT: usize = 1000;

#[derive(Clone, Default)]
struct State {
    exits: HashSet<BlockId>,
    region: Option<RegionId>,
}

/// `RegionStack`
#[derive(Default)]
pub struct RegionStack {
    stack: Vec<State>,
    cur: State,
}

impl RegionStack {
    pub fn push(&mut self, region: RegionId) -> Res<()> {
        self.stack.push(self.cur.clone());
        if self.stack.len() > REGIONS_STACK_LIMIT {
            return Err("Regions stack size limit reached".to_string());
        }
        self.cur.region = Some(region);
        Ok(())
    }

    pub fn pop(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.cur = s;
        }
    }

    pub fn add_exit(&mut self, exit: Option<BlockId>) {
        if let Some(e) = exit {
            self.cur.exits.insert(e);
        }
    }

    pub fn add_exits(&mut self, exits: &[BlockId]) {
        for e in exits {
            self.cur.exits.insert(*e);
        }
    }

    pub fn remove_exit(&mut self, exit: Option<BlockId>) {
        if let Some(e) = exit {
            self.cur.exits.remove(&e);
        }
    }

    pub fn contains_exit(&self, exit: BlockId) -> bool {
        self.cur.exits.contains(&exit)
    }

    pub fn exits(&self) -> Vec<BlockId> {
        self.cur.exits.iter().copied().collect()
    }

    pub fn peek_region(&self) -> Option<RegionId> {
        self.cur.region
    }

    pub fn clear(&mut self) {
        self.stack.clear();
        self.cur = State::default();
    }
}

pub struct RegionMaker<'a> {
    pub cfg: &'a mut MethodCfg,
    pub env: &'a MethodEnv<'a>,
    pub stack: RegionStack,
    processed: BitSet,
    regions_limit: usize,
    regions_count: usize,
}

impl<'a> RegionMaker<'a> {
    pub fn new(cfg: &'a mut MethodCfg, env: &'a MethodEnv<'a>) -> Self {
        let regions_limit = cfg.blocks.len() * 400;
        RegionMaker {
            cfg,
            env,
            stack: RegionStack::default(),
            processed: BitSet::new(),
            regions_limit,
            regions_count: 0,
        }
    }

    pub fn make_mth_region(&mut self) -> Res<RegionId> {
        let enter = self.cfg.enter;
        self.make_region(enter)
    }

    pub fn is_processed(&self, block: BlockId) -> bool {
        self.processed.get(self.cfg.block(block).pos)
    }

    pub fn clear_block_processed_state(&mut self, block: BlockId) {
        let p = self.cfg.block(block).pos;
        self.processed.clear(p);
    }

    pub fn make_region(&mut self, start_block: BlockId) -> Res<RegionId> {
        let parent = self.stack.peek_region();
        let region = self.cfg.regions.new_plain(parent);
        if self.stack.contains_exit(start_block) {
            self.insert_edge_insns(region, start_block);
            return Ok(region);
        }
        let pos = self.cfg.block(start_block).pos;
        let was_processed = self.processed.get(pos);
        self.processed.set(pos);
        if was_processed {
            // Add block to multiple regions (duplicate the instructions in decompiled code)
            // and allow processing to continue
            if !self.cfg.block(start_block).contains(bflag::DUPLICATED) {
                self.cfg.warnings.push(format!(
                    "Code duplicated, block: {} {}",
                    self.cfg.block_to_string(start_block),
                    self.cfg.block_attributes_string(start_block)
                ));
                self.cfg.block_mut(start_block).add(bflag::DUPLICATED);
            }
        }
        let mut next = Some(start_block);
        while let Some(n) = next {
            next = self.traverse(region, n)?;
            self.regions_count += 1;
            if self.regions_count > self.regions_limit {
                return Err(format!(
                    "Regions count limit reached at block {start_block}"
                ));
            }
        }
        Ok(region)
    }

    /// Recursively traverse all blocks from `block` until block from `exits`
    fn traverse(&mut self, r: RegionId, block: BlockId) -> Res<Option<BlockId>> {
        if self.cfg.block(block).contains(bflag::MTH_EXIT_BLOCK) {
            return Ok(None);
        }
        let mut next = None;
        let mut processed = false;
        let loops = self.cfg.block(block).loops.clone();
        let loop_count = loops.len();
        if loop_count != 0 && self.cfg.block(block).contains(bflag::LOOP_START) {
            if loop_count == 1 {
                next = self.process_loop(r, loops[0])?;
                processed = true;
            } else {
                for l in &loops {
                    if self.cfg.loop_infos[*l].start == block {
                        next = self.process_loop(r, *l)?;
                        processed = true;
                        break;
                    }
                }
            }
        }
        let insn = last_insn(self.cfg, block);
        if !processed {
            if let Some(insn) = insn {
                match self.cfg.insn(insn).ty {
                    InsnType::If => {
                        next = self.process_if(r, block, insn)?;
                        processed = true;
                    }
                    InsnType::Switch => {
                        next = self.process_switch(r, block, insn)?;
                        processed = true;
                    }
                    InsnType::MonitorEnter => {
                        next = self.process_sync(r, block, insn)?;
                        processed = true;
                    }
                    _ => {}
                }
            }
        }
        if !processed {
            self.cfg.regions.plain_add(r, Container::Block(block));
            next = next_block(self.cfg, block);
        }
        if let Some(n) = next {
            if !self.stack.contains_exit(block) && !self.stack.contains_exit(n) {
                return Ok(Some(n));
            }
        }
        Ok(None)
    }

    fn insert_edge_insns(&mut self, region: RegionId, exit_block: BlockId) {
        let edge_insns: Vec<_> = self.cfg.regions.edge_insns_of(exit_block).to_vec();
        if edge_insns.is_empty() {
            return;
        }
        let mut insns: Vec<InsnId> = Vec::new();
        for ty in [InsnType::Break, InsnType::Continue] {
            for e in &edge_insns {
                if self.cfg.insn(e.insn).ty == ty {
                    insns.push(e.insn);
                    break;
                }
            }
        }
        let c = self.cfg.regions.new_insn_container(insns);
        self.cfg.regions.plain_add(region, c);
    }

    /// `new InsnNode(type, 0)`
    pub fn new_plain_insn(&mut self, ty: InsnType) -> InsnId {
        self.cfg.add_insn(InsnNode::plain(ty))
    }
}
