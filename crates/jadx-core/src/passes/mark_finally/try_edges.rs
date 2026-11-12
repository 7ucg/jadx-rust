//! Try edges, execution scope groups and the per-edge block scopes of a try/catch
//! (`TryEdge`, `TryEdgeScopeGroupMap`, `TryCatchEdgeBlockMap` and the edge related parts of
//! `TryCatchBlockAttr`).
//!
//! The reference stores edges in hash maps keyed by objects with identity based hash codes, so the
//! iteration order is not reproducible; ordered vectors are used instead.

use crate::model::bitset::java_hash_map_order;
use crate::model::cfg::{bflag, MethodCfg, TryId};
use crate::model::insn::{BlockId, HandlerId};
use crate::passes::block_utils::*;
use crate::passes::exception_handler::handler_hash;

type Res<T> = Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TryEdgeType {
    TrueFallthrough,
    PrematureExit,
    LoopExit,
    Handler,
}

/// `TryEdge`: an exit out of a try body, the source block is inside the try body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TryEdge {
    pub source: BlockId,
    pub target: BlockId,
    pub ty: TryEdgeType,
    pub handler: Option<HandlerId>,
}

impl TryEdge {
    pub fn plain(source: BlockId, target: BlockId, ty: TryEdgeType) -> Self {
        TryEdge {
            source,
            target,
            ty,
            handler: None,
        }
    }

    pub fn for_handler(source: BlockId, target: BlockId, handler: HandlerId) -> Self {
        TryEdge {
            source,
            target,
            ty: TryEdgeType::Handler,
            handler: Some(handler),
        }
    }

    pub fn is_handler_exit(&self) -> bool {
        self.ty == TryEdgeType::Handler
    }

    pub fn is_not_handler_exit(&self) -> bool {
        !self.is_handler_exit()
    }

    /// Handler edge of `handler`.
    pub fn is_exit_of(&self, handler: HandlerId) -> bool {
        self.is_handler_exit() && self.handler == Some(handler)
    }
}

/// Insertion ordered map keyed by try edges.
#[derive(Debug, Clone)]
pub struct EdgeMap<V> {
    entries: Vec<(TryEdge, V)>,
}

impl<V> Default for EdgeMap<V> {
    fn default() -> Self {
        EdgeMap {
            entries: Vec::new(),
        }
    }
}

