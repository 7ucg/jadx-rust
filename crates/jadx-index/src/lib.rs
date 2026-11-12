//! Cross reference and name index over the classes of a loaded input.
//!
//! The crate works on the [`Root`](jadx_core::model::node::Root) of `jadx-core` (classes,
//! methods, fields) and on decoded instructions handed out by a [`CodeSource`]; it does not
//! touch the decompilation pipeline.
//!
//! * [`UsageIndex`]: callers and callees of methods, readers and writers of fields, string and
//!   integer literal sites, class uses by type operands. Built in one pass over all methods;
//!   the postings live in a bounded [`PostingStore`] (memory, or spilled to disk beyond a
//!   memory budget). Every hit is verified against the code.
//! * [`build_call_graph`]: bounded breadth first call graph around a method, in either
//!   direction, with Android component classification.
//! * [`NameIndex`]: exact, prefix, substring, glob, camel case and word search over class,
//!   method and field names, with a [`PackageScope`] filter.
//! * [`root_usage`]: usage lists the decompiler core already keeps on the root.
//!
//! Large parts of the design (fingerprint postings with verification, bounded call graph,
//! package scope) are adapted from RDX (Apache-2.0), see the `NOTICE` file.
//!
//! ```ignore
//! let source = DexCodeSource { dexes: &dexes, class_dex: &class_dex };
//! let index = UsageIndex::build(&root, &source, &BuildOptions::default())?;
//! let callers = index.callers_of(method_id, &Query::default())?;
//! ```

pub mod calls;
pub mod error;
pub mod names;
pub mod postings;
pub mod resolve;
pub mod root_usage;
pub mod scope;
pub mod source;
pub mod usage;

pub use calls::{
    build_call_graph, component_of, dispatch_targets, CallGraph, Component, Direction, Edge,
    GraphLimits, Node, NodeKey,
};
pub use error::{IndexError, Result};
pub use names::{MatchMode, NameHit, NameIndex, NameKind, NameQuery, NameResults};
pub use postings::{PostingStore, StoreConfig};
pub use resolve::{
    display_field, display_method, parse_field_ref, parse_method_ref, FieldId, Resolver,
};
pub use scope::PackageScope;
pub use source::{CodeSource, DexCodeSource, Insn, Operand, SiteKind};
pub use usage::{
    Access, BuildOptions, BuildStats, CallTarget, Callee, Needle, Query, ScanResult, Site, Sites,
    UsageIndex,
};
