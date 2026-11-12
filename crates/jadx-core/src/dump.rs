//! Canonical text dump of instruction nodes, used by the oracle comparisons
//! (`tests/oracle/java/InsnNodeDump.java` prints the same format from the reference).

use std::fmt::Write as _;

use jadx_input_api::ArrayData;

use crate::model::insn::{FillArrayData, IndexValue, InsnArg, InsnKind, InsnNode, RegisterArg};

fn reg(r: &RegisterArg) -> String {
    format!(
        "r{}:{}{}",
        r.reg_num,
        r.init_type,
        if r.immutable_type { "!" } else { "" }
    )
}

fn arg(a: &InsnArg) -> String {
    match a {
        InsnArg::Register(r) => reg(r),
        InsnArg::Literal(l) => format!("#{}:{}", l.literal, l.ty),
    }
}

fn ints(v: &[i32]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn data_string(d: &FillArrayData) -> String {
    match &d.data {
        ArrayData::Empty => "[]".into(),
        ArrayData::Bytes(v) => format!(
            "[{}]",
            v.iter()
                .map(|b| (*b as i8).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ArrayData::Shorts(v) => format!(
            "[{}]",
            v.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ArrayData::Ints(v) => ints(v),
        ArrayData::Longs(v) => format!(
            "[{}]",
            v.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn fill_string(d: &FillArrayData) -> String {
    format!("{}:{}:{}", d.size, d.elem_type, data_string(d))
}

/// One line describing `n` (without trailing newline).
pub fn dump_insn(n: &InsnNode) -> String {
    dump_insn_with(n, None)
}

/// Like [`dump_insn`]; `if_target` replaces the printed `if` target (the reference reports the start
/// offset of the `then` block once blocks are initialised).
pub fn dump_insn_with(n: &InsnNode, if_target: Option<i32>) -> String {
    let mut s = String::new();
    let _ = write!(s, "I {:x} {}", n.offset, n.ty.name());
    let _ = write!(
        s,
        " res={}",
        n.result.as_ref().map(reg).unwrap_or_else(|| "-".into())
    );
    let _ = write!(
        s,
        " args=[{}]",
        n.args.iter().map(arg).collect::<Vec<_>>().join(",")
    );
    match &n.kind {
        InsnKind::None => {}
        InsnKind::Goto { target } => {
            let _ = write!(s, " goto={target}");
        }
        InsnKind::If { op, target, .. } => {
            let target = if_target.unwrap_or(*target);
            let _ = write!(s, " if={} target={target}", op.name());
        }
        InsnKind::Switch {
            data_target,
            packed,
            data,
            def,
            ..
        } => {
            let _ = write!(s, " switch packed={packed} data={data_target} def={def}");
            match data {
                Some(d) => {
                    let _ = write!(s, " keys={} targets={}", ints(&d.keys), ints(&d.targets));
                }
                None => s.push_str(" nodata"),
            }
        }
        InsnKind::SwitchData(d) => {
            let _ = write!(
                s,
                " swdata keys={} targets={}",
                ints(&d.keys),
                ints(&d.targets)
            );
        }
        InsnKind::ConstStr(v) => {
            let units: Vec<String> = v.iter().map(|u| format!("{u:x}")).collect();
            let _ = write!(s, " str=[{}]", units.join(","));
        }
        InsnKind::ConstClass(t) => {
            let _ = write!(s, " cls={t}");
        }
        InsnKind::Index(IndexValue::Type(t)) => {
            let _ = write!(s, " idx={t}");
        }
        InsnKind::Index(IndexValue::Field(f)) => {
            let _ = write!(s, " idx={f}");
        }
        InsnKind::Invoke { invoke_type, mth } => {
            let _ = write!(s, " invoke={} {mth}", invoke_type.name());
        }
        InsnKind::Arith(op) => {
            let _ = write!(s, " op={}", op.name());
        }
        InsnKind::NewArray(t) => {
            let _ = write!(s, " arr={t}");
        }
        InsnKind::FilledNewArray(t) => {
            let _ = write!(s, " elem={t}");
        }
        InsnKind::FillArray { target, data } => {
            let _ = write!(s, " fill target={target}");
            match data {
                Some(d) => {
                    let _ = write!(s, " data={}", fill_string(d));
                }
                None => s.push_str(" nodata"),
            }
        }
        InsnKind::FillArrayData(d) => {
            let _ = write!(s, " fdata={}", fill_string(d));
        }
    }
    if !n.jumps.is_empty() {
        let j: Vec<String> = n
            .jumps
            .iter()
            .map(|j| format!("{}>{}", j.src, j.dest))
            .collect();
        let _ = write!(s, " jumps=[{}]", j.join(","));
    }
    if n.error.is_some() {
        s.push_str(" ERROR");
    }
    s
}

fn flag_names(flags: u64, names: &[(u64, &str)]) -> String {
    let mut v: Vec<&str> = names
        .iter()
        .filter(|(f, _)| flags & f != 0)
        .map(|(_, n)| *n)
        .collect();
    v.sort_unstable();
    v.join(",")
}

fn cids(v: &[usize]) -> String {
    v.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Block graph dump (oracle comparison format, see `tests/oracle/java/BlockDump.java`).
pub fn dump_cfg(id: &str, cfg: &crate::model::cfg::MethodCfg) -> String {
    use crate::model::cfg::bflag;
    use crate::model::insn::iflag;
    let mut out = format!("M {id}\n");
    for h in &cfg.handlers {
        let types: Vec<&str> = h.catch_types.iter().map(|c| c.full_name()).collect();
        let _ = writeln!(out, "H {} types=[{}]", h.handler_offset, types.join(","));
    }
    for bid in &cfg.blocks {
        let b = cfg.block(*bid);
        let _ = write!(
            out,
            "B {} pos={} off={} flags=[{}] preds=[{}] succs=[{}]",
            b.cid,
            b.pos,
            b.start_offset,
            flag_names(b.flags, bflag::NAMES),
            cids(&b.preds),
            cids(&b.succs)
        );
        let _ = write!(
            out,
            " tmp={}",
            b.tmp_edge
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".into())
        );
        let _ = writeln!(
            out,
            " exh={}",
            b.exc_handler
                .map(|h| cfg.handlers[h].handler_offset.to_string())
                .unwrap_or_else(|| "-".into())
        );
        for insn in &b.insns {
            let if_target = match &insn.kind {
                InsnKind::If {
                    then_block: Some(t),
                    ..
                } => Some(cfg.block(*t).start_offset),
                _ => None,
            };
            let _ = write!(out, "  {}", dump_insn_with(insn, if_target));
            let _ = write!(out, " fl=[{}]", flag_names(insn.flags, iflag::NAMES));
            if let Some(c) = &insn.catch {
                let offs: Vec<String> = c
                    .iter()
                    .map(|h| cfg.handlers[*h].handler_offset.to_string())
                    .collect();
                let _ = write!(out, " catch=[{}]", offs.join(","));
            }
            if let Some(h) = insn.exc_handler {
                let _ = write!(out, " exh={}", cfg.handlers[h].handler_offset);
            }
            match &insn.kind {
                InsnKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    let f =
                        |b: &Option<usize>| b.map(|x| x.to_string()).unwrap_or_else(|| "-".into());
                    let _ = write!(out, " then={} else={}", f(then_block), f(else_block));
                }
                InsnKind::Switch {
                    target_blocks,
                    def_block,
                    ..
                } => {
                    let tb = target_blocks
                        .as_ref()
                        .map(|t| cids(t))
                        .unwrap_or_else(|| "-".into());
                    let _ = write!(
                        out,
                        " tblocks={} defblock={}",
                        tb,
                        def_block
                            .map(|x| x.to_string())
                            .unwrap_or_else(|| "-".into())
                    );
                }
                _ => {}
            }
            out.push('\n');
        }
    }
    out
}
