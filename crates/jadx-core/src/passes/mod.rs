//! Decompilation passes.

pub mod attach_try_catch;
pub mod block_processor;
pub mod block_splitter;
pub mod dominators;
pub mod override_methods;
pub mod pipeline;
pub mod process_insns;
pub mod signature;
