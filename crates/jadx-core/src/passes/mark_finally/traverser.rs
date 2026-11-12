//! Comparison of two block subgraphs (`visitors.finaly.traverser`).
//!
//! The "finally" subgraph is the code of the catch-all handler, a "candidate" subgraph is the code
//! of another exit of the try. Both are walked backwards from their scope terminus, block by block,
//! comparing instructions; the result lists the matching instruction pairs and the blocks that were
//! matched entirely.
//!
//! The reference shares mutable objects between states (centrality states, block positions, the
//! cache of compared block pairs and the path objects themselves), and the traversal relies on that;
//! the same sharing is kept here with reference counted cells.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::model::cfg::MethodCfg;
use crate::model::insn::{BlockId, InsnArg, InsnId, InsnKind, InsnNode, InsnType, RegisterArg};
use crate::passes::block_processor::insn_is_same as kind_is_same;
use crate::passes::block_utils::get_bottom_common_predecessor;

type Res<T> = Result<T, String>;

/// Identity of a register argument (`RegisterArg.equals`: register number and SSA variable).
type RegKey = (u32, Option<u32>);

pub(super) fn reg_key(cfg: &MethodCfg, r: &RegisterArg) -> RegKey {
    (r.reg_num, r.svar.map(|v| cfg.svars[v].version))
}

// ---- instruction comparison (`SameInstructionsStrategyImpl`) ----------------------------------

/// `InsnNode.isSame`
pub fn insn_is_same(cfg: &MethodCfg, a: InsnId, b: InsnId) -> bool {
    if a == b {
        return true;
    }
    let (x, y) = (cfg.insn(a), cfg.insn(b));
    if x.ty != y.ty || x.args.len() != y.args.len() {
        return false;
    }
    for (p, q) in x.args.iter().zip(&y.args) {
        if let InsnArg::Wrapped(w) = p {
            let InsnArg::Wrapped(w2) = q else {
                return false;
            };
            if !insn_is_same(cfg, w.insn, w2.insn) {
                return false;
            }
        }
    }
    match (&x.kind, &y.kind) {
        (
            InsnKind::Constructor {
                call_mth: m1,
                call_type: t1,
            },
            InsnKind::Constructor {
                call_mth: m2,
                call_type: t2,
            },
        ) => m1 == m2 && t1 == t2,
        (InsnKind::Phi { .. }, InsnKind::Phi { .. }) => true,
        _ => kind_is_same(x, y),
    }
}

/// `InsnArg.equals`
fn arg_equals(cfg: &MethodCfg, a: &InsnArg, b: &InsnArg) -> bool {
    match (a, b) {
        (InsnArg::Register(x), InsnArg::Register(y)) => reg_key(cfg, x) == reg_key(cfg, y),
        (InsnArg::Literal(x), InsnArg::Literal(y)) => x.literal == y.literal && x.ty == y.ty,
        (InsnArg::Wrapped(x), InsnArg::Wrapped(y)) => {
            if !insn_is_same(cfg, x.insn, y.insn) {
                return false;
            }
            let (p, q) = (cfg.insn(x.insn), cfg.insn(y.insn));
            p.args
                .iter()
                .zip(&q.args)
                .all(|(m, n)| arg_equals(cfg, m, n))
        }
        _ => false,
    }
}

fn is_const_insn(insn: &InsnNode) -> bool {
    matches!(
        insn.ty,
        InsnType::Const | InsnType::ConstStr | InsnType::ConstClass
    )
}

/// `InsnArg.isConst`
fn is_const_arg(cfg: &MethodCfg, a: &InsnArg) -> bool {
    match a {
        InsnArg::Literal(_) => true,
        InsnArg::Wrapped(w) => is_const_insn(cfg.insn(w.insn)),
        InsnArg::Register(_) | InsnArg::Named(_) => false,
    }
}

/// `RegisterArg.sameCodeVar`
fn same_code_var(cfg: &MethodCfg, a: &RegisterArg, b: &RegisterArg) -> Res<bool> {
    let code_var = |r: &RegisterArg| -> Res<usize> {
        let v = r.svar.ok_or("register without SSA variable")?;
        cfg.svars[v]
            .code_var
            .ok_or_else(|| "Code variable not set".to_string())
    };
    Ok(code_var(a)? == code_var(b)?)
}

/// `SameInstructionsStrategyImpl.assignInsnDifferent`
fn assign_insn_different(cfg: &MethodCfg, dup: &RegisterArg, fin: &RegisterArg) -> bool {
    let assign = |r: &RegisterArg| r.svar.and_then(|v| cfg.svar_assign_insn(v));
    let (Some(f_assign), Some(d_assign)) = (assign(fin), assign(dup)) else {
        return true;
    };
    if !insn_is_same(cfg, f_assign, d_assign) {
        return true;
    }
    // the reference compares the argument list of the first instruction with itself here
    false
}

/// `SameInstructionsStrategyImpl.isSameArgs`
fn is_same_args(cfg: &MethodCfg, dup: &InsnArg, fin: &InsnArg) -> Res<bool> {
    let dup_reg = dup.as_register();
    if dup_reg.is_some() != fin.as_register().is_some() {
        return Ok(false);
    }
    if let (Some(d), Some(f)) = (dup_reg, fin.as_register()) {
        // register debug info is not attached at this point
        if !same_code_var(cfg, d, f)? && assign_insn_different(cfg, d, f) {
            return Ok(false);
        }
    }
    let rem_const = is_const_arg(cfg, dup);
    if rem_const != is_const_arg(cfg, fin) {
        return Ok(false);
    }
    Ok(!(rem_const && !arg_equals(cfg, dup, fin)))
}

