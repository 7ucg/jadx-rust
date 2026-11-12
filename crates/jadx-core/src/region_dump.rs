//! Canonical text dump of the region tree (oracle comparison format, see
//! `tests/oracle/java/RegionDump.java`).

use std::fmt::Write as _;

use crate::dump::{dump_arg, dump_insn_ssa, flag_names};
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::condition::{if_op, CondMode, IfCondition};
use crate::model::insn::{iflag, InsnId, InsnKind};
use crate::model::region::{Container, LoopType, RegionId, RegionKind};

fn block_flags(flags: u64) -> String {
    let mut names: Vec<(u64, &str)> = bflag::NAMES.to_vec();
    names.extend_from_slice(bflag::REGION_NAMES);
    flag_names(flags, &names)
}

fn insn_flags(flags: u64) -> String {
    let mut names: Vec<(u64, &str)> = iflag::NAMES.to_vec();
    names.push((iflag::FORCE_ASSIGN_INLINE, "FORCE_ASSIGN_INLINE"));
    flag_names(flags, &names)
}

/// `IfCondition.toString` with arguments in the dump format.
pub fn cond_string(cfg: &MethodCfg, c: &IfCondition) -> String {
    match c.mode {
        CondMode::Compare => {
            let insn = c.compare.expect("compare");
            let n = cfg.insn(insn);
            format!(
                "{} {} {}",
                dump_arg(&n.args[0], cfg, true),
                if_op(cfg, insn).symbol(),
                dump_arg(&n.args[1], cfg, true)
            )
        }
        CondMode::Ternary => format!(
            "{} ? {} : {}",
            cond_string(cfg, c.first()),
            cond_string(cfg, c.second()),
            cond_string(cfg, c.third())
        ),
        CondMode::Not => format!("!({})", cond_string(cfg, c.first())),
        CondMode::And | CondMode::Or => {
            let op = if c.mode == CondMode::Or {
                " || "
            } else {
                " && "
            };
            let parts: Vec<String> = c.args.iter().map(|a| cond_string(cfg, a)).collect();
            format!("({})", parts.join(op))
        }
    }
}

fn opt_cond(cfg: &MethodCfg, c: &Option<IfCondition>) -> String {
    match c {
        Some(c) => cond_string(cfg, c),
        None => "-".to_string(),
    }
}

fn cids(v: &[usize]) -> String {
    v.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn opt(b: Option<usize>) -> String {
    b.map(|x| x.to_string()).unwrap_or_else(|| "-".into())
}

fn indent(out: &mut String, ind: usize) {
    out.push_str("R ");
    for _ in 0..ind {
        out.push_str("  ");
    }
}

/// One instruction line (same suffixes as the block dump).
pub fn insn_line(cfg: &MethodCfg, iid: InsnId) -> String {
    let insn = cfg.insn(iid);
    let if_target = match &insn.kind {
        InsnKind::If {
            then_block: Some(t),
            ..
        } => Some(cfg.block(*t).start_offset),
        _ => None,
    };
    let mut out = dump_insn_ssa(insn, if_target, cfg, true);
    let _ = write!(out, " fl=[{}]", insn_flags(insn.flags));
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
            let _ = write!(out, " then={} else={}", opt(*then_block), opt(*else_block));
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
            let _ = write!(out, " tblocks={} defblock={}", tb, opt(*def_block));
        }
        _ => {}
    }
    if let Some(loops) = cfg.regions.insn_loops.get(&iid) {
        let ids: Vec<String> = loops
            .iter()
            .map(|l| cfg.loop_infos[*l].id.to_string())
            .collect();
        let _ = write!(out, " loops=[{}]", ids.join(","));
    }
    if let Some(l) = cfg.regions.insn_loop_label.get(&iid) {
        let _ = write!(out, " label={}", cfg.loop_infos[*l].id);
    }
    out
}

fn dump_container(cfg: &MethodCfg, c: Container, ind: usize, out: &mut String) {
    match c {
        Container::Block(b) => {
            let blk = cfg.block(b);
            indent(out, ind);
            let _ = write!(out, "B {} flags=[{}]", blk.cid, block_flags(blk.flags));
            if cfg.regions.block_loop_label.contains_key(&b) {
                let _ = write!(
                    out,
                    " label={}",
                    cfg.loop_infos[cfg.regions.block_loop_label[&b]].id
                );
            }
            out.push('\n');
            for iid in &blk.insns {
                indent(out, ind + 1);
                out.push_str(&insn_line(cfg, *iid));
                out.push('\n');
            }
        }
        Container::Insns(i) => {
            indent(out, ind);
            let ic = &cfg.regions.insn_conts[i];
            let _ = writeln!(out, "INSNS flags=[{}]", block_flags(ic.flags));
            for iid in &ic.insns {
                indent(out, ind + 1);
                out.push_str(&insn_line(cfg, *iid));
                out.push('\n');
            }
        }
        Container::Region(r) => dump_region(cfg, r, ind, out),
    }
}

