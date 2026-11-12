//! Per-method pass pipeline (grows with each ported stage).

use jadx_input_api::TryData;

use crate::decode::DecodedMethod;
use crate::model::cfg::MethodCfg;
use crate::model::insn::InsnType;
use crate::passes::attach_try_catch::{attach_try_catch, sort_catch_attrs};
use crate::passes::block_processor::process_blocks;
use crate::passes::block_splitter::split_blocks;
use crate::passes::process_insns::process_instructions;
use crate::root::RootCtx;

/// AttachTryCatch + ProcessInstructions + BlockSplitter.
pub fn build_blocks(
    mut decoded: DecodedMethod,
    tries: &[TryData],
    root: &RootCtx,
    void_return: bool,
) -> Result<MethodCfg, String> {
    let handlers = attach_try_catch(&mut decoded.insns, tries, root)?;
    sort_catch_attrs(&mut decoded.insns, &handlers);
    let has_switch = decoded
        .insns
        .iter()
        .flatten()
        .any(|i| i.ty == InsnType::Switch);
    let has_move_exc = decoded
        .insns
        .iter()
        .flatten()
        .any(|i| i.ty == InsnType::MoveException);
    process_instructions(&mut decoded.insns).map_err(|e| e.0)?;
    let handler_list: Vec<usize> = (0..handlers.len()).collect();
    let mut cfg = MethodCfg {
        handlers,
        handler_list,
        compute_post_dom: has_switch || has_move_exc,
        has_switch,
        void_return,
        ..Default::default()
    };
    split_blocks(decoded.insns, &mut cfg).map_err(|e| e.0)?;
    Ok(cfg)
}

/// [`build_blocks`] followed by `BlockProcessor`.
pub fn build_processed_blocks(
    decoded: DecodedMethod,
    tries: &[TryData],
    root: &RootCtx,
    clsp: &crate::clsp::ClspGraph,
    void_return: bool,
) -> Result<MethodCfg, String> {
    let mut cfg = build_blocks(decoded, tries, root, void_return)?;
    process_blocks(&mut cfg, clsp)?;
    Ok(cfg)
}

/// Method frame description needed by SSA construction.
pub struct FrameInfo {
    pub regs_count: usize,
    pub is_static: bool,
    pub this_type: crate::model::types::ArgType,
    pub arg_types: Vec<crate::model::types::ArgType>,
}

impl FrameInfo {
    /// Frame of method `mth` of class `cls` (`MethodNode.initArguments` types).
    pub fn new(
        root: &crate::model::node::Root,
        cls: usize,
        mth: usize,
        regs_count: usize,
    ) -> FrameInfo {
        let tu = crate::type_utils::TypeUtils::new(root);
        let m = &root.classes[cls].methods[mth];
        FrameInfo {
            regs_count,
            is_static: m.access & crate::model::node::ACC_STATIC != 0,
            this_type: tu.expand_type_variables(cls, mth, &tu.class_type(cls)),
            arg_types: m
                .arg_types
                .iter()
                .map(|t| tu.expand_type_variables(cls, mth, t))
                .collect(),
        }
    }
}

/// [`build_processed_blocks`] followed by `SSATransform` and, depending on `stage`, the later
/// SSA passes (3 = `MoveInlineVisitor`, 4 = `ConstructorVisitor`, 5 = `InitCodeVariables`, 6 = `ConstInlineVisitor`).
pub fn build_ssa(
    decoded: DecodedMethod,
    tries: &[TryData],
    root: &crate::model::node::Root,
    cls: usize,
    mth: usize,
    frame: &FrameInfo,
    stage: u32,
) -> Result<MethodCfg, String> {
    let void_return = root.classes[cls].methods[mth].info.return_type.is_void();
    let mut cfg = build_processed_blocks(decoded, tries, &root.ctx, &root.clsp, void_return)?;
    crate::passes::ssa::init_arguments(
        &mut cfg,
        frame.regs_count,
        frame.is_static,
        &frame.this_type,
        &frame.arg_types,
    );
    crate::passes::ssa::ssa_transform(&mut cfg)?;
    if stage >= 3 {
        crate::passes::move_inline::move_inline(&mut cfg)?;
    }
    if stage >= 4 {
        let env = crate::passes::constructor::MethodEnv { root, cls, mth };
        if crate::passes::constructor::replace_invoke(&mut cfg, &env)? {
            crate::passes::move_inline::move_inline(&mut cfg)?;
        }
    }
    if stage >= 5 {
        crate::passes::init_code_vars::init_code_vars(&mut cfg)?;
    }
    if stage >= 6 {
        crate::passes::const_inline::const_inline(&mut cfg)?;
    }
    Ok(cfg)
}