/// `SameInstructionsStrategyImpl.sameInsns`
fn same_insns(cfg: &MethodCfg, dup: InsnId, fin: InsnId) -> Res<bool> {
    if !insn_is_same(cfg, dup, fin) {
        return Ok(false);
    }
    let (d, f) = (cfg.insn(dup), cfg.insn(fin));
    for (a, b) in d.args.iter().zip(&f.args) {
        if !is_same_args(cfg, a, b)? {
            return Ok(false);
        }
    }
    Ok(true)
}

// ---- shared state objects -----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Finally,
    Candidate,
}

/// `CentralityState`
#[derive(Clone)]
struct Centrality {
    allows_central: bool,
    allows_non_starting: bool,
    outputs: HashSet<RegKey>,
}

type CentralityRef = Rc<RefCell<Centrality>>;

impl Centrality {
    fn new(allows_non_starting: bool) -> CentralityRef {
        Rc::new(RefCell::new(Centrality {
            allows_central: true,
            allows_non_starting,
            outputs: HashSet::new(),
        }))
    }

    fn add_output_args(&mut self, cfg: &MethodCfg, insn: InsnId) {
        for a in &cfg.insn(insn).args {
            if let InsnArg::Register(r) = a {
                self.outputs.insert(reg_key(cfg, r));
            }
        }
    }

    fn has_allowable_output(&self, cfg: &MethodCfg, insn: InsnId) -> bool {
        if self.outputs.is_empty() {
            return false;
        }
        match &cfg.insn(insn).result {
            Some(r) => self.outputs.contains(&reg_key(cfg, r)),
            None => false,
        }
    }
}

fn dup_centrality(c: &CentralityRef) -> CentralityRef {
    Rc::new(RefCell::new(c.borrow().clone()))
}

/// `TraverserBlockInfo`: the block and how many instructions were skipped at its bottom and top.
#[derive(Clone)]
struct BlockInfo {
    block: BlockId,
    bottom_offset: usize,
    top_offset: usize,
    bottom_implicit_count: usize,
}

type InfoRef = Rc<RefCell<BlockInfo>>;

impl BlockInfo {
    fn new(block: BlockId) -> InfoRef {
        Rc::new(RefCell::new(BlockInfo {
            block,
            bottom_offset: 0,
            top_offset: 0,
            bottom_implicit_count: 0,
        }))
    }

    /// `getInsnsSlice`
    fn insns_slice(&self, cfg: &MethodCfg) -> Res<Vec<InsnId>> {
        let insns = &cfg.block(self.block).insns;
        let skipped = self.bottom_offset + self.top_offset;
        if skipped > insns.len() {
            return Err(format!(
                "Attempted to get instructions slice of block {} with {} skipped instructions whilst only having {} instructions in block.",
                self.block,
                skipped,
                insns.len()
            ));
        }
        Ok(insns[self.top_offset..insns.len() - self.bottom_offset].to_vec())
    }
}

fn dup_info(i: &InfoRef) -> InfoRef {
    Rc::new(RefCell::new(i.borrow().clone()))
}

// ---- states -------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Compare {
    NotReady,
    ReadyToCompare,
}

enum StateKind {
    NewBlock {
        centrality: CentralityRef,
        info: InfoRef,
    },
    NoBlock {
        centrality: CentralityRef,
        source: BlockId,
    },
    AwaitingInsnCompare {
        centrality: CentralityRef,
        info: InfoRef,
    },
    IdentifiedScope {
        centrality: CentralityRef,
        roots: Vec<BlockId>,
        terminator: Option<BlockId>,
    },
    UnknownAdvance {
        centrality: CentralityRef,
        next_blocks: Vec<BlockId>,
    },
    Terminal,
    RecoveredFromCache {
        underlying: StateRef,
    },
}

type StateRef = Rc<StateKind>;

impl StateKind {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            StateKind::Terminal | StateKind::RecoveredFromCache { .. }
        )
    }

    fn compare_state(&self) -> Compare {
        match self {
            StateKind::AwaitingInsnCompare { .. }
            | StateKind::IdentifiedScope { .. }
            | StateKind::UnknownAdvance { .. } => Compare::ReadyToCompare,
            _ => Compare::NotReady,
        }
    }

    fn centrality(&self) -> Res<CentralityRef> {
        match self {
            StateKind::NewBlock { centrality, .. }
            | StateKind::NoBlock { centrality, .. }
            | StateKind::AwaitingInsnCompare { centrality, .. }
            | StateKind::IdentifiedScope { centrality, .. }
            | StateKind::UnknownAdvance { centrality, .. } => Ok(centrality.clone()),
            _ => Err("Centrality state is not supported for this traverser state".to_string()),
        }
    }

    /// `getBlockInsnInfo`
    fn block_info(&self) -> Option<InfoRef> {
        match self {
            StateKind::NewBlock { info, .. } | StateKind::AwaitingInsnCompare { info, .. } => {
                Some(info.clone())
            }
            _ => None,
        }
    }

    /// `duplicate`: deep clone of the state.
    fn duplicate(&self) -> StateRef {
        Rc::new(match self {
            StateKind::NewBlock { centrality, info } => StateKind::NewBlock {
                centrality: dup_centrality(centrality),
                info: dup_info(info),
            },
            StateKind::NoBlock { centrality, source } => StateKind::NoBlock {
                centrality: dup_centrality(centrality),
                source: *source,
            },
            StateKind::AwaitingInsnCompare { centrality, info } => StateKind::AwaitingInsnCompare {
                centrality: dup_centrality(centrality),
                info: dup_info(info),
            },
            StateKind::IdentifiedScope {
                centrality,
                roots,
                terminator,
            } => StateKind::IdentifiedScope {
                centrality: centrality.clone(),
                roots: roots.clone(),
                terminator: *terminator,
            },
            StateKind::UnknownAdvance {
                centrality,
                next_blocks,
            } => StateKind::UnknownAdvance {
                centrality: dup_centrality(centrality),
                next_blocks: next_blocks.clone(),
            },
            StateKind::Terminal => StateKind::Terminal,
            StateKind::RecoveredFromCache { underlying } => StateKind::RecoveredFromCache {
                underlying: underlying.clone(),
            },
        })
    }
}

