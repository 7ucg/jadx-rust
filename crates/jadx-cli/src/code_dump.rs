//! `insn-code`: code string of every block instruction and every wrapped instruction after the
//! shrink stage (oracle comparison format, see `tests/oracle/java/InsnCodeDump.java`).

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::Arc;

use anyhow::Result;
use jadx_core::codegen::{
    var_names::apply_variable_names, ClassGen, CodeWriter, Flags, GenEnv, InsnGen, MethodGen,
};
use jadx_core::decode::{InsnDecoder, MethodCtx};
use jadx_core::model::cfg::MethodCfg;
use jadx_core::model::insn::{iflag, InsnArg, InsnId, InsnKind};
use jadx_core::model::node::Root;
use jadx_dex_input::DexFile;

/// Stage of the pipeline the code is generated after (`CodeShrinkVisitor`).
const STAGE: u32 = 11;

pub fn insn_code(dexes: &[DexFile]) -> Result<()> {
    let (root, class_dex) = crate::dev::build_root(dexes)?;
    let root = Arc::new(root);
    let env = GenEnv::new(&root);
    let names = std::env::var("NAMES").is_ok_and(|v| v == "1");
    let extract_finally = std::env::var("JADX_RS_FINALLY").is_ok_and(|v| v == "1");
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    // one generator per top level class: inner classes share its imports
    let mut tops: Vec<Option<ClassGen>> = Vec::with_capacity(root.classes.len());
    for ci in 0..root.classes.len() {
        if root.top_parent_index(ci) == ci {
            let g = ClassGen::new(&env, ci);
            g.set_body_gen_started(true);
            tops.push(Some(g));
        } else {
            tops.push(None);
        }
    }
    for ci in root.sorted_class_order() {
        let cls = &root.classes[ci];
        let dex = &dexes[class_dex[ci]];
        let top = root.top_parent_index(ci);
        let top_gen = tops[top].as_ref().expect("top level generator");
        let own;
        let cls_gen = if top == ci {
            top_gen
        } else {
            own = ClassGen::with_parent(&env, ci, top_gen);
            own.set_body_gen_started(true);
            &own
        };
        for (mi, m) in cls.methods.iter().enumerate() {
            let Some(code_off) = m.code_offset else {
                continue;
            };
            let id = format!("{}.{}", cls.info.raw_name(), m.info.short_id());
            let code = dex.code_item(code_off)?;
            let raw = jadx_dex_input::insns::decode_insn_data(&code.insns)?;
            let ctx = MethodCtx {
                return_type: m.return_type.clone(),
                parent_class: cls.info.clone(),
                root: root.clone(),
            };
            let Ok(decoded) = InsnDecoder::new(dex, &ctx).process(&raw, code.insns.len()) else {
                writeln!(out, "M {id} LOAD_ERROR")?;
                continue;
            };
            let tries = dex.tries_api(&code)?;
            let frame = jadx_core::passes::pipeline::FrameInfo::new(
                &root,
                ci,
                mi,
                code.registers_size as usize,
            );
            let built = jadx_core::passes::pipeline::build_ssa(
                decoded,
                &tries,
                &root,
                ci,
                mi,
                &frame,
                STAGE,
                extract_finally,
            );
            let Ok(mut cfg) = built else {
                writeln!(out, "M {id} LOAD_ERROR")?;
                continue;
            };
            if names {
                prepare_names(&mut cfg, &root);
            }
            let mut text = format!("M {id}\n");
            dump_method(&mut text, cls_gen, mi, &cfg, names);
            out.write_all(text.as_bytes())?;
        }
    }
    Ok(())
}

