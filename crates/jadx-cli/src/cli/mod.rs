//! User facing command line: `jadx-rs [options] <input files>`.

pub mod options;
pub mod run;

pub use run::run;