/// `TraverserStateFactory`: states are produced when the new path exists (duplicating factories
/// copy their base state at that moment).
enum Factory {
    NewBlock(CentralityRef, InfoRef),
    NoBlock(CentralityRef, BlockId),
    IdentifiedScope(CentralityRef, Vec<BlockId>, Option<BlockId>),
    Terminal,
    Recovered(StateRef),
    Duplicated(StateRef),
}

impl Factory {
    fn generate(&self) -> StateRef {
        match self {
            Factory::NewBlock(c, i) => Rc::new(StateKind::NewBlock {
                centrality: c.clone(),
                info: i.clone(),
            }),
            Factory::NoBlock(c, b) => Rc::new(StateKind::NoBlock {
                centrality: c.clone(),
                source: *b,
            }),
            Factory::IdentifiedScope(c, roots, term) => Rc::new(StateKind::IdentifiedScope {
                centrality: c.clone(),
                roots: roots.clone(),
                terminator: *term,
            }),
            Factory::Terminal => Rc::new(StateKind::Terminal),
            Factory::Recovered(s) => Rc::new(StateKind::RecoveredFromCache {
                underlying: s.clone(),
            }),
            Factory::Duplicated(s) => s.duplicate(),
        }
    }
}

// ---- path state ---------------------------------------------------------------------------------

/// `GlobalTraverserSourceState`
struct Global {
    contained: HashSet<BlockId>,
}

/// `TraverserGlobalCommonState`: results of already compared block pairs.
#[derive(Default)]
struct CommonState {
    searched: HashMap<(BlockId, BlockId), Vec<PathRef>>,
}

/// Insertion ordered set of matched instruction pairs.
#[derive(Default, Clone)]
struct PairSet {
    items: Vec<(InsnId, InsnId)>,
    seen: HashSet<(InsnId, InsnId)>,
}

impl PairSet {
    fn add(&mut self, p: (InsnId, InsnId)) {
        if self.seen.insert(p) {
            self.items.push(p);
        }
    }
}

/// `BlockCompletionMonitor`: which instruction indices of a block are not matched yet.
#[derive(Clone)]
struct Monitor {
    block: BlockId,
    unmatched: HashSet<usize>,
}

impl Monitor {
    fn new(cfg: &MethodCfg, block: BlockId) -> Self {
        Monitor {
            block,
            unmatched: (0..cfg.block(block).insns.len()).collect(),
        }
    }

    fn register(&mut self, info: &BlockInfo, number_matched: usize) {
        if info.block != self.block {
            return;
        }
        let bot = info.bottom_offset;
        for i in 0..number_matched {
            self.unmatched.remove(&(bot + i));
        }
        let implicit = info.bottom_implicit_count;
        if bot == implicit {
            for i in 0..implicit {
                self.unmatched.remove(&i);
            }
        }
    }

    fn merge_with(&mut self, other: &Monitor) {
        if other.block != self.block {
            return;
        }
        self.unmatched.retain(|i| other.unmatched.contains(i));
    }
}

#[derive(Default, Clone)]
struct MonitorMap {
    map: HashMap<BlockId, Monitor>,
}

impl MonitorMap {
    fn register(&mut self, cfg: &MethodCfg, info: &BlockInfo, number_matched: usize) {
        self.map
            .entry(info.block)
            .or_insert_with(|| Monitor::new(cfg, info.block))
            .register(info, number_matched);
    }

    fn merge_map(&mut self, other: &MonitorMap) {
        for m in other.map.values() {
            match self.map.get_mut(&m.block) {
                Some(mine) => mine.merge_with(m),
                None => {
                    self.map.insert(m.block, m.clone());
                }
            }
        }
    }

    fn fully_matched(&self) -> Vec<BlockId> {
        self.map
            .values()
            .filter(|m| m.unmatched.is_empty())
            .map(|m| m.block)
            .collect()
    }
}

/// `TraverserActivePathState`
pub struct Path {
    finally: StateRef,
    candidate: StateRef,
    finally_global: Rc<Global>,
    candidate_global: Rc<Global>,
    common: Rc<RefCell<CommonState>>,
    matched: Rc<RefCell<PairSet>>,
    finally_monitor: Rc<RefCell<MonitorMap>>,
    candidate_monitor: Rc<RefCell<MonitorMap>>,
}

pub type PathRef = Rc<RefCell<Path>>;

impl Path {
    /// Root path of a new traversal.
    pub fn new(
        cfg: &MethodCfg,
        finally_terminus: BlockId,
        candidate_terminus: BlockId,
        finally_blocks: &[BlockId],
        candidate_blocks: &[BlockId],
    ) -> PathRef {
        let allow_f_skip = !cfg.block(finally_terminus).insns.is_empty();
        let allow_c_skip = !cfg.block(candidate_terminus).insns.is_empty();
        let finally = Rc::new(StateKind::NewBlock {
            centrality: Centrality::new(allow_f_skip),
            info: BlockInfo::new(finally_terminus),
        });
        let candidate = Rc::new(StateKind::NewBlock {
            centrality: Centrality::new(allow_c_skip),
            info: BlockInfo::new(candidate_terminus),
        });
        Rc::new(RefCell::new(Path {
            finally,
            candidate,
            finally_global: Rc::new(Global {
                contained: finally_blocks.iter().copied().collect(),
            }),
            candidate_global: Rc::new(Global {
                contained: candidate_blocks.iter().copied().collect(),
            }),
            common: Rc::new(RefCell::new(CommonState::default())),
            matched: Rc::default(),
            finally_monitor: Rc::default(),
            candidate_monitor: Rc::default(),
        }))
    }

