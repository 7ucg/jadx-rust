//! `MarkFinallyVisitor`: extract finally blocks from the code duplicated at the end of every exit
//! of a try body.
//!
//! The scope of every exit of a try (its handlers and its fallthrough paths) is determined first.
//! The catch-all handler is the candidate for the finally code; the blocks of every other scope are
//! compared against it, starting at the end of each scope (see [`traverser`]). Instructions that
//! are found in all scopes are marked as duplicates (`DONT_GENERATE`) and the finally code itself
//! is marked with `FINALLY_INSNS`; the code that rethrows the caught exception is hidden.
//!
//! Debug information of registers (`RegDebugInfoAttr`) is not attached in this pipeline, so it
//! never counts as a reason to treat two registers as the same variable.

mod traverser;
mod try_edges;

use std::collections::HashSet;

use crate::model::cfg::{bflag, MethodCfg, TryId};
use crate::model::insn::{iflag, BlockId, HandlerId, InsnArg, InsnId, InsnType};
use crate::passes::block_utils::*;
use crate::passes::exception_handler::{clear_exception_handlers, mark_for_remove};

use self::traverser::{Controller, Path};
use self::try_edges::{
    all_in_scope, blocks_for_handler, blocks_for_handler_mut, execution_scope_groups,
    is_implicit_or_merged, EdgeBlockMap, ScopeGroupMap, ScopeGroups,
};

type Res<T> = Result<T, String>;

/// `TryExtractInfo`
struct TryExtractInfo {
    try_block: TryId,
    scope_groups: ScopeGroupMap,
    finally_handler: HandlerId,
    terminus_groups: ScopeGroups,
    handler_scopes: EdgeBlockMap,
    rethrow_blocks: Vec<BlockId>,
    complete_finally_blocks: Option<HashSet<BlockId>>,
    complete_candidate_blocks: Option<HashSet<BlockId>>,
}

/// Search and mark the code duplicated for finally blocks of `cfg`.
///
/// On an internal error the method is left as it was before the pass ran.
pub fn mark_finally(cfg: &mut MethodCfg) {
    if cfg.handler_list.is_empty() || !has_catch_all_try(cfg) {
        return;
    }
    let snapshot = cfg.clone();
    if let Err(e) = run(cfg) {
        *cfg = snapshot;
        cfg.warnings
            .push(format!("Undo finally extract visitor: {e}"));
    }
}

/// Whether some try block has a catch-all handler, the others are not touched by the pass.
fn has_catch_all_try(cfg: &MethodCfg) -> bool {
    cfg.try_list.iter().any(|t| {
        let tb = &cfg.try_blocks[*t];
        !tb.merged && tb.handlers.iter().any(|h| cfg.handlers[*h].is_catch_all())
    })
}

fn run(cfg: &mut MethodCfg) -> Res<()> {
    let try_blocks: Vec<TryId> = cfg.try_list.clone();
    let mut process_required: Vec<TryId> = Vec::new();
    let mut implicit_handler_removed = false;

    // remove implicit handlers, mark the others to be searched for a finally block
    for &t in &try_blocks {
        let Some(mut info) = try_block_data(cfg, t)? else {
            continue;
        };
        let handler = info.finally_handler;
        let Some(cut) = cut_handler_blocks(cfg, &mut info, handler)? else {
            continue;
        };
        if attempt_remove_implicit_handlers(cfg, &info, &cut)? {
            implicit_handler_removed = true;
        } else {
            process_required.push(t);
        }
    }
    if implicit_handler_removed {
        reset_try_blocks(cfg, &try_blocks);
    }

    let mut finally_extracted = false;
    for &t in &process_required {
        // refresh the scope groups now that implicit handlers are gone
        let Some(mut info) = try_block_data(cfg, t)? else {
            continue;
        };
        let handler = info.finally_handler;
        cut_handler_blocks(cfg, &mut info, handler)?;
        finally_extracted |= process_try_block(cfg, &mut info)?;
    }
    if finally_extracted {
        reset_try_blocks(cfg, &try_blocks);
    }
    Ok(())
}

fn reset_try_blocks(cfg: &mut MethodCfg, try_blocks: &[TryId]) {
    clear_exception_handlers(cfg);
    // remove merged or empty try blocks from the list of the method
    let cleared: Vec<TryId> = try_blocks
        .iter()
        .copied()
        .filter(|t| !is_implicit_or_merged(cfg, *t))
        .collect();
    if cleared.len() != try_blocks.len() {
        cfg.try_list = cleared;
    }
}

