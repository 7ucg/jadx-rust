//! `ProcessInstructionsVisitor`: attach switch / fill-array payloads, merge `move-result`
//! into the preceding invoke and record jump targets.

use crate::model::insn::{InsnKind, InsnNode, InsnType, JumpInfo, RegisterArg};
use crate::model::types::ArgType;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ProcessError(pub String);

fn format_offset(off: i32) -> String {
    if off < 0 {
        "?".to_string()
    } else {
        format!("0x{off:04x}")
    }
}

/// Next non-NOP instruction after `offset`, `None` if there is none.
pub fn next_insn_offset(insns: &[Option<InsnNode>], offset: usize) -> Option<usize> {
    ((offset + 1)..insns.len()).find(|&i| matches!(&insns[i], Some(n) if n.ty != InsnType::Nop))
}

fn insn_offset_at(insns: &[Option<InsnNode>], offset: usize) -> Option<usize> {
    (offset..insns.len()).find(|&i| matches!(&insns[i], Some(n) if n.ty != InsnType::Nop))
}

fn add_jump(insns: &mut [Option<InsnNode>], src: i32, dest: i32, errors: &mut Vec<String>) {
    match usize::try_from(dest)
        .ok()
        .and_then(|d| insns.get_mut(d))
        .and_then(|s| s.as_mut())
    {
        Some(n) => n.jumps.push(JumpInfo { src, dest }),
        None => errors.push(format!(
            "Failed to set jump: {} -> {}",
            format_offset(src),
            format_offset(dest)
        )),
    }
}

/// Run the pass over an instruction array indexed by offset. Returns non fatal errors.
pub fn process_instructions(insns: &mut [Option<InsnNode>]) -> Result<Vec<String>, ProcessError> {
    let mut errors = Vec::new();
    for offset in 0..insns.len() {
        let Some(insn) = insns[offset].as_ref() else {
            continue;
        };
        let off = offset as i32;
        match insn.ty {
            InsnType::Switch => {
                let (needs_data, data_target) = match &insn.kind {
                    InsnKind::Switch {
                        data, data_target, ..
                    } => (data.is_none(), *data_target),
                    _ => continue,
                };
                if needs_data {
                    attach_switch_data(insns, offset, data_target)?;
                }
                let (def, targets) = match &insns[offset].as_ref().map(|n| &n.kind) {
                    Some(InsnKind::Switch {
                        def, data: Some(d), ..
                    }) => (*def, d.targets.clone()),
                    _ => continue,
                };
                if def != -1 {
                    add_jump(insns, off, def, &mut errors);
                }
                for t in targets {
                    add_jump(insns, off, t, &mut errors);
                }
            }
            InsnType::If => {
                let target = match &insn.kind {
                    InsnKind::If { target, .. } => *target,
                    _ => continue,
                };
                if let Some(next) = next_insn_offset(insns, offset) {
                    add_jump(insns, off, next as i32, &mut errors);
                }
                add_jump(insns, off, target, &mut errors);
            }
            InsnType::Goto => {
                let target = match &insn.kind {
                    InsnKind::Goto { target } => *target,
                    _ => continue,
                };
                add_jump(insns, off, target, &mut errors);
            }
            InsnType::JavaJsr => {
                let target = match &insn.kind {
                    InsnKind::Goto { target } => *target,
                    _ => continue,
                };
                add_jump(insns, off, target, &mut errors);
                if let Some(on_ret) = next_insn_offset(insns, offset) {
                    add_jump(insns, off, on_ret as i32, &mut errors);
                }
            }
            InsnType::Invoke => {
                if insn.result.is_none() {
                    let ret = match &insn.kind {
                        InsnKind::Invoke { mth, .. } => mth.return_type.clone(),
                        _ => continue,
                    };
                    merge_move_result(insns, offset, ret);
                }
            }
            InsnType::FilledNewArray => {
                let arr = match &insn.kind {
                    InsnKind::FilledNewArray(elem) => ArgType::array(elem.clone()),
                    _ => continue,
                };
                merge_move_result(insns, offset, arr);
            }
            InsnType::FillArray => {
                let target = match &insn.kind {
                    InsnKind::FillArray { target, .. } => *target,
                    _ => continue,
                };
                let found = usize::try_from(target)
                    .ok()
                    .and_then(|t| insn_offset_at(insns, t));
                let payload =
                    found.and_then(|i| match insns[i].as_ref().map(|n| (&n.ty, &n.kind)) {
                        Some((InsnType::FillArrayData, InsnKind::FillArrayData(d))) => {
                            Some((i, d.clone()))
                        }
                        _ => None,
                    });
                match payload {
                    Some((i, data)) => {
                        if let Some(InsnKind::FillArray { data: slot, .. }) =
                            insns[offset].as_mut().map(|n| &mut n.kind)
                        {
                            *slot = Some(data);
                        }
                        insns[i] = None;
                    }
                    None => {
                        return Err(ProcessError(format!(
                            "Payload for fill-array not found at {}",
                            format_offset(target)
                        )))
                    }
                }
            }
            _ => {}
        }
    }
    Ok(errors)
}

fn attach_switch_data(
    insns: &mut [Option<InsnNode>],
    offset: usize,
    data_target: i32,
) -> Result<(), ProcessError> {
    let next = next_insn_offset(insns, offset)
        .map(|n| n as i32)
        .unwrap_or(-1);
    let found = usize::try_from(data_target)
        .ok()
        .and_then(|t| insn_offset_at(insns, t));
    let payload = found.and_then(|i| match insns[i].as_ref().map(|n| (&n.ty, &n.kind)) {
        Some((InsnType::SwitchData, InsnKind::SwitchData(d))) => Some((i, d.clone())),
        _ => None,
    });
    let Some((i, mut data)) = payload else {
        return Err(ProcessError(format!(
            "Payload for switch not found at {}",
            format_offset(data_target)
        )));
    };
    for t in &mut data.targets {
        *t = t.wrapping_add(offset as i32);
    }
    if let Some(InsnKind::Switch {
        data: slot, def, ..
    }) = insns[offset].as_mut().map(|n| &mut n.kind)
    {
        *slot = Some(data);
        *def = next;
    }
    insns[i] = None;
    Ok(())
}

fn merge_move_result(insns: &mut [Option<InsnNode>], offset: usize, res_type: ArgType) {
    let Some(next) = next_insn_offset(insns, offset) else {
        return;
    };
    let Some(next_insn) = insns[next].as_ref() else {
        return;
    };
    if next_insn.ty != InsnType::MoveResult {
        return;
    }
    let Some(move_res) = next_insn.result.clone() else {
        return;
    };
    let new_res = RegisterArg {
        immutable_type: move_res.immutable_type,
        ..RegisterArg::new(move_res.reg_num, res_type)
    };
    let next_insn = insns[next].take();
    if let (Some(insn), Some(next_insn)) = (insns[offset].as_mut(), next_insn) {
        insn.result = Some(new_res);
        insn.copy_attributes_from(&next_insn);
    }
}