    fn state(&self, side: Side) -> StateRef {
        match side {
            Side::Finally => self.finally.clone(),
            Side::Candidate => self.candidate.clone(),
        }
    }

    fn global(&self, side: Side) -> Rc<Global> {
        match side {
            Side::Finally => self.finally_global.clone(),
            Side::Candidate => self.candidate_global.clone(),
        }
    }

    fn set_state(&mut self, side: Side, s: StateRef) {
        match side {
            Side::Finally => self.finally = s,
            Side::Candidate => self.candidate = s,
        }
    }

    /// `produceFromFactories`: shallow clone sharing the matched instructions, the monitors and the
    /// global state with `prev`.
    fn produce(prev: &PathRef, finally: &Factory, candidate: &Factory) -> PathRef {
        let p = prev.borrow();
        Rc::new(RefCell::new(Path {
            finally: finally.generate(),
            candidate: candidate.generate(),
            finally_global: p.finally_global.clone(),
            candidate_global: p.candidate_global.clone(),
            common: p.common.clone(),
            matched: p.matched.clone(),
            finally_monitor: p.finally_monitor.clone(),
            candidate_monitor: p.candidate_monitor.clone(),
        }))
    }

    /// `duplicate`: deep clone.
    fn duplicate(&self) -> PathRef {
        Rc::new(RefCell::new(Path {
            finally: self.finally.duplicate(),
            candidate: self.candidate.duplicate(),
            finally_global: self.finally_global.clone(),
            candidate_global: self.candidate_global.clone(),
            common: self.common.clone(),
            matched: Rc::new(RefCell::new(self.matched.borrow().clone())),
            finally_monitor: Rc::new(RefCell::new(self.finally_monitor.borrow().clone())),
            candidate_monitor: Rc::new(RefCell::new(self.candidate_monitor.borrow().clone())),
        }))
    }

    /// Matched instruction pairs (finally instruction, candidate instruction).
    pub fn matched_insns(&self) -> Vec<(InsnId, InsnId)> {
        self.matched.borrow().items.clone()
    }

    pub fn fully_matched_finally_blocks(&self) -> Vec<BlockId> {
        self.finally_monitor.borrow().fully_matched()
    }

    pub fn fully_matched_candidate_blocks(&self) -> Vec<BlockId> {
        self.candidate_monitor.borrow().fully_matched()
    }

    fn register_with_block_info(&self, cfg: &MethodCfg, info: &BlockInfo, number_matched: usize) {
        let monitor = if self.finally_global.contained.contains(&info.block) {
            &self.finally_monitor
        } else {
            &self.candidate_monitor
        };
        monitor.borrow_mut().register(cfg, info, number_matched);
    }
}

/// `mergeWith`
fn merge_paths(this: &PathRef, others: &[PathRef]) {
    let (matched, f_mon, c_mon) = {
        let p = this.borrow();
        (
            p.matched.clone(),
            p.finally_monitor.clone(),
            p.candidate_monitor.clone(),
        )
    };
    for other in others {
        let (o_matched, o_f, o_c) = {
            let o = other.borrow();
            let items = o.matched.borrow().items.clone();
            let f = o.finally_monitor.borrow().clone();
            let c = o.candidate_monitor.borrow().clone();
            (items, f, c)
        };
        let mut m = matched.borrow_mut();
        for p in o_matched {
            m.add(p);
        }
        drop(m);
        f_mon.borrow_mut().merge_map(&o_f);
        c_mon.borrow_mut().merge_map(&o_c);
    }
}

// ---- block visitors -------------------------------------------------------------------------------

/// `NewBlockTraverserState` handler: skip implicit instructions, then instructions leading to the
/// end of the path (`BaseBlockTraverserHandler`).
fn handle_new_block(cfg: &MethodCfg, path: &PathRef, side: Side) -> Res<()> {
    let state = path.borrow().state(side);
    let StateKind::NewBlock { centrality, info } = &*state else {
        return Err("Expected a new block state".into());
    };
    // ImplicitInsnBlockTraverserVisitor: trailing GOTOs are skipped
    {
        let slice = info.borrow().insns_slice(cfg)?;
        let mut delta = 0;
        for &insn in slice.iter().rev() {
            if cfg.insn(insn).ty != InsnType::Goto {
                break;
            }
            delta += 1;
        }
        let mut i = info.borrow_mut();
        i.bottom_offset += delta;
        i.bottom_implicit_count += delta;
    }
    // PathEndBlockTraverserVisitor
    let next: StateRef = if !centrality.borrow().allows_central {
        Rc::new(StateKind::AwaitingInsnCompare {
            centrality: centrality.clone(),
            info: info.clone(),
        })
    } else {
        let slice = info.borrow().insns_slice(cfg)?;
        let mut delta = 0;
        for &insn in slice.iter().rev() {
            let n = cfg.insn(insn);
            if matches!(n.ty, InsnType::Return | InsnType::Throw) {
                // the argument of the path end instruction may be set by instructions before it
                if let Some(InsnArg::Register(r)) = n.args.first() {
                    centrality.borrow_mut().outputs.insert(reg_key(cfg, r));
                }
                delta += 1;
            } else if centrality.borrow().has_allowable_output(cfg, insn) {
                delta += 1;
                centrality.borrow_mut().add_output_args(cfg, insn);
            } else {
                break;
            }
        }
        let (block, no_insns_left) = {
            let mut i = info.borrow_mut();
            i.bottom_offset += delta;
            (i.block, i.bottom_offset >= cfg.block(i.block).insns.len())
        };
        if no_insns_left {
            Rc::new(StateKind::NoBlock {
                centrality: centrality.clone(),
                source: block,
            })
        } else {
            Rc::new(StateKind::AwaitingInsnCompare {
                centrality: centrality.clone(),
                info: info.clone(),
            })
        }
    };
    path.borrow_mut().set_state(side, next);
    Ok(())
}