impl<V> EdgeMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, key: &TryEdge) -> Option<&V> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn put(&mut self, key: TryEdge, value: V) {
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    pub fn keys(&self) -> Vec<TryEdge> {
        self.entries.iter().map(|(k, _)| *k).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(TryEdge, V)> {
        self.entries.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut (TryEdge, V)> {
        self.entries.iter_mut()
    }
}

/// Identity hash code of the `TryEdgeType` constants, which are part of the hash code of an edge.
/// The reference has no stable value for it; the oracle runs it with a constant identity hash.
const ENUM_IDENTITY_HASH: i32 = 1;

/// `TryEdge.hashCode`: `Objects.hash(source, target, type, handler)`.
fn edge_hash(cfg: &MethodCfg, e: &TryEdge) -> i32 {
    let handler = e.handler.map_or(0, |h| handler_hash(cfg, h));
    [
        e.source as i32,
        e.target as i32,
        ENUM_IDENTITY_HASH,
        handler,
    ]
    .iter()
    .fold(1i32, |h, v| h.wrapping_mul(31).wrapping_add(*v))
}

/// Iteration order of a `java.util.HashMap` filled with `put` in the given order. `initial` is the
/// requested initial capacity (`None` for the default constructor).
fn java_map_order(hashes: &[i32], initial: Option<usize>) -> Vec<usize> {
    let mut cap = initial.map_or(16, |n| n.max(1).next_power_of_two());
    for size in 1..=hashes.len() {
        if size > cap * 3 / 4 {
            cap *= 2;
        }
    }
    let mut idx: Vec<usize> = (0..hashes.len()).collect();
    idx.sort_by_key(|i| {
        let h = hashes[*i] as u32;
        ((h ^ (h >> 16)) as usize) & (cap - 1)
    });
    idx
}

impl<V> EdgeMap<V> {
    /// Entries in the iteration order of a hash map created with `initial` capacity.
    fn into_hash_order(self, cfg: &MethodCfg, initial: Option<usize>) -> Self {
        let hashes: Vec<i32> = self
            .entries
            .iter()
            .map(|(k, _)| edge_hash(cfg, k))
            .collect();
        let mut slots: Vec<Option<(TryEdge, V)>> = self.entries.into_iter().map(Some).collect();
        let entries = java_map_order(&hashes, initial)
            .into_iter()
            .filter_map(|i| slots[i].take())
            .collect();
        EdgeMap { entries }
    }
}

// ---- TryCatchBlockAttr ------------------------------------------------------------------------

/// `TryCatchBlockAttr.isImplicitOrMerged`
pub fn is_implicit_or_merged(cfg: &MethodCfg, t: TryId) -> bool {
    cfg.try_blocks[t].merged || cfg.try_blocks[t].handlers.is_empty()
}

/// `TryCatchBlockAttr.getMergedHandlers`: handlers of the try and of its direct inner tries.
pub fn merged_handlers(cfg: &MethodCfg, t: TryId) -> Vec<HandlerId> {
    let tb = &cfg.try_blocks[t];
    let mut handlers = tb.handlers.clone();
    for inner in &tb.inner {
        handlers.extend(cfg.try_blocks[*inner].handlers.iter().copied());
    }
    handlers
}

/// `TryCatchBlockAttr.hashCode`
fn try_hash(cfg: &MethodCfg, t: TryId) -> i32 {
    let tb = &cfg.try_blocks[t];
    let mut hh: i32 = 1;
    for h in &tb.handlers {
        hh = hh.wrapping_mul(31).wrapping_add(handler_hash(cfg, *h));
    }
    let mut bh: i32 = 1;
    for b in &tb.blocks {
        bh = bh.wrapping_mul(31).wrapping_add(*b as i32);
    }
    hh.wrapping_add(bh.wrapping_mul(31))
}

/// `ExceptionHandler.getBottomSplitter`
pub fn bottom_splitter(cfg: &MethodCfg, handler: HandlerId) -> Res<Option<BlockId>> {
    let handler_try = cfg.handlers[handler]
        .try_block
        .ok_or("exception handler without try block")?;
    let inner = &cfg.try_blocks[handler_try].inner;
    if inner.len() > 1 {
        return Ok(None);
    }
    let search = inner.first().copied().unwrap_or(handler_try);
    let handler_block = cfg.handlers[handler]
        .handler_block
        .ok_or("exception handler without block")?;
    for &hp in &cfg.block(handler_block).preds {
        if !cfg.block(hp).contains(bflag::EXC_BOTTOM_SPLITTER) {
            continue;
        }
        for &sp in &cfg.block(hp).preds {
            if cfg.block(sp).try_block == Some(search) {
                return Ok(Some(hp));
            }
        }
    }
    Ok(None)
}

/// `TryCatchBlockAttr.getHandlerTryEdges`
pub fn handler_try_edges(cfg: &MethodCfg, t: TryId) -> Res<Vec<TryEdge>> {
    let mut edges = Vec::new();
    for handler in merged_handlers(cfg, t) {
        let handler_block = cfg.handlers[handler]
            .handler_block
            .ok_or("exception handler without block")?;
        let mut splitter = bottom_splitter(cfg, handler)?;
        if splitter.is_none() {
            // no bottom splitter: assume the top splitter of this try is the source of the exit
            let children: Vec<BlockId> = cfg
                .block(handler_block)
                .preds
                .iter()
                .copied()
                .filter(|b| cfg.try_blocks[t].blocks.contains(b))
                .collect();
            splitter = get_bottom_block(cfg, &children);
            if splitter.is_none() {
                splitter = cfg.try_blocks[t].top_splitter;
            }
        }
        let splitter = splitter.ok_or("no splitter found for handler edge")?;
        edges.push(TryEdge::for_handler(splitter, handler_block, handler));
    }
    Ok(edges)
}

/// `TryCatchBlockAttr.getFallthroughTryEdges`
pub fn fallthrough_try_edges(cfg: &MethodCfg, t: TryId) -> Res<Vec<TryEdge>> {
    let mut edges = Vec::new();
    let mut explored_blocks = Vec::new();
    let mut explored_trys = Vec::new();
    fallthrough_try_edges_into(cfg, t, &mut edges, &mut explored_blocks, &mut explored_trys)?;
    Ok(edges)
}

fn fallthrough_try_edges_into(
    cfg: &MethodCfg,
    t: TryId,
    edges: &mut Vec<TryEdge>,
    explored_blocks: &mut Vec<BlockId>,
    explored_trys: &mut Vec<TryId>,
) -> Res<()> {
    let merged = merged_handlers(cfg, t);
    // `new HashSet<>(getBlocks())` minus the blocks of every handler
    let mut search_blocks: Vec<BlockId> = Vec::new();
    for b in &cfg.try_blocks[t].blocks {
        if !search_blocks.contains(b) {
            search_blocks.push(*b);
        }
    }
    let initial = search_blocks.len();
    for h in &merged {
        for b in &cfg.handlers[*h].blocks {
            search_blocks.retain(|x| x != b);
        }
    }
    let ordered = java_hash_set_order(&search_blocks, initial);
    if let Some(source) = get_top_block(cfg, &ordered) {
        explored_trys.push(t);
        explore_try_path(
            cfg,
            t,
            edges,
            source,
            &search_blocks,
            explored_blocks,
            explored_trys,
        )?;
    }
    Ok(())
}

fn explore_try_path(
    cfg: &MethodCfg,
    t: TryId,
    edges: &mut Vec<TryEdge>,
    blk: BlockId,
    search_blocks: &[BlockId],
    explored_blocks: &mut Vec<BlockId>,
    explored_trys: &mut Vec<TryId>,
) -> Res<()> {
    for &successor in &cfg.block(blk).succs {
        if explored_blocks.contains(&successor) {
            continue;
        }
        // bottom splitters lead to handlers, only non-handler edges are of interest
        if cfg.block(successor).contains(bflag::EXC_BOTTOM_SPLITTER) {
            continue;
        }
        explored_blocks.push(successor);

        if cfg.block(successor).contains(bflag::LOOP_END) {
            let mut loop_start_blocks: Vec<BlockId> = Vec::new();
            for &l in &cfg.block(successor).loops {
                let li = &cfg.loop_infos[l];
                loop_start_blocks.push(li.start);
                for (src, target) in loop_exit_edges(cfg, l) {
                    if target == successor {
                        loop_start_blocks.push(src);
                    }
                }
            }
            if !loop_start_blocks
                .iter()
                .all(|b| explored_blocks.contains(b))
            {
                edges.push(TryEdge::plain(blk, successor, TryEdgeType::LoopExit));
                continue;
            }
        }

        let is_path_to_search = search_blocks
            .iter()
            .any(|sb| is_path_exists(cfg, successor, *sb));
        if !search_blocks.contains(&successor) && !is_path_to_search {
            // not part of this try: either an exit of the try or a block leading to an exit
            let mut all_with_current = cfg.try_blocks[t].blocks.clone();
            all_with_current.push(successor);
            let bottom = get_bottom_block(cfg, &all_with_current);
            if !(bottom.is_none() || bottom == Some(successor)) {
                // this block leads to an exit
                explore_try_path(
                    cfg,
                    t,
                    edges,
                    successor,
                    search_blocks,
                    explored_blocks,
                    explored_trys,
                )?;
                continue;
            }
            let empty_end = follow_empty_path(cfg, successor, false, false);
            if cfg.block(empty_end).contains(bflag::EXC_TOP_SPLITTER) {
                // an exit entering another try catch of the same scope: take over its edges
                let mut nested: Vec<TryId> = Vec::new();
                for &enter in &cfg.block(empty_end).succs {
                    if !cfg.block(enter).contains(bflag::TRY_ENTER) {
                        continue;
                    }
                    let Some(nt) = cfg.block(enter).try_block else {
                        continue;
                    };
                    if explored_trys.contains(&nt) {
                        continue;
                    }
                    if cfg.try_blocks[nt].top_splitter != cfg.try_blocks[t].top_splitter {
                        continue;
                    }
                    if !nested.contains(&nt) {
                        nested.push(nt);
                    }
                }
                if !nested.is_empty() {
                    let hashes: Vec<i32> = nested.iter().map(|n| try_hash(cfg, *n)).collect();
                    for i in java_hash_map_order(&hashes) {
                        fallthrough_try_edges_into(
                            cfg,
                            nested[i],
                            edges,
                            explored_blocks,
                            explored_trys,
                        )?;
                    }
                    continue;
                }
            }
            match bottom {
                None => edges.push(TryEdge::plain(blk, successor, TryEdgeType::PrematureExit)),
                Some(_) => edges.push(TryEdge::plain(blk, successor, TryEdgeType::TrueFallthrough)),
            }
        } else {
            explore_try_path(
                cfg,
                t,
                edges,
                successor,
                search_blocks,
                explored_blocks,
                explored_trys,
            )?;
        }
    }
    Ok(())
}

/// `LoopInfo.getExitEdges`: (source, target) pairs.
fn loop_exit_edges(cfg: &MethodCfg, l: usize) -> Vec<(BlockId, BlockId)> {
    let li = &cfg.loop_infos[l];
    let mut edges = Vec::new();
    for b in li.blocks.iter() {
        for &s in &cfg.block(b).succs {
            if !li.blocks.contains(s) && !cfg.is_exception_handler_path(s) {
                edges.push((b, s));
            }
        }
    }
    edges
}

/// `TryCatchBlockAttr.getTryEdges`
pub fn try_edges(cfg: &MethodCfg, t: TryId) -> Res<Vec<TryEdge>> {
    let mut edges = handler_try_edges(cfg, t)?;
    edges.extend(fallthrough_try_edges(cfg, t)?);
    Ok(edges)
}

/// `TryCatchBlockAttr.getEdgeBlockMap`
pub fn edge_block_map(cfg: &MethodCfg, t: TryId) -> Res<EdgeMap<BlockId>> {
    let mut map = EdgeMap::new();
    for e in try_edges(cfg, t)? {
        map.put(e, e.target);
    }
    Ok(map.into_hash_order(cfg, None))
}

// ---- TryEdgeScopeGroupMap -----------------------------------------------------------------------

/// Blocks where edge scopes end, with the edges ending there. The key is `None` when no common
/// block was found (a `null` key in the reference).
pub type ScopeGroups = Vec<(Option<BlockId>, Vec<TryEdge>)>;

/// `TryEdgeScopeGroupMap`
pub struct ScopeGroupMap {
    pub merged_edges: Vec<(TryEdge, TryEdge)>,
    try_block: TryId,
    map: EdgeMap<EdgeMap<Option<BlockId>>>,
}

/// `TryCatchBlockAttr.getExecutionScopeGroups`
pub fn execution_scope_groups(cfg: &MethodCfg, t: TryId) -> Res<ScopeGroupMap> {
    let handler_blocks = edge_block_map(cfg, t)?;
    let mut groups = ScopeGroupMap {
        merged_edges: Vec::new(),
        try_block: t,
        map: EdgeMap::new(),
    };
    groups.populate_from_edges(cfg, &handler_blocks);
    Ok(groups)
}

impl ScopeGroupMap {
    fn populate_from_edges(&mut self, cfg: &MethodCfg, edges: &EdgeMap<BlockId>) {
        self.merge_same_scopes(cfg, edges);
        let mut map = EdgeMap::new();
        for (edge, edge_block) in edges.iter() {
            let relations = self.create_edge_terminus_map(cfg, edges, *edge, *edge_block);
            map.put(*edge, relations);
        }
        self.map = map.into_hash_order(cfg, Some(edges.len()));
    }

    /// `getScopeEnds`
    pub fn scope_ends(&self, cfg: &MethodCfg) -> Res<ScopeGroups> {
        let mut groups: ScopeGroups = Vec::new();
        // edges without common clean successors with some other edge
        let mut isolated: Vec<TryEdge> = Vec::new();

        for (edge_a, relations) in self.map.iter() {
            if self.merged_node_from_edge(edge_a).is_some() {
                continue;
            }
            let mut scope_ends: Vec<BlockId> = Vec::new();
            for (edge_b, shared) in relations.iter() {
                if let Some((first, _)) = self.merged_node_from_edge(edge_b) {
                    if first == *edge_a {
                        continue;
                    }
                }
                match shared {
                    None => isolated.push(*edge_b),
                    Some(b) => scope_ends.push(*b),
                }
            }
            if scope_ends.is_empty() {
                continue;
            }
            let top = get_top_block(cfg, &scope_ends);
            group_add(&mut groups, top, *edge_a);
        }

        for isolated_edge in isolated {
            if groups.iter().any(|(_, l)| l.contains(&isolated_edge)) {
                // not isolated with another handler
                break;
            }
            // the end of an isolated edge is the farthest successor of its target without
            // clean successors
            let successors = collect_all_successors(cfg, isolated_edge.target, true);
            let end = get_bottom_block(cfg, &successors)
                .ok_or("Could not find bottom clean successor for isolated try edge")?;
            group_add(&mut groups, Some(end), isolated_edge);
        }

        if groups.len() == 1 {
            let handlers = &cfg.try_blocks[self.try_block].handlers;
            for &(kept, removed) in &self.merged_edges {
                if kept.is_handler_exit() && !handlers.contains(&kept.handler.unwrap_or(usize::MAX))
                {
                    continue;
                }
                if removed.is_handler_exit()
                    && !handlers.contains(&removed.handler.unwrap_or(usize::MAX))
                {
                    continue;
                }
                // both not handler exits: the paths merge at the phi starting the duplicated code
                if kept.is_not_handler_exit() && removed.is_not_handler_exit() {
                    continue;
                }
                for (_, list) in groups.iter_mut() {
                    if let Some(p) = list.iter().position(|e| *e == kept) {
                        list.remove(p);
                    }
                }
                let relations = self
                    .map
                    .get(&kept)
                    .ok_or("no scope relations for try edge")?;
                let terminus = relations.get(&removed).copied().flatten();
                match groups.iter_mut().find(|(k, _)| *k == terminus) {
                    Some((_, l)) => l.push(removed),
                    None => groups.push((terminus, vec![kept, removed])),
                }
            }
        }
        Ok(order_groups(groups))
    }

    /// The merged pair whose removed edge is `edge`.
    fn merged_node_from_edge(&self, edge: &TryEdge) -> Option<(TryEdge, TryEdge)> {
        self.merged_edges.iter().find(|(_, s)| s == edge).copied()
    }

    /// Map of all points where edges meet each other, dictating the end of that edge's scope.
    fn create_edge_terminus_map(
        &mut self,
        cfg: &MethodCfg,
        start_map: &EdgeMap<BlockId>,
        edge: TryEdge,
        edge_start: BlockId,
    ) -> EdgeMap<Option<BlockId>> {
        let mut relations: EdgeMap<Option<BlockId>> = EdgeMap::new();
        for (other_edge, other_start) in start_map.iter() {
            let (other_edge, other_start) = (*other_edge, *other_start);
            if edge == other_edge {
                continue;
            }
            let either_handler = edge.is_handler_exit() || other_edge.is_handler_exit();
            if other_start == edge_start && either_handler {
                continue;
            }
            if cfg.block(other_start).contains(bflag::MTH_EXIT_BLOCK) {
                relations.put(other_edge, Some(other_start));
                // everything leads to the exit node so merged edges are no longer needed
                self.merged_edges.clear();
                continue;
            }
            if cfg.block(edge_start).contains(bflag::MTH_EXIT_BLOCK) {
                relations.put(other_edge, Some(edge_start));
                self.merged_edges.clear();
                continue;
            }
            let post_a_empty = cfg
                .block(edge_start)
                .post_doms
                .as_ref()
                .is_none_or(|b| b.is_empty());
            let post_b_empty = cfg
                .block(other_start)
                .post_doms
                .as_ref()
                .is_none_or(|b| b.is_empty());
            if post_a_empty || post_b_empty {
                continue;
            }
            let mut chain: Vec<BlockId> = Vec::new();
            let mut cur = Some(edge_start);
            while let Some(b) = cur {
                chain.push(b);
                cur = cfg.block(b).ipost_dom;
            }
            let mut common = None;
            let mut cur = Some(other_start);
            while let Some(b) = cur {
                if chain.contains(&b) {
                    common = Some(b);
                    break;
                }
                cur = cfg.block(b).ipost_dom;
            }
            relations.put(other_edge, common);
        }
        relations.into_hash_order(cfg, Some(start_map.len().saturating_sub(1)))
    }

    /// Records scopes that merge: one edge leads to the same execution point as the target of
    /// another edge.
    fn merge_same_scopes(&mut self, cfg: &MethodCfg, handlers: &EdgeMap<BlockId>) {
        let entries: Vec<(TryEdge, BlockId)> = handlers.iter().map(|(k, v)| (*k, *v)).collect();
        let mut pairs: Vec<((TryEdge, BlockId), (TryEdge, BlockId))> = Vec::new();
        for i in 0..entries.len() {
            for j in (i + 1)..entries.len() {
                pairs.push((entries[i], entries[j]));
            }
        }
        let mut i = 0;
        while i < pairs.len() {
            let ((edge_a, block_a), (edge_b, block_b)) = pairs[i];
            let path_exists =
                is_path_exists(cfg, block_a, block_b) || is_path_exists(cfg, block_b, block_a);
            if path_exists {
                let bottom = get_bottom_block(cfg, &[block_a, block_b]);
                let remove = if Some(block_a) != bottom {
                    edge_a
                } else {
                    edge_b
                };
                let keep = if Some(block_a) == bottom {
                    edge_a
                } else {
                    edge_b
                };
                pairs.remove(i);
                self.merged_edges.push((keep, remove));
            } else {
                i += 1;
            }
        }
    }
}

fn group_add(groups: &mut ScopeGroups, key: Option<BlockId>, edge: TryEdge) {
    match groups.iter_mut().find(|(k, _)| *k == key) {
        Some((_, l)) => l.push(edge),
        None => groups.push((key, vec![edge])),
    }
}

/// Order of a `HashMap<BlockNode, ..>` iteration (hash code = block id, `null` key in bucket 0).
fn order_groups(groups: ScopeGroups) -> ScopeGroups {
    let hashes: Vec<i32> = groups
        .iter()
        .map(|(k, _)| k.map_or(0, |b| b as i32))
        .collect();
    let order = java_hash_map_order(&hashes);
    let mut slots: Vec<Option<(Option<BlockId>, Vec<TryEdge>)>> =
        groups.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

// ---- TryCatchEdgeBlockMap ----------------------------------------------------------------------

/// `TryCatchEdgeBlockMap`: every edge of a try with the blocks reachable within the scope of that
/// edge.
pub type EdgeBlockMap = EdgeMap<Vec<BlockId>>;

/// `TryCatchEdgeBlockMap.anyBlockHasNonImplicitTry`
fn any_block_has_non_implicit_try(cfg: &MethodCfg, blocks: &[BlockId]) -> bool {
    for &top_splitter in blocks {
        if !cfg.block(top_splitter).contains(bflag::EXC_TOP_SPLITTER) {
            continue;
        }
        let mut found = None;
        for &s in cfg
            .block(top_splitter)
            .clean_succs()
            .map_or(&[][..], |c| c.as_slice())
        {
            if let Some(t) = cfg.block(s).try_block {
                found = Some(t);
            }
        }
        let Some(t) = found else {
            continue;
        };
        if !is_implicit_or_merged(cfg, t) {
            return true;
        }
    }
    false
}

/// `TryCatchEdgeBlockMap.getAllInScope`
pub fn all_in_scope(
    cfg: &MethodCfg,
    t: TryId,
    scope_groups: &ScopeGroupMap,
    finally_handler: HandlerId,
    terminus_groups: &ScopeGroups,
) -> Res<EdgeBlockMap> {
    let edge_blocks = edge_block_map(cfg, t)?;
    let mut result: EdgeBlockMap = EdgeMap::new();
    for (terminus, source_edges) in terminus_groups {
        for source_edge in source_edges {
            let edge_block = *edge_blocks
                .get(source_edge)
                .ok_or("try edge without block")?;
            let use_clean = !(source_edge.is_not_handler_exit()
                && scope_groups
                    .merged_edges
                    .iter()
                    .any(|(_, second)| second.is_not_handler_exit()));
            let stop = |b: BlockId| Some(b) == *terminus;
            let mut all_blocks = collect_all_successors_until(cfg, edge_block, use_clean, &stop);
            if use_clean && any_block_has_non_implicit_try(cfg, &all_blocks) {
                // a try edge among the blocks: collect all successors, not just the clean ones
                all_blocks = collect_all_successors_until(cfg, edge_block, false, &stop);
            }
            if source_edge.is_not_handler_exit() {
                // fallthrough case: add the try body
                all_blocks.extend(cfg.try_blocks[t].blocks.iter().copied());
            }
            result.put(*source_edge, all_blocks);
        }
    }
    if let Some(finally_blocks) = blocks_for_handler(&result, finally_handler).cloned() {
        for (edge, blocks) in result.iter_mut() {
            if edge.is_exit_of(finally_handler) {
                continue;
            }
            blocks.retain(|b| !finally_blocks.contains(b));
        }
    }
    Ok(result)
}

/// `TryCatchEdgeBlockMap.getBlocksForHandler`
pub fn blocks_for_handler(map: &EdgeBlockMap, handler: HandlerId) -> Option<&Vec<BlockId>> {
    map.iter()
        .find(|(e, _)| e.is_exit_of(handler))
        .map(|(_, v)| v)
}

/// Mutable variant of [`blocks_for_handler`].
pub fn blocks_for_handler_mut(
    map: &mut EdgeBlockMap,
    handler: HandlerId,
) -> Option<&mut Vec<BlockId>> {
    map.iter_mut()
        .find(|(e, _)| e.is_exit_of(handler))
        .map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::java_map_order;

    #[test]
    fn hash_order_is_by_bucket_then_insertion() {
        assert_eq!(java_map_order(&[2, 1], None), vec![1, 0]);
        // same bucket (1, 17, 33 with 16 buckets): insertion order is kept
        assert_eq!(java_map_order(&[17, 1, 33], None), vec![0, 1, 2]);
    }

    #[test]
    fn hash_order_grows_the_table() {
        // 13 entries exceed the load factor of the default table: 17 moves from bucket 1 to 17
        let hashes: Vec<i32> = (0..13).map(|i| if i == 0 { 17 } else { i }).collect();
        let order = java_map_order(&hashes, None);
        assert_eq!(order.last(), Some(&0));
        // an explicit capacity is rounded up to a power of two
        assert_eq!(java_map_order(&[5, 4, 3], Some(3)), vec![1, 0, 2]);
    }
}
