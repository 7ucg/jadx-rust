//! Per-method pass pipeline (grows with each ported stage).

use jadx_input_api::TryData;

use crate::decode::DecodedMethod;
use crate::model::cfg::MethodCfg;
use crate::passes::attach_try_catch::{attach_try_catch, sort_catch_attrs};
use crate::passes::block_splitter::split_blocks;
use crate::passes::process_insns::process_instructions;
use crate::root::RootCtx;

/// AttachTryCatch + ProcessInstructions + BlockSplitter.
pub fn build_blocks(
    mut decoded: DecodedMethod,
    tries: &[TryData],
    root: &RootCtx,
) -> Result<MethodCfg, String> {
    let handlers = attach_try_catch(&mut decoded.insns, tries, root)?;
    sort_catch_attrs(&mut decoded.insns, &handlers);
    process_instructions(&mut decoded.insns).map_err(|e| e.0)?;
    let mut cfg = MethodCfg {
        handlers,
        ..Default::default()
    };
    split_blocks(decoded.insns, &mut cfg).map_err(|e| e.0)?;
    Ok(cfg)
}