/// `NoBlockTraverserState` handler: continue with the predecessors inside the scope
/// (`PredecessorBlockPathTraverserHandler`).
fn handle_no_block(cfg: &MethodCfg, path: &PathRef, side: Side) -> Res<()> {
    let (state, global) = {
        let p = path.borrow();
        (p.state(side), p.global(side))
    };
    let StateKind::NoBlock { centrality, source } = &*state else {
        return Err("Expected a no block state".into());
    };
    let contained: Vec<BlockId> = cfg
        .block(*source)
        .preds
        .iter()
        .copied()
        .filter(|p| global.contained.contains(p))
        .collect();
    let next: StateRef = match contained.len() {
        0 => Rc::new(StateKind::Terminal),
        1 => Rc::new(StateKind::NewBlock {
            centrality: centrality.clone(),
            info: BlockInfo::new(contained[0]),
        }),
        _ => Rc::new(StateKind::UnknownAdvance {
            centrality: centrality.clone(),
            next_blocks: contained,
        }),
    };
    path.borrow_mut().set_state(side, next);
    Ok(())
}

// ---- instruction comparison handler -------------------------------------------------------------

fn handle_instructions(cfg: &MethodCfg, path: &PathRef) -> Res<Vec<PathRef>> {
    let (finally_state, candidate_state, common) = {
        let p = path.borrow();
        (p.finally.clone(), p.candidate.clone(), p.common.clone())
    };
    let f_info = finally_state
        .block_info()
        .ok_or("state without block info")?;
    let c_info = candidate_state
        .block_info()
        .ok_or("state without block info")?;
    let (fb, cb) = (f_info.borrow().block, c_info.borrow().block);
    let new_state = compare_instructions(cfg, path)?;
    common
        .borrow_mut()
        .searched
        .insert((fb, cb), vec![new_state.clone()]);
    Ok(vec![new_state])
}

/// `InstructionBlockComparatorTraverserVisitor.visit`
fn compare_instructions(cfg: &MethodCfg, state: &PathRef) -> Res<PathRef> {
    let (finally_state, candidate_state) = {
        let p = state.borrow();
        (p.finally.clone(), p.candidate.clone())
    };
    let f_info = finally_state
        .block_info()
        .ok_or("The instruction comparator handler has received a state which does not support block insn info")?;
    let c_info = candidate_state
        .block_info()
        .ok_or("The instruction comparator handler has received a state which does not support block insn info")?;
    let (finally_block, candidate_block) = (f_info.borrow().block, c_info.borrow().block);
    let f_insns = f_info.borrow().insns_slice(cfg)?;
    let c_insns = c_info.borrow().insns_slice(cfg)?;
    let (f_size, c_size) = (f_insns.len(), c_insns.len());
    let max_iterate = f_size.min(c_size);

    // compare in reverse, count how many instructions match
    let mut matching: Vec<(InsnId, InsnId)> = Vec::with_capacity(max_iterate);
    for i in 0..max_iterate {
        let cand = c_insns[c_size - i - 1];
        let fin = f_insns[f_size - i - 1];
        if !same_insns(cfg, cand, fin)? {
            break;
        }
        matching.push((fin, cand));
    }
    let matched = matching.len();

    {
        let p = state.borrow();
        p.register_with_block_info(cfg, &f_info.borrow().clone(), matched);
        p.register_with_block_info(cfg, &c_info.borrow().clone(), matched);
        let mut set = p.matched.borrow_mut();
        for m in &matching {
            set.add(*m);
        }
    }

    let same_sized = f_size == c_size;
    let all_matched = matched == max_iterate;
    let none_matched = matched == 0;

    if all_matched {
        if same_sized {
            perfect_match_state(
                state,
                &finally_state,
                &candidate_state,
                finally_block,
                candidate_block,
            )
        } else {
            uneven_match_state(
                state,
                &finally_state,
                &candidate_state,
                finally_block,
                candidate_block,
                f_size,
                c_size,
            )
        }
    } else if none_matched && either_allows_block_skip(&finally_state, &candidate_state)? {
        block_skip_state(
            state,
            &finally_state,
            &candidate_state,
            finally_block,
            candidate_block,
        )
    } else {
        // the first instructions of the block do not match: no further block can be a duplicate
        Ok(Path::produce(state, &Factory::Terminal, &Factory::Terminal))
    }
}

fn either_allows_block_skip(f: &StateRef, c: &StateRef) -> Res<bool> {
    Ok(
        f.centrality()?.borrow().allows_non_starting
            || c.centrality()?.borrow().allows_non_starting,
    )
}

fn perfect_match_state(
    prev: &PathRef,
    finally_state: &StateRef,
    candidate_state: &StateRef,
    finally_block: BlockId,
    candidate_block: BlockId,
) -> Res<PathRef> {
    let f_cent = dup_centrality(&finally_state.centrality()?);
    let c_cent = dup_centrality(&candidate_state.centrality()?);
    for c in [&f_cent, &c_cent] {
        let mut c = c.borrow_mut();
        c.allows_central = false;
        c.allows_non_starting = false;
    }
    Ok(Path::produce(
        prev,
        &Factory::NoBlock(f_cent, finally_block),
        &Factory::NoBlock(c_cent, candidate_block),
    ))
}