/// Declare variables at their first assignment (in block order) and guess variable names.
fn prepare_names(cfg: &mut MethodCfg, root: &Root) {
    for v in cfg.svar_list.clone() {
        if let Some(cv) = cfg.svars[v].code_var {
            cfg.code_vars[cv].is_declared = true;
        }
    }
    apply_variable_names(cfg, root);
    for v in cfg.svar_list.clone() {
        if let Some(cv) = cfg.svars[v].code_var {
            cfg.code_vars[cv].is_declared = false;
        }
    }
    let arg_vars: Vec<_> = cfg
        .this_arg
        .iter()
        .chain(cfg.arg_regs.iter())
        .filter_map(|r| r.svar)
        .collect();
    for v in arg_vars {
        if let Some(cv) = cfg.svars[v].code_var {
            cfg.code_vars[cv].is_declared = true;
        }
    }
    for b in cfg.blocks.clone() {
        for i in cfg.arena[b].insns.clone() {
            if cfg.insns[i].contains_flag(iflag::DONT_GENERATE) {
                continue;
            }
            let Some(cv) = cfg.insns[i]
                .result
                .as_ref()
                .and_then(|r| r.svar)
                .and_then(|v| cfg.svars[v].code_var)
            else {
                continue;
            };
            if !cfg.code_vars[cv].is_declared {
                cfg.insns[i].flags |= iflag::DECLARE_VAR;
                cfg.code_vars[cv].is_declared = true;
            }
        }
    }
}

fn dump_method(out: &mut String, cls_gen: &ClassGen<'_>, mi: usize, cfg: &MethodCfg, names: bool) {
    let mgen = MethodGen::new(cls_gen, cfg, mi);
    let gen = InsnGen::new(&mgen, false);
    if names {
        // the method definition assigns the argument names
        mgen.assign_arg_names();
    }
    for &b in &cfg.blocks {
        for (i, &insn) in cfg.arena[b].insns.iter().enumerate() {
            let loc = format!("b{}i{}", cfg.arena[b].cid, i);
            statement(out, &gen, cfg, cls_gen, &loc, insn);
            wrapped(out, &gen, cfg, cls_gen, &loc, insn);
        }
    }
}

fn has_anonymous_ctor(cfg: &MethodCfg, root: &Root, insn: InsnId) -> bool {
    let node = &cfg.insns[insn];
    if let InsnKind::Constructor { call_mth, .. } = &node.kind {
        if let Some(c) = root.resolve_class(call_mth.decl_class.arg_type()) {
            if root.classes[c].is_anonymous() {
                return true;
            }
        }
    }
    node.args.iter().any(|a| match a {
        InsnArg::Wrapped(w) => has_anonymous_ctor(cfg, root, w.insn),
        _ => false,
    })
}

fn clean(code: &CodeWriter) -> String {
    let s = code.as_str();
    s.strip_prefix('\n')
        .unwrap_or(s)
        .replace('\r', "")
        .replace('\n', "<NL>")
}

fn statement(
    out: &mut String,
    gen: &InsnGen<'_, '_>,
    cfg: &MethodCfg,
    cls_gen: &ClassGen<'_>,
    loc: &str,
    insn: InsnId,
) {
    let ty = cfg.insns[insn].ty;
    let _ = write!(out, "S {loc} {ty} ");
    if has_anonymous_ctor(cfg, cls_gen.env().root, insn) {
        out.push_str("<ANON>\n");
        return;
    }
    let mut code = CodeWriter::new();
    match gen.make_insn(insn, &mut code, None) {
        Ok(()) => {
            out.push_str(&clean(&code));
            out.push('\n');
        }
        Err(_) => out.push_str("<ERR>\n"),
    }
}

fn wrapped(
    out: &mut String,
    gen: &InsnGen<'_, '_>,
    cfg: &MethodCfg,
    cls_gen: &ClassGen<'_>,
    loc: &str,
    insn: InsnId,
) {
    for (k, arg) in cfg.insns[insn].args.iter().enumerate() {
        let InsnArg::Wrapped(w) = arg else {
            continue;
        };
        let wloc = format!("{loc}.{k}");
        let wrap = &cfg.insns[w.insn];
        let _ = write!(out, "W {wloc} {} ", wrap.ty);
        if has_anonymous_ctor(cfg, cls_gen.env().root, w.insn) {
            out.push_str("<ANON>\n");
        } else {
            let mut code = CodeWriter::new();
            match gen.make_insn(w.insn, &mut code, Some(Flags::BodyOnly)) {
                Ok(()) => {
                    out.push_str(&clean(&code));
                    out.push('\n');
                }
                Err(_) => out.push_str("<ERR>\n"),
            }
        }
        wrapped(out, gen, cfg, cls_gen, &wloc, w.insn);
    }
}