/// Handlers of the try block, scope groups of its exits and the blocks within each scope.
fn try_block_data(cfg: &MethodCfg, t: TryId) -> Res<Option<TryExtractInfo>> {
    if cfg.try_blocks[t].merged {
        return Ok(None);
    }
    let Some(all_handler) = cfg.try_blocks[t]
        .handlers
        .iter()
        .copied()
        .find(|h| cfg.handlers[*h].is_catch_all())
    else {
        return Ok(None);
    };
    let scope_groups = execution_scope_groups(cfg, t)?;
    let terminus_groups = scope_groups.scope_ends(cfg)?;
    let handler_scopes = all_in_scope(cfg, t, &scope_groups, all_handler, &terminus_groups)?;
    Ok(Some(TryExtractInfo {
        try_block: t,
        scope_groups,
        finally_handler: all_handler,
        terminus_groups,
        handler_scopes,
        rethrow_blocks: Vec::new(),
        complete_finally_blocks: None,
        complete_candidate_blocks: None,
    }))
}

fn process_try_block(cfg: &mut MethodCfg, info: &mut TryExtractInfo) -> Res<bool> {
    if info.rethrow_blocks.is_empty() {
        return Ok(false);
    }
    if extract_finally(cfg, info)? {
        for &b in &info.rethrow_blocks {
            if let Some(last) = get_last_insn(cfg, b) {
                cfg.insns[last].flags |= iflag::DONT_GENERATE;
            }
        }
        return Ok(true);
    }
    Ok(false)
}

/// Blocks of the handler without its trailing rethrow of the caught exception. The rethrow blocks
/// are collected in `info`.
fn cut_handler_blocks(
    cfg: &mut MethodCfg,
    info: &mut TryExtractInfo,
    handler: HandlerId,
) -> Res<Option<Vec<BlockId>>> {
    let handler_block = cfg.handlers[handler]
        .handler_block
        .ok_or("exception handler without block")?;
    let handler_final_insn = get_first_insn(cfg, handler_block);
    let Some(scope_blocks) = blocks_for_handler_mut(&mut info.handler_scopes, handler) else {
        return Ok(None);
    };
    if handler_final_insn.is_some_and(|i| cfg.insns[i].ty == InsnType::MoveException) {
        // exclude the block with 'move-exception'
        if let Some(p) = scope_blocks.iter().position(|b| *b == handler_block) {
            scope_blocks.remove(p);
        }
    }
    let handler_blocks = scope_blocks.clone();

    let Some(bottom_block) = get_bottom_block(cfg, &handler_blocks) else {
        cfg.warnings
            .push(format!("Bottom block not found for handler: {handler}"));
        return Ok(Some(handler_blocks));
    };
    let path_exits = follow_empty_up_path_within_set(cfg, bottom_block, &handler_blocks);
    if path_exits.is_empty() {
        return Ok(Some(handler_blocks));
    }
    for &exit in &path_exits {
        // all paths into the handler's logic must end with a throw of the exception caught by
        // the 'move-exception' at the start of the handler
        let valid = match (get_last_insn(cfg, exit), handler_final_insn) {
            (Some(last), Some(first)) => {
                let last = cfg.insn(last);
                last.ty == InsnType::Throw
                    && match (last.args.first(), &cfg.insn(first).result) {
                        (Some(InsnArg::Register(a)), Some(r)) => {
                            traverser::reg_key(cfg, a) == traverser::reg_key(cfg, r)
                        }
                        _ => false,
                    }
            }
            _ => false,
        };
        if !valid {
            return Ok(Some(handler_blocks));
        }
    }
    let mut cut = handler_blocks;
    for &exit in &path_exits {
        if let Some(p) = cut.iter().position(|b| *b == exit) {
            cut.remove(p);
        }
        remove_empty_up_path(cfg, &mut cut, exit);
        if !info.rethrow_blocks.contains(&exit) {
            info.rethrow_blocks.push(exit);
        }
    }
    Ok(Some(cut))
}

fn remove_empty_up_path(cfg: &MethodCfg, handler_blocks: &mut Vec<BlockId>, start: BlockId) {
    for &pred in &cfg.block(start).preds {
        if cfg.block(pred).insns.is_empty() {
            let removed = match handler_blocks.iter().position(|b| *b == pred) {
                Some(p) => {
                    handler_blocks.remove(p);
                    true
                }
                None => false,
            };
            if removed && !is_back_edge(cfg, pred, start) {
                remove_empty_up_path(cfg, handler_blocks, pred);
            }
        }
    }
}