fn uneven_match_state(
    prev: &PathRef,
    finally_state: &StateRef,
    candidate_state: &StateRef,
    finally_block: BlockId,
    candidate_block: BlockId,
    f_size: usize,
    c_size: usize,
) -> Res<PathRef> {
    let max_iterate = f_size.max(c_size);
    let finally_overruns = f_size > c_size;
    let (new_finally, new_candidate, adjusted, delta);
    if finally_overruns {
        // more finally instructions than candidate instructions
        let cand_cent = dup_centrality(&candidate_state.centrality()?);
        {
            let mut c = cand_cent.borrow_mut();
            c.allows_central = false;
            c.allows_non_starting = false;
        }
        {
            let f = finally_state.centrality()?;
            let mut f = f.borrow_mut();
            f.allows_central = false;
            f.allows_non_starting = false;
        }
        delta = f_size - max_iterate;
        new_finally = Factory::Duplicated(finally_state.clone());
        adjusted = finally_state.block_info();
        new_candidate = Factory::NoBlock(cand_cent, candidate_block);
    } else {
        // more candidate instructions than finally instructions
        let f_cent = dup_centrality(&finally_state.centrality()?);
        {
            let mut c = f_cent.borrow_mut();
            c.allows_central = false;
            c.allows_non_starting = false;
        }
        {
            let c = candidate_state.centrality()?;
            let mut c = c.borrow_mut();
            c.allows_central = false;
            c.allows_non_starting = false;
        }
        delta = c_size - max_iterate;
        new_candidate = Factory::Duplicated(candidate_state.clone());
        adjusted = candidate_state.block_info();
        new_finally = Factory::NoBlock(f_cent, finally_block);
    }
    if let Some(info) = adjusted {
        info.borrow_mut().bottom_offset += delta;
    }
    Ok(Path::produce(prev, &new_finally, &new_candidate))
}

fn block_skip_state(
    prev: &PathRef,
    finally_state: &StateRef,
    candidate_state: &StateRef,
    finally_block: BlockId,
    candidate_block: BlockId,
) -> Res<PathRef> {
    let f_cent = finally_state.centrality()?;
    let c_cent = candidate_state.centrality()?;
    // fix up the finally path first
    let finally_allows = f_cent.borrow().allows_non_starting;
    if finally_allows {
        f_cent.borrow_mut().allows_non_starting = false;
        Ok(Path::produce(
            prev,
            &Factory::NoBlock(f_cent, finally_block),
            &Factory::Duplicated(candidate_state.clone()),
        ))
    } else {
        c_cent.borrow_mut().allows_non_starting = false;
        Ok(Path::produce(
            prev,
            &Factory::Duplicated(finally_state.clone()),
            &Factory::NoBlock(c_cent, candidate_block),
        ))
    }
}

// ---- merging of paths ---------------------------------------------------------------------------

/// `PredecessorMergeActivePathTraverserHandler`
fn handle_predecessor_merge(cfg: &MethodCfg, path: &PathRef) -> Res<Vec<PathRef>> {
    let (finally_state, candidate_state) = {
        let p = path.borrow();
        (p.finally.clone(), p.candidate.clone())
    };
    let f_needs = finally_state.compare_state() == Compare::ReadyToCompare;
    let c_needs = candidate_state.compare_state() == Compare::ReadyToCompare;
    if f_needs && c_needs {
        let (
            StateKind::UnknownAdvance {
                next_blocks: fb, ..
            },
            StateKind::UnknownAdvance {
                next_blocks: cb, ..
            },
        ) = (&*finally_state, &*candidate_state)
        else {
            return Err("Unexpected traverser state class".into());
        };
        merge_scopes(cfg, path, fb, cb)
    } else {
        let (advancing, other, from_finally) = if f_needs {
            (&finally_state, &candidate_state, true)
        } else {
            (&candidate_state, &finally_state, false)
        };
        let StateKind::UnknownAdvance {
            centrality,
            next_blocks,
        } = &**advancing
        else {
            return Err("Unexpected traverser state class".into());
        };
        let mut paths = Vec::with_capacity(next_blocks.len());
        for &pred in next_blocks {
            let new_factory = Factory::NewBlock(centrality.clone(), BlockInfo::new(pred));
            let other_factory = Factory::Duplicated(other.clone());
            let dup = path.borrow().duplicate();
            paths.push(if from_finally {
                Path::produce(&dup, &new_factory, &other_factory)
            } else {
                Path::produce(&dup, &other_factory, &new_factory)
            });
        }
        Ok(paths)
    }
}

fn merge_scopes(
    cfg: &MethodCfg,
    path: &PathRef,
    finally_blocks: &[BlockId],
    candidate_blocks: &[BlockId],
) -> Res<Vec<PathRef>> {
    let (f_size, c_size) = (finally_blocks.len(), candidate_blocks.len());
    if f_size == 0 {
        return Err("/ by zero".into());
    }
    if c_size % f_size == 0 && c_size == f_size {
        let duplication_count = c_size / f_size;
        let mut states = Vec::with_capacity(duplication_count);
        for i in 0..duplication_count {
            let subset: Vec<BlockId> = (0..f_size)
                .map(|j| candidate_blocks[i * f_size + j])
                .collect();
            states.push(scope_for_blocks(cfg, path, finally_blocks, &subset)?);
        }
        Ok(states)
    } else {
        Ok(vec![Path::produce(
            path,
            &Factory::Terminal,
            &Factory::Terminal,
        )])
    }
}

fn scope_for_blocks(
    cfg: &MethodCfg,
    path: &PathRef,
    finally_blocks: &[BlockId],
    candidate_blocks: &[BlockId],
) -> Res<PathRef> {
    let (finally_state, candidate_state) = {
        let p = path.borrow();
        (p.finally.clone(), p.candidate.clone())
    };
    let f_term = get_bottom_common_predecessor(cfg, finally_blocks);
    let c_term = get_bottom_common_predecessor(cfg, candidate_blocks);
    Ok(Path::produce(
        path,
        &Factory::IdentifiedScope(finally_state.centrality()?, finally_blocks.to_vec(), f_term),
        &Factory::IdentifiedScope(
            candidate_state.centrality()?,
            candidate_blocks.to_vec(),
            c_term,
        ),
    ))
}

/// `PostMergeStatus`
#[derive(Default)]
struct PostMerge {
    finally_outputs: HashSet<RegKey>,
    candidate_outputs: HashSet<RegKey>,
    finally_allows_central: bool,
    candidate_allows_central: bool,
    imperfect: bool,
}

