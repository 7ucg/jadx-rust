//! Decompilation passes.

pub mod attach_try_catch;
pub mod block_processor;
pub mod block_splitter;
pub mod block_utils;
pub mod constructor;
pub mod dominators;
pub mod exception_handler;
pub mod move_inline;
pub mod override_methods;
pub mod pipeline;
pub mod process_insns;
pub mod signature;
pub mod ssa;