/// Removes the catch-all handler if it has no code (an implicit try catch).
fn attempt_remove_implicit_handlers(
    cfg: &mut MethodCfg,
    info: &TryExtractInfo,
    cut_handler_blocks: &[BlockId],
) -> Res<bool> {
    if !(cut_handler_blocks.is_empty() || is_all_blocks_empty(cfg, cut_handler_blocks)) {
        return Ok(false);
    }
    // remove empty catch
    let handler = info.finally_handler;
    let t = cfg.handlers[handler]
        .try_block
        .ok_or("exception handler without try block")?;
    if let Some(p) = cfg.try_blocks[t]
        .handlers
        .iter()
        .position(|h| *h == handler)
    {
        cfg.try_blocks[t].handlers.remove(p);
    }
    mark_for_remove(cfg, handler);
    Ok(true)
}

/// Search and mark common code of the 'try' block and its 'handlers'.
fn extract_finally(cfg: &mut MethodCfg, info: &mut TryExtractInfo) -> Res<bool> {
    let t = info.try_block;
    let has_inner_blocks = !cfg.try_blocks[t].inner.is_empty();
    if !has_handlers_for_try_catch(cfg, t) {
        return Ok(false);
    }
    let Some(insns) = find_common_insns(cfg, info)? else {
        return Ok(false);
    };
    if insns.is_empty() {
        return Ok(false);
    }
    let ignored_finally_insns: HashSet<InsnId> = HashSet::new();
    let ignored_candidate_insns: HashSet<InsnId> = HashSet::new();
    for (_, candidates) in &insns {
        // an instruction has matched if it was found as often as the try has edges which are not
        // the finally handler
        if candidates.len() + 1 != info.handler_scopes.len() {
            // partial `catch (Throwable)` finally clauses are not supported
            return Ok(false);
        }
    }
    for (finally_insn, candidates) in &insns {
        cfg.insns[*finally_insn].flags |= iflag::FINALLY_INSNS;
        for &candidate in candidates {
            copy_code_vars(cfg, *finally_insn, candidate)?;
            cfg.insns[candidate].flags |= iflag::DONT_GENERATE;
        }
    }
    let finally_blocks: Vec<BlockId> = info
        .complete_finally_blocks
        .as_ref()
        .ok_or("no finally blocks")?
        .iter()
        .copied()
        .collect();
    for block in finally_blocks {
        if cfg
            .block(block)
            .insns
            .iter()
            .any(|i| ignored_finally_insns.contains(i))
        {
            // contains an instruction which was not found in all try edges
            continue;
        }
        cfg.block_mut(block).add(bflag::FINALLY_INSNS);
    }
    let candidate_blocks: Vec<BlockId> = info
        .complete_candidate_blocks
        .as_ref()
        .ok_or("no candidate blocks")?
        .iter()
        .copied()
        .collect();
    for block in candidate_blocks {
        if cfg
            .block(block)
            .insns
            .iter()
            .any(|i| ignored_candidate_insns.contains(i))
        {
            continue;
        }
        cfg.block_mut(block).add(bflag::DONT_GENERATE);
    }

    // If any scope has been merged with the fallthrough case of the try catch, don't merge inner
    // trys. Otherwise, merge inner trys.
    let merged_fallthrough_scope = info
        .scope_groups
        .merged_edges
        .iter()
        .any(|(first, _)| first.is_not_handler_exit());
    let merge_inner_try_blocks = has_inner_blocks && !merged_fallthrough_scope;

    cfg.handlers[info.finally_handler].is_finally = true;
    if merge_inner_try_blocks {
        let inner_tries = cfg.try_blocks[t].inner.clone();
        for inner in inner_tries {
            let handlers = cfg.try_blocks[inner].handlers.clone();
            let blocks = cfg.try_blocks[inner].blocks.clone();
            cfg.try_blocks[t].handlers.extend(handlers);
            cfg.try_blocks[t].blocks.extend(blocks);
            cfg.try_blocks[inner].merged = true;
        }
        let mut distinct: Vec<BlockId> = Vec::new();
        for b in std::mem::take(&mut cfg.try_blocks[t].blocks) {
            if !distinct.contains(&b) {
                distinct.push(b);
            }
        }
        cfg.try_blocks[t].blocks = distinct;
        cfg.try_blocks[t].inner.clear();
    }
    Ok(true)
}

