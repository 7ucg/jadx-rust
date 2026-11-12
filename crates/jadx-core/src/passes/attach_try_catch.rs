//! `AttachTryCatchVisitor`: attach try/catch info to instructions.

use jadx_input_api::TryData;

use crate::model::cfg::ExceptionHandler;
use crate::model::info::ClassInfo;
use crate::model::insn::{iflag, HandlerId, InsnNode, InsnType};
use crate::root::RootCtx;

fn insert_nop(insns: &mut [Option<InsnNode>], offset: usize) {
    let mut nop = InsnNode::plain(InsnType::Nop);
    nop.offset = offset as i32;
    nop.flags |= iflag::SYNTHETIC;
    insns[offset] = Some(nop);
}

fn next_insn_offset(insns: &[Option<InsnNode>], offset: usize) -> Option<usize> {
    ((offset + 1)..insns.len()).find(|&i| insns[i].is_some())
}

fn build_catch(handlers: &mut [HandlerId], all: &[ExceptionHandler]) -> Vec<HandlerId> {
    let mut v = handlers.to_vec();
    v.sort_by_key(|h| all[*h].handler_offset); // stable
    v
}

fn create_handler(
    handlers: &mut Vec<ExceptionHandler>,
    insns: &mut [Option<InsnNode>],
    handler_offset: usize,
    ty: Option<ClassInfo>,
) -> Option<HandlerId> {
    if handler_offset >= insns.len() {
        return None;
    }
    match insns.get(handler_offset).and_then(|i| i.as_ref()) {
        Some(insn) => {
            if let Some(h) = insn.exc_handler {
                if handlers[h].add_catch_type(ty) {
                    return None;
                }
                return Some(h);
            }
        }
        None => insert_nop(insns, handler_offset),
    }
    let id = handlers.len();
    handlers.push(ExceptionHandler::build(handler_offset as i32, ty));
    insns[handler_offset].as_mut()?.exc_handler = Some(id);
    Some(id)
}

/// Returns the handlers created for the method (empty when there are no try blocks).
pub fn attach_try_catch(
    insns: &mut [Option<InsnNode>],
    tries: &[TryData],
    root: &RootCtx,
) -> Result<Vec<ExceptionHandler>, String> {
    let mut handlers: Vec<ExceptionHandler> = Vec::new();
    for t in tries {
        let mut list: Vec<HandlerId> = Vec::new();
        for (i, off) in t.catch.handlers.iter().enumerate() {
            let ty = ArgTypeParse::class(root, &t.catch.types[i])?;
            if let Some(h) = create_handler(&mut handlers, insns, *off as usize, Some(ty)) {
                list.push(h);
            }
        }
        if t.catch.catch_all >= 0 {
            if let Some(h) = create_handler(&mut handlers, insns, t.catch.catch_all as usize, None)
            {
                list.push(h);
            }
        }
        if list.is_empty() {
            continue;
        }
        let catch = build_catch(&mut list, &handlers);
        mark_try_bounds(insns, t, &catch);
    }
    Ok(handlers)
}

struct ArgTypeParse;
impl ArgTypeParse {
    fn class(root: &RootCtx, name: &str) -> Result<ClassInfo, String> {
        root.class_info(&crate::model::types::ArgType::object(name))
    }
}

fn attach_catch(catch: &[HandlerId], insn: &mut InsnNode) {
    match &insn.catch {
        Some(exist) => {
            let mut merged = exist.clone();
            merged.extend_from_slice(catch);
            // CatchAttr.build sorts by handler offset; ids are created in offset discovery order,
            // so sort is applied by the caller on the final list (see sort_catch_attrs)
            insn.catch = Some(merged);
        }
        None => insn.catch = Some(catch.to_vec()),
    }
}

fn mark_try_bounds(insns: &mut [Option<InsnNode>], t: &TryData, catch: &[HandlerId]) {
    let mut offset = t.start as usize;
    let end = t.end as usize;
    let mut started = false;
    let mut last: Option<usize> = None;
    while offset <= end && offset < insns.len() {
        if let Some(insn) = insns[offset].as_mut() {
            last = Some(offset);
            attach_catch(catch, insn);
            if !started {
                insn.flags |= iflag::TRY_ENTER;
                started = true;
            }
        }
        match next_insn_offset(insns, offset) {
            Some(n) => offset = n,
            None => break,
        }
    }
    if started {
        if let Some(l) = last {
            if let Some(i) = insns[l].as_mut() {
                i.flags |= iflag::TRY_LEAVE;
            }
        }
    } else {
        let start = t.start as usize;
        if start < insns.len() {
            insert_nop(insns, start);
            if let Some(nop) = insns[start].as_mut() {
                nop.flags |= iflag::TRY_ENTER | iflag::TRY_LEAVE;
                nop.catch = Some(catch.to_vec());
            }
        }
    }
}

/// `CatchAttr.build` sorts merged handler lists by offset; apply to all instructions.
pub fn sort_catch_attrs(insns: &mut [Option<InsnNode>], handlers: &[ExceptionHandler]) {
    for insn in insns.iter_mut().flatten() {
        if let Some(c) = insn.catch.as_mut() {
            c.sort_by_key(|h| handlers[*h].handler_offset);
        }
    }
}