fn scope_split_post_merge_status(paths: &[PathRef]) -> Res<PostMerge> {
    let mut status = PostMerge::default();
    for path in paths {
        let (f, c) = {
            let p = path.borrow();
            (p.finally.clone(), p.candidate.clone())
        };
        let (finally_state, candidate_state);
        if f.is_terminal() || c.is_terminal() {
            let (
                StateKind::RecoveredFromCache { underlying: fu },
                StateKind::RecoveredFromCache { underlying: cu },
            ) = (&*f, &*c)
            else {
                status.imperfect = true;
                continue;
            };
            // `canContinue`: the underlying state is terminal
            if fu.is_terminal() || cu.is_terminal() {
                status.imperfect = true;
                continue;
            }
            finally_state = fu.clone();
            candidate_state = cu.clone();
        } else {
            finally_state = f;
            candidate_state = c;
        }
        let f_cent = finally_state.centrality()?;
        let c_cent = candidate_state.centrality()?;
        // the reference never initialises the "allows central" flags to true
        status.finally_allows_central &= f_cent.borrow().allows_central;
        status.candidate_allows_central &= c_cent.borrow().allows_central;
        status
            .finally_outputs
            .extend(f_cent.borrow().outputs.iter().copied());
        status
            .candidate_outputs
            .extend(c_cent.borrow().outputs.iter().copied());
    }
    Ok(status)
}

/// Permutations in the order produced by the reference (which moves the used element to the end
/// of the remaining elements).
fn all_permutations(elements: &[BlockId]) -> Vec<Vec<BlockId>> {
    let mut perms = Vec::new();
    let mut stack = Vec::new();
    let mut elems = elements.to_vec();
    permutations(&mut perms, &mut elems, &mut stack, elements.len());
    perms
}

fn permutations(
    perms: &mut Vec<Vec<BlockId>>,
    elements: &mut Vec<BlockId>,
    stack: &mut Vec<BlockId>,
    size: usize,
) {
    if stack.len() == size {
        perms.push(stack.clone());
    }
    let available = elements.clone();
    for i in available {
        stack.push(i);
        if let Some(p) = elements.iter().position(|x| *x == i) {
            elements.remove(p);
        }
        permutations(perms, elements, stack, size);
        if let Some(top) = stack.pop() {
            elements.push(top);
        }
    }
}

/// Largest number of paths merged at one point (the number of permutations grows factorially).
const MAX_SCOPE_ROOTS: usize = 8;

/// `MergePathActivePathTraverserHandler`
fn handle_merge_path(cfg: &MethodCfg, path: &PathRef) -> Res<Vec<PathRef>> {
    let comparator = path.borrow().duplicate();
    let common = comparator.borrow().common.clone();
    let (finally_state, candidate_state) = {
        let p = comparator.borrow();
        (p.finally.clone(), p.candidate.clone())
    };
    let (
        StateKind::IdentifiedScope {
            centrality: f_cent,
            roots: f_roots,
            terminator: f_term,
        },
        StateKind::IdentifiedScope {
            centrality: c_cent,
            roots: c_roots,
            terminator: c_term,
        },
    ) = (&*finally_state, &*candidate_state)
    else {
        return Err("Unexpected traverser state class".into());
    };

    let abort = Abort {
        finally_terminus: *f_term,
        candidate_terminus: *c_term,
    };
    // every permutation of the candidate roots is traversed: give up on pathological scopes
    if c_roots.len() > MAX_SCOPE_ROOTS {
        return Err(format!(
            "Too many scope roots to compare: {}",
            c_roots.len()
        ));
    }
    let mut result: Option<(Vec<PathRef>, PostMerge)> = None;
    for perm in all_permutations(c_roots) {
        let mut traversal_paths = Vec::new();
        for (i, &finally_root) in f_roots.iter().enumerate() {
            let candidate_root = *perm.get(i).ok_or("Index out of bounds")?;
            traversal_paths.push(Path::produce(
                &comparator,
                &Factory::NewBlock(dup_centrality(f_cent), BlockInfo::new(finally_root)),
                &Factory::NewBlock(dup_centrality(c_cent), BlockInfo::new(candidate_root)),
            ));
        }
        let mut current_paths = Vec::new();
        for p in &traversal_paths {
            let controller = Controller::new(Some(abort.clone()));
            current_paths.extend(controller.process(cfg, p)?);
        }
        // if the termini have been compared, a non perfect match is accepted
        let terminus_evaluated = match (f_term, c_term) {
            (Some(f), Some(c)) => common.borrow().searched.contains_key(&(*f, *c)),
            _ => return Err("null block in pair".into()),
        };
        let post = scope_split_post_merge_status(&current_paths)?;
        if post.imperfect && !terminus_evaluated {
            continue;
        }
        result = Some((current_paths, post));
        break;
    }
    let Some((paths, post)) = result else {
        // no match
        return Ok(vec![Path::produce(
            &comparator,
            &Factory::Terminal,
            &Factory::Terminal,
        )]);
    };
    let new_f_cent = dup_centrality(f_cent);
    {
        let mut c = new_f_cent.borrow_mut();
        c.allows_central = post.finally_allows_central;
        c.outputs.extend(post.finally_outputs);
    }
    let new_c_cent = dup_centrality(c_cent);
    {
        let mut c = new_c_cent.borrow_mut();
        c.allows_central = post.candidate_allows_central;
        c.outputs.extend(post.candidate_outputs);
    }
    let f_terminus = f_term.ok_or("scope without terminus block")?;
    let c_terminus = c_term.ok_or("scope without terminus block")?;
    let next = Path::produce(
        &comparator,
        &Factory::NewBlock(new_f_cent, BlockInfo::new(f_terminus)),
        &Factory::NewBlock(new_c_cent, BlockInfo::new(c_terminus)),
    );
    merge_paths(&next, &paths);
    Ok(vec![next])
}

