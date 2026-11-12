//! Region building stages: `RegionMakerVisitor` and the passes that follow it.

pub mod block_ext;
pub mod clean;
pub mod if_maker;
pub mod loop_maker;
pub mod maker;
pub mod post_process;
pub mod switch_maker;
pub mod sync_maker;
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
    Ok(())
}

/// `RegionMakerVisitor.visit`
pub fn region_maker_visitor(cfg: &mut MethodCfg, env: &MethodEnv) -> Res<()> {
    if cfg.blocks.is_empty() {
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

fn process_force_inline_insns(cfg: &mut MethodCfg) -> Res<()> {
    let need_shrink = cfg.blocks.iter().any(|b| {
        cfg.block(*b)
            .insns
            .iter()
            .any(|i| cfg.insn(*i).contains_flag(iflag::FORCE_ASSIGN_INLINE))
    });
    if need_shrink {
        crate::passes::code_shrink::code_shrink(cfg)?;
    }
    Ok(())
}
