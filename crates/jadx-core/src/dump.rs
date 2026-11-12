//! Canonical text dump of instruction nodes, used by the oracle comparisons
//! (`tests/oracle/java/InsnNodeDump.java` prints the same format from the reference).

use std::fmt::Write as _;

use jadx_input_api::ArrayData;

use crate::model::cfg::MethodCfg;
use crate::model::insn::{FillArrayData, IndexValue, InsnArg, InsnKind, InsnNode, RegisterArg};

/// Dump context: the method (needed for wrapped instructions) and whether SSA versions are shown.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    cfg: Option<&'a MethodCfg>,
    versions: bool,
}

fn reg(r: &RegisterArg, ctx: Ctx) -> String {
    let cfg = if ctx.versions { ctx.cfg } else { None };
    let mut s = format!(
        "r{}:{}{}",
        r.reg_num,
        r.init_type,
        if r.immutable_type { "!" } else { "" }
    );
    if let (Some(cfg), Some(v)) = (cfg, r.svar) {
        let _ = write!(s, "/v{}", cfg.svars[v].version);
    }
    s
}

fn arg(a: &InsnArg, ctx: Ctx) -> String {
    match a {
        InsnArg::Register(r) => reg(r, ctx),
        InsnArg::Literal(l) => format!("#{}:{}", l.literal, l.ty),
        InsnArg::Wrapped(w) => {
            let inner = ctx
                .cfg
                .map(|c| insn_line(c.insn(w.insn), None, ctx))
                .unwrap_or_default();
            format!("(wrap {}:{})", w.ty, inner.trim_start_matches("I "))
        }
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
    insn_line(
        n,
        if_target,
        Ctx {
            cfg: None,
            versions: false,
        },
    )
}

/// Like [`dump_insn_with`] inside `cfg`; with `ssa` registers carry their SSA version
/// (`r1:int/v0`).
pub fn dump_insn_ssa(n: &InsnNode, if_target: Option<i32>, cfg: &MethodCfg, ssa: bool) -> String {
    insn_line(
        n,
        if_target,
        Ctx {
            cfg: Some(cfg),
            versions: ssa,
        },
    )
}

fn insn_line(n: &InsnNode, if_target: Option<i32>, cfg: Ctx) -> String {
    let mut s = String::new();
    let _ = write!(s, "I {:x} {}", n.offset, n.ty.name());
    let _ = write!(
        s,
        " res={}",
        n.result
            .as_ref()
            .map(|r| reg(r, cfg))
            .unwrap_or_else(|| "-".into())
    );
    let _ = write!(
        s,
        " args=[{}]",
        n.args
            .iter()
            .map(|a| arg(a, cfg))
            .collect::<Vec<_>>()
            .join(",")
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
        InsnKind::Phi { binds } => {
            let _ = write!(s, " binds=[{}]", cids(binds));
        }
        InsnKind::Constructor {
            call_mth,
            call_type,
        } => {
            let _ = write!(s, " ctor {call_mth} {}", call_type.name());
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
    dump_cfg_ext(id, cfg, false)
}

fn bits(b: &Option<crate::model::bitset::BitSet>) -> String {
    match b {
        Some(b) => b
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(","),
        None => "-".to_string(),
    }
}

fn opt(b: Option<usize>) -> String {
    b.map(|x| x.to_string()).unwrap_or_else(|| "-".into())
}

/// With `analysis`, dominators, loops and clean successors are included.
pub fn dump_cfg_ext(id: &str, cfg: &crate::model::cfg::MethodCfg, analysis: bool) -> String {
    dump_cfg_impl(id, cfg, analysis, false)
}

/// Like [`dump_cfg_ext`] with analysis, plus SSA variables, phi nodes and register versions.
pub fn dump_cfg_ssa(id: &str, cfg: &crate::model::cfg::MethodCfg) -> String {
    dump_cfg_impl(id, cfg, true, true)
}

fn insn_locations(cfg: &MethodCfg) -> std::collections::HashMap<usize, String> {
    let mut map = std::collections::HashMap::new();
    for bid in &cfg.blocks {
        let b = cfg.block(*bid);
        for (i, iid) in b.insns.iter().enumerate() {
            map.insert(*iid, format!("b{}i{}", b.cid, i));
        }
        for iid in b.phi_list.iter().flatten() {
            let reg = cfg
                .insn(*iid)
                .result
                .as_ref()
                .map(|r| r.reg_num)
                .unwrap_or(0);
            map.insert(*iid, format!("p{}r{}", b.cid, reg));
        }
    }
    map
}

fn arg_location(
    r: crate::model::ssa::ArgRef,
    locs: &std::collections::HashMap<usize, String>,
) -> String {
    use crate::model::ssa::ArgRef;
    let l = |i: &usize| locs.get(i).cloned().unwrap_or_else(|| "gone".into());
    match r {
        ArgRef::This => "this".into(),
        ArgRef::Method(k) => format!("m{k}"),
        ArgRef::Floating(_) => "floating".into(),
        ArgRef::Result(i) => format!("{}:res", l(&i)),
        ArgRef::Arg(i, k) => format!("{}:{k}", l(&i)),
    }
}

fn dump_cfg_impl(
    id: &str,
    cfg: &crate::model::cfg::MethodCfg,
    analysis: bool,
    ssa: bool,
) -> String {
    use crate::model::cfg::bflag;
    use crate::model::insn::iflag;
    let mut out = format!("M {id}\n");
    for hid in &cfg.handler_list {
        let h = &cfg.handlers[*hid];
        let types: Vec<&str> = h.catch_types.iter().map(|c| c.full_name()).collect();
        let _ = writeln!(out, "H {} types=[{}]", h.handler_offset, types.join(","));
    }
    if analysis {
        for t in &cfg.try_list {
            let tb = &cfg.try_blocks[*t];
            let _ = writeln!(
                out,
                "T {} handlers=[{}] blocks=[{}] outer={} inner=[{}] top={}",
                tb.id,
                tb.handlers
                    .iter()
                    .map(|h| cfg.handlers[*h].handler_offset.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                cids(&tb.blocks),
                tb.outer
                    .map(|o| cfg.try_blocks[o].id.to_string())
                    .unwrap_or_else(|| "-".into()),
                tb.inner
                    .iter()
                    .map(|i| cfg.try_blocks[*i].id.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                opt(tb.top_splitter)
            );
        }
        for l in &cfg.loops {
            let li = &cfg.loop_infos[*l];
            let mut blocks: Vec<usize> = li.blocks.iter();
            blocks.sort_unstable();
            let _ = writeln!(
                out,
                "L {} start={} end={} parent={} blocks=[{}]",
                li.id,
                li.start,
                li.end,
                li.parent
                    .map(|p| cfg.loop_infos[p].id.to_string())
                    .unwrap_or_else(|| "-".into()),
                cids(&blocks)
            );
        }
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
        let _ = write!(
            out,
            " exh={}",
            b.exc_handler
                .map(|h| cfg.handlers[h].handler_offset.to_string())
                .unwrap_or_else(|| "-".into())
        );
        if analysis {
            let clean = b
                .clean_succs()
                .map(|c| cids(c))
                .unwrap_or_else(|| "-".into());
            let loops: Vec<String> = b
                .loops
                .iter()
                .map(|l| {
                    let li = &cfg.loop_infos[*l];
                    format!("{}:{}>{}", li.id, li.start, li.end)
                })
                .collect();
            let _ = write!(
                out,
                " doms=[{}] idom={} df=[{}] pdoms=[{}] ipdom={} domon=[{}] clean=[{}] loops=[{}]",
                bits(&b.doms),
                opt(b.idom),
                bits(&b.dom_frontier),
                bits(&b.post_doms),
                opt(b.ipost_dom),
                cids(&b.dominates_on),
                clean,
                loops.join(",")
            );
        }
        out.push('\n');
        if ssa {
            for phi in b.phi_list.iter().flatten() {
                let insn = cfg.insn(*phi);
                let _ = writeln!(
                    out,
                    "  P {}",
                    dump_insn_ssa(insn, None, cfg, true).trim_start_matches("I ")
                );
            }
        }
        for iid in &b.insns {
            let insn = cfg.insn(*iid);
            let if_target = match &insn.kind {
                InsnKind::If {
                    then_block: Some(t),
                    ..
                } => Some(cfg.block(*t).start_offset),
                _ => None,
            };
            let _ = write!(out, "  {}", dump_insn_ssa(insn, if_target, cfg, ssa));
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
    if ssa {
        let locs = insn_locations(cfg);
        let mut cv_ids: Vec<usize> = Vec::new();
        for v in &cfg.svar_list {
            let var = &cfg.svars[*v];
            let uses: Vec<String> = var.uses.iter().map(|u| arg_location(*u, &locs)).collect();
            let phis = match &var.used_in_phi {
                None => "-".to_string(),
                Some(l) => l
                    .iter()
                    .map(|i| locs.get(i).cloned().unwrap_or_else(|| "gone".into()))
                    .collect::<Vec<_>>()
                    .join(","),
            };
            let _ = write!(
                out,
                "V r{}/v{} assign={} uses=[{}] phis=[{}]",
                var.reg_num,
                var.version,
                arg_location(var.assign, &locs),
                uses.join(","),
                phis
            );
            if let Some(cv) = var.code_var {
                let n = match cv_ids.iter().position(|c| *c == cv) {
                    Some(n) => n,
                    None => {
                        cv_ids.push(cv);
                        cv_ids.len() - 1
                    }
                };
                let c = &cfg.code_vars[cv];
                let mut flags = Vec::new();
                if c.is_declared {
                    flags.push("declared");
                }
                if c.is_final {
                    flags.push("final");
                }
                if c.is_this {
                    flags.push("this");
                }
                let _ = write!(
                    out,
                    " cv={n}[{}] cvty={} cvname={}",
                    flags.join(","),
                    c.ty.as_ref()
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "-".into()),
                    c.name.clone().unwrap_or_else(|| "-".into())
                );
            }
            out.push('\n');
        }
    }
    out
}