fn branch(cfg: &MethodCfg, name: &str, c: Option<Container>, ind: usize, out: &mut String) {
    indent(out, ind);
    match c {
        None => {
            let _ = writeln!(out, "{name} -");
        }
        Some(c) => {
            let _ = writeln!(out, "{name}");
            dump_container(cfg, c, ind + 1, out);
        }
    }
}

fn dump_region(cfg: &MethodCfg, r: RegionId, ind: usize, out: &mut String) {
    let region = &cfg.regions.arena[r];
    let flags = block_flags(region.flags);
    let exh = region
        .exc_handler
        .map(|h| format!(" exh={}", cfg.handlers[h].handler_offset))
        .unwrap_or_default();
    indent(out, ind);
    match &region.kind {
        RegionKind::Plain { blocks } => {
            let _ = writeln!(out, "REGION flags=[{flags}]{exh}");
            for b in blocks {
                dump_container(cfg, *b, ind + 1, out);
            }
        }
        RegionKind::If {
            cond,
            cond_blocks,
            then_r,
            else_r,
        } => {
            let _ = writeln!(
                out,
                "IF cond={} cbs=[{}] flags=[{flags}]{exh}",
                opt_cond(cfg, cond),
                cids(cond_blocks)
            );
            branch(cfg, "THEN", *then_r, ind + 1, out);
            branch(cfg, "ELSE", *else_r, ind + 1, out);
        }
        RegionKind::Loop {
            info,
            cond,
            cond_blocks,
            header,
            condition_at_end,
            pre_condition,
            body,
            ty,
        } => {
            let ty = match ty {
                None => "-".to_string(),
                Some(LoopType::For { init, incr }) => format!(
                    "for init=[{}] incr=[{}]",
                    insn_line(cfg, *init),
                    insn_line(cfg, *incr)
                ),
                Some(LoopType::ForEach {
                    var_arg_insn,
                    iterable_insn,
                }) => format!(
                    "foreach var=[{}] iter=[{}]",
                    insn_line(cfg, *var_arg_insn),
                    insn_line(cfg, *iterable_insn)
                ),
            };
            let _ = writeln!(
                out,
                "LOOP id={} header={} end={} pre={} cond={} cbs=[{}] type={} flags=[{flags}]{exh}",
                cfg.loop_infos[*info].id,
                opt(*header),
                u8::from(*condition_at_end),
                opt(*pre_condition),
                opt_cond(cfg, cond),
                cids(cond_blocks),
                ty
            );
            branch(cfg, "BODY", body.map(Container::Region), ind + 1, out);
        }
        RegionKind::Switch { header, cases } => {
            let _ = writeln!(out, "SWITCH header={header} flags=[{flags}]{exh}");
            for c in cases {
                indent(out, ind + 1);
                let keys: Vec<String> = c.keys.iter().map(|k| k.text()).collect();
                let _ = writeln!(out, "CASE [{}]", keys.join(","));
                dump_container(cfg, c.container, ind + 2, out);
            }
        }
        RegionKind::Sync {
            enter_insn,
            exit_insns,
            ..
        } => {
            let exits: Vec<String> = exit_insns
                .iter()
                .map(|i| format!("{:x}", cfg.insn(*i).offset))
                .collect();
            let _ = writeln!(
                out,
                "SYNC enter={:x} exits=[{}] flags=[{flags}]{exh}",
                cfg.insn(*enter_insn).offset,
                exits.join(",")
            );
            for b in cfg.regions.sub_blocks(r) {
                dump_container(cfg, b, ind + 1, out);
            }
        }
        RegionKind::TryCatch {
            try_r,
            catches,
            finally_r,
            try_block,
        } => {
            let _ = writeln!(
                out,
                "TRY id={} flags=[{flags}]{exh}",
                try_block
                    .map(|t| cfg.try_blocks[t].id.to_string())
                    .unwrap_or_else(|| "-".into())
            );
            branch(cfg, "TRYBODY", Some(*try_r), ind + 1, out);
            for (h, c) in catches {
                branch(
                    cfg,
                    &format!("CATCH {}", cfg.handlers[*h].handler_offset),
                    Some(*c),
                    ind + 1,
                    out,
                );
            }
            if let Some(f) = finally_r {
                branch(cfg, "FINALLY", Some(*f), ind + 1, out);
            }
        }
    }
}

/// Region tree of the method (including exception handler regions).
pub fn dump_regions(id: &str, cfg: &MethodCfg) -> String {
    let mut out = format!("M {id}\n");
    if let Some(r) = cfg.regions.method_region {
        dump_container(cfg, Container::Region(r), 0, &mut out);
    }
    for h in &cfg.handler_list {
        let hr = cfg.regions.handler_regions.get(h).copied();
        indent(&mut out, 0);
        let _ = writeln!(
            out,
            "HANDLER {}{}",
            cfg.handlers[*h].handler_offset,
            if hr.is_none() { " none" } else { "" }
        );
        if let Some(hr) = hr {
            dump_container(cfg, hr, 1, &mut out);
        }
    }
    out
}
