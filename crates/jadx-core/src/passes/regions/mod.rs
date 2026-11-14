//! Region building stages: `RegionMakerVisitor` and the passes that follow it.

pub mod block_ext;
pub mod check;
pub mod clean;
pub mod if_maker;
pub mod if_visitor;
pub mod loop_maker;
pub mod loop_visitor;
pub mod maker;
pub mod post_process;
pub mod switch_break;
pub mod switch_maker;
pub mod switch_over_string;
pub mod sync_maker;
pub mod ternary_mod;
pub mod traversal;
pub mod try_catch;
pub mod utils;

use crate::model::cfg::MethodCfg;
use crate::model::insn::iflag;
use crate::passes::constructor::MethodEnv;
use crate::passes::regions::maker::{RegionMaker, Res};

/// `ACC_SYNCHRONIZED | ACC_DECLARED_SYNCHRONIZED`
const ACC_SYNCHRONIZED: u32 = 0x20 | 0x20000;

/// The region passes of the default pipeline up to `stage` (12 = `RegionMakerVisitor`).
pub fn run_region_stages(cfg: &mut MethodCfg, env: &MethodEnv, stage: u32) -> Res<()> {
    if stage >= 12 {
        region_maker_visitor(cfg, env)?;
    }
    if stage >= 13 && !cfg.blocks.is_empty() {
        if_visitor::if_region_visitor(cfg, env)?;
    }
    if stage >= 14 {
        if_visitor::return_visitor(cfg)?;
    }
    if stage >= 15 {
        clean::clean_regions(cfg)?;
    }
    if stage >= 16 {
        shrink_method(cfg)?;
    }
    if stage >= 17 {
        check::check_regions(cfg)?;
    }
    if stage >= 18 {
        loop_visitor::loop_region_visitor(cfg, env)?;
    }
    if stage >= 19 {
        switch_break::switch_break_visitor(cfg, env)?;
    }
    Ok(())
}

/// `RegionMakerVisitor.visit`
/// Block-count ceiling above which [`region_maker_visitor`] abandons structured recovery for a
/// method and lets it fall back to an instruction dump (see the call site). The region recovery
/// and the following region/variable analyses are super-linear in the block count, so beyond a
/// few thousand blocks a single method stalls for minutes. Measured anchors: the largest method
/// in a normal app (MT Manager.apk) has 1567 blocks and decompiles in well under a second, while
/// WhatsApp's obfuscated giants (2577 up to 9393 blocks) each stall for minutes; 2000 sits above
/// every real method and below the degenerate ones. Overridable with `JADX_RS_MAX_REGION_BLOCKS`.
const MAX_REGION_BLOCKS: usize = 2000;

pub fn region_maker_visitor(cfg: &mut MethodCfg, env: &MethodEnv) -> Res<()> {
    if cfg.blocks.is_empty() {
        return Ok(());
    }
    if std::env::var("JADX_RS_DEBUG_BLOCKS").is_ok() && cfg.blocks.len() > 500 {
        eprintln!(
            "BLOCKS {} {}.{}",
            cfg.blocks.len(),
            env.root.classes[env.cls].info.raw_name(),
            env.root.classes[env.cls].methods[env.mth].info.name
        );
    }
    // Pathological methods (heavily obfuscated code with enormous control-flow graphs) make the
    // region recovery and the variable/region analyses that follow it run in time super-linear
    // in the block count, which on the largest apps can stall for many minutes on a single
    // method. Above a block-count limit, skip structured recovery for this method and let it
    // fall back to a commented instruction dump (the same outcome the reference reaches when its
    // own region stack limit throws) - recorded as a method error so every later per-method pass
    // skips it, exactly like a type-inference overflow. The limit is far above any method in real
    // code (so normal output is unaffected) and overridable for experiments.
    let limit = std::env::var("JADX_RS_MAX_REGION_BLOCKS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(MAX_REGION_BLOCKS);
    if cfg.blocks.len() > limit {
        cfg.errors.push(format!(
            "Region recovery skipped: control-flow graph too large ({} blocks)",
            cfg.blocks.len()
        ));
        return Ok(());
    }
    {
        let mut rm = RegionMaker::new(cfg, env);
        let mth_region = rm.make_mth_region()?;
        rm.cfg.regions.method_region = Some(mth_region);
        if !rm.cfg.handler_list.is_empty() {
            rm.process_exc_handlers()?;
        }
    }
    process_force_inline_insns(cfg)?;
    try_catch::process_try_catch_regions(cfg)?;
    post_process::post_process_regions(cfg)?;
    clean::clean_regions(cfg)?;
    let m = &env.root.classes[env.cls].methods[env.mth];
    if m.access & ACC_SYNCHRONIZED != 0 {
        let class_type = env.root.classes[env.cls].info.arg_type().clone();
        sync_maker::remove_synchronized(
            cfg,
            m.access & crate::model::node::ACC_STATIC != 0,
            &class_type,
        )?;
    }
    Ok(())
}

/// `CodeShrinkVisitor.shrinkMethod`: also clears the shrink request.
pub fn shrink_method(cfg: &mut MethodCfg) -> Res<()> {
    cfg.regions.request_code_shrink = false;
    crate::passes::code_shrink::code_shrink(cfg)
}

fn process_force_inline_insns(cfg: &mut MethodCfg) -> Res<()> {
    let need_shrink = cfg.blocks.iter().any(|b| {
        cfg.block(*b)
            .insns
            .iter()
            .any(|i| cfg.insn(*i).contains_flag(iflag::FORCE_ASSIGN_INLINE))
    });
    if need_shrink {
        shrink_method(cfg)?;
    }
    Ok(())
}