// ---- controller ---------------------------------------------------------------------------------

/// State abort condition: a state stops when it reaches the terminus of its scope.
#[derive(Clone)]
struct Abort {
    finally_terminus: Option<BlockId>,
    candidate_terminus: Option<BlockId>,
}

impl Abort {
    fn apply(&self, path: &PathRef, side: Side) -> bool {
        let state = path.borrow().state(side);
        let terminus = match side {
            Side::Finally => self.finally_terminus,
            Side::Candidate => self.candidate_terminus,
        };
        match state.block_info() {
            Some(info) => Some(info.borrow().block) == terminus,
            None => false,
        }
    }
}

/// `TraverserController`
pub struct Controller {
    abort: Option<Abort>,
}

impl Controller {
    pub fn new_plain() -> Self {
        Controller { abort: None }
    }

    fn new(abort: Option<Abort>) -> Self {
        Controller { abort }
    }

    /// Evaluates a path until both subgraphs ended, returning the paths at the point of
    /// termination of each branch.
    pub fn process(&self, cfg: &MethodCfg, state: &PathRef) -> Res<Vec<PathRef>> {
        let mut next_state = state.clone();
        let mut prev_finally: Option<StateRef> = None;
        let mut prev_candidate: Option<StateRef> = None;
        loop {
            let advanced =
                self.advance(cfg, &next_state, &mut prev_finally, &mut prev_candidate)?;
            let advanced = match advanced {
                Some(a) if !a.is_empty() => a,
                _ => break,
            };
            if advanced.len() != 1 {
                let controller = Controller::new(self.abort.clone());
                let mut out = Vec::new();
                for a in &advanced {
                    out.extend(controller.process(cfg, a)?);
                }
                return Ok(out);
            }
            next_state = advanced[0].clone();
        }
        Ok(vec![next_state])
    }

    fn advance(
        &self,
        cfg: &MethodCfg,
        state: &PathRef,
        prev_finally: &mut Option<StateRef>,
        prev_candidate: &mut Option<StateRef>,
    ) -> Res<Option<Vec<PathRef>>> {
        let (finally_state, candidate_state, common) = {
            let p = state.borrow();
            (p.finally.clone(), p.candidate.clone(), p.common.clone())
        };
        let same = |prev: &Option<StateRef>, cur: &StateRef| {
            prev.as_ref().is_some_and(|p| Rc::ptr_eq(p, cur))
        };
        if same(prev_finally, &finally_state) && same(prev_candidate, &candidate_state) {
            return Ok(Some(vec![Path::produce(
                state,
                &Factory::Terminal,
                &Factory::Terminal,
            )]));
        }
        *prev_finally = Some(finally_state.clone());
        *prev_candidate = Some(candidate_state.clone());
        if finally_state.is_terminal() || candidate_state.is_terminal() {
            return Ok(None);
        }
        if std::mem::discriminant(&*finally_state) == std::mem::discriminant(&*candidate_state)
            && finally_state.compare_state() == Compare::ReadyToCompare
            && candidate_state.compare_state() == Compare::ReadyToCompare
        {
            let blocks = match (finally_state.block_info(), candidate_state.block_info()) {
                (Some(f), Some(c)) => Some((f.borrow().block, c.borrow().block)),
                _ => None,
            };
            let cached = blocks.and_then(|b| common.borrow().searched.get(&b).cloned());
            if let Some(dup_states) = cached {
                let mut recovered = Vec::with_capacity(dup_states.len());
                for dup in &dup_states {
                    let (rf, rc) = {
                        let d = dup.borrow();
                        (d.finally.clone(), d.candidate.clone())
                    };
                    let rec =
                        Path::produce(state, &Factory::Recovered(rf), &Factory::Recovered(rc));
                    merge_paths(&rec, &dup_states);
                    recovered.push(rec);
                }
                return Ok(Some(recovered));
            }
            let paths = match &*candidate_state {
                StateKind::AwaitingInsnCompare { .. } => handle_instructions(cfg, state)?,
                StateKind::IdentifiedScope { .. } => handle_merge_path(cfg, state)?,
                StateKind::UnknownAdvance { .. } => handle_predecessor_merge(cfg, state)?,
                _ => return Err("State without handler".into()),
            };
            return Ok(Some(paths));
        }
        let finally_aborted = self.advance_single_state(cfg, state, Side::Finally)?;
        let candidate_aborted = self.advance_single_state(cfg, state, Side::Candidate)?;
        if finally_aborted && candidate_aborted {
            return Ok(None);
        }
        Ok(Some(vec![state.clone()]))
    }

    /// Advances a singular state once; returns whether the state was aborted.
    fn advance_single_state(&self, cfg: &MethodCfg, path: &PathRef, side: Side) -> Res<bool> {
        let aborted = self.abort.as_ref().is_some_and(|a| a.apply(path, side));
        if !aborted {
            let state = path.borrow().state(side);
            if state.compare_state() == Compare::NotReady {
                match &*state {
                    StateKind::NewBlock { .. } => handle_new_block(cfg, path, side)?,
                    StateKind::NoBlock { .. } => handle_no_block(cfg, path, side)?,
                    _ => return Err("State without handler".into()),
                }
            }
        }
        Ok(aborted)
    }
}

#[cfg(test)]
mod tests {
    use super::all_permutations;

    #[test]
    fn permutation_order_follows_the_reference() {
        assert_eq!(
            all_permutations(&[1, 2, 3]),
            vec![
                vec![1, 2, 3],
                vec![1, 3, 2],
                vec![2, 3, 1],
                vec![2, 1, 3],
                vec![3, 1, 2],
                vec![3, 2, 1],
            ]
        );
        assert_eq!(all_permutations(&[7]), vec![vec![7]]);
    }
}