/// Whether the try block or one of its inner try blocks has an exception handler.
fn has_handlers_for_try_catch(cfg: &MethodCfg, t: TryId) -> bool {
    let tb = &cfg.try_blocks[t];
    !tb.handlers.is_empty() || tb.inner.iter().any(|i| has_handlers_for_try_catch(cfg, *i))
}

/// Matching instructions: every instruction of the finally code with the instructions of the
/// other scopes that duplicate it.
type CommonInsns = Vec<(InsnId, Vec<InsnId>)>;

fn find_common_insns(cfg: &MethodCfg, info: &mut TryExtractInfo) -> Res<Option<CommonInsns>> {
    let finally_handler = info.finally_handler;
    let all_handler_blocks = blocks_for_handler(&info.handler_scopes, finally_handler)
        .ok_or("no blocks for the finally handler")?
        .clone();
    let Some(finally_terminus) = terminus_for_handler(info, finally_handler) else {
        return Ok(None);
    };
    let mut matching: CommonInsns = Vec::new();
    let mut index: std::collections::HashMap<InsnId, usize> = std::collections::HashMap::new();
    let edges = info.handler_scopes.keys();
    for edge in edges {
        if edge.is_exit_of(finally_handler) {
            continue;
        }
        let handler_blocks = info
            .handler_scopes
            .get(&edge)
            .ok_or("edge without blocks")?
            .clone();
        let scope_terminus = info
            .terminus_groups
            .iter()
            .find(|(_, edges)| edges.contains(&edge))
            .and_then(|(terminus, _)| *terminus)
            .ok_or_else(|| format!("Expected to find fallthrough terminus for handler {edge:?}"))?;
        let state = Path::new(
            cfg,
            finally_terminus,
            scope_terminus,
            &all_handler_blocks,
            &handler_blocks,
        );
        let path_results = Controller::new_plain().process(cfg, &state)?;
        let mut complete_finally: HashSet<BlockId> = HashSet::new();
        let mut complete_candidate: HashSet<BlockId> = HashSet::new();
        for result in &path_results {
            let result = result.borrow();
            for (finally_insn, candidate_insn) in result.matched_insns() {
                let slot = *index.entry(finally_insn).or_insert_with(|| {
                    matching.push((finally_insn, Vec::new()));
                    matching.len() - 1
                });
                matching[slot].1.push(candidate_insn);
            }
            complete_finally.extend(result.fully_matched_finally_blocks());
            complete_candidate.extend(result.fully_matched_candidate_blocks());
        }
        match &mut info.complete_finally_blocks {
            None => info.complete_finally_blocks = Some(complete_finally),
            Some(existing) => existing.retain(|b| complete_finally.contains(b)),
        }
        match &mut info.complete_candidate_blocks {
            None => info.complete_candidate_blocks = Some(complete_candidate),
            Some(existing) => existing.extend(complete_candidate),
        }
    }
    Ok(Some(matching))
}

/// The scope terminus of the handler edge of `handler`.
fn terminus_for_handler(info: &TryExtractInfo, handler: HandlerId) -> Option<BlockId> {
    for (terminus, edges) in &info.terminus_groups {
        for edge in edges {
            if edge.is_exit_of(handler) {
                return *terminus;
            }
        }
    }
    None
}

fn copy_code_vars(cfg: &mut MethodCfg, from: InsnId, to: InsnId) -> Res<()> {
    let from_result = cfg.insn(from).result.as_ref().map(|r| r.svar);
    let to_result = cfg.insn(to).result.as_ref().map(|r| r.svar);
    if let (Some(f), Some(t)) = (from_result, to_result) {
        copy_code_var(cfg, f, t)?;
    }
    for i in 0..cfg.insn(from).args.len() {
        let f = match &cfg.insn(from).args[i] {
            InsnArg::Register(r) => Some(r.svar),
            _ => None,
        };
        let t = match cfg.insn(to).args.get(i).ok_or("Index out of bounds")? {
            InsnArg::Register(r) => Some(r.svar),
            _ => None,
        };
        if let (Some(f), Some(t)) = (f, t) {
            copy_code_var(cfg, f, t)?;
        }
    }
    Ok(())
}

fn copy_code_var(
    cfg: &mut MethodCfg,
    from: Option<crate::model::ssa::SsaId>,
    to: Option<crate::model::ssa::SsaId>,
) -> Res<()> {
    let from = from.ok_or("register without SSA variable")?;
    let to = to.ok_or("register without SSA variable")?;
    let code_var = cfg.svars[from]
        .code_var
        .ok_or_else(|| "Code variable not set".to_string())?;
    cfg.svar_set_code_var(to, code_var);
    Ok(())
}
