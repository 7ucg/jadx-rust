//! Developer subcommand `input-classes <file>...`: every class of all code inputs (dex files
//! anywhere in the archive, JVM classes in nested archives, the APKs of split packages) after the
//! duplicate selection of the reference root, one `name<TAB>source` line per class, sorted by
//! name. Used by `tests/oracle/input_oracle.sh`.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{bail, Result};
use jadx_package::{load_inputs, LoadOptions};

pub fn run(args: &[String]) -> Result<()> {
    let paths: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        bail!("usage: jadx-rs input-classes <file>... [--no-verify-checksum]");
    }
    let opts = LoadOptions {
        verify_checksum: !args.iter().any(|a| a == "--no-verify-checksum"),
        ..LoadOptions::default()
    };
    let set = load_inputs(&paths, &opts);
    for w in &set.warnings {
        eprintln!("warning: {w}");
    }
    let (classes, dup_warnings) = set.deduplicated();
    for w in &dup_warnings {
        eprintln!("warning: {w}");
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for c in &classes {
        writeln!(out, "{}\t{}", c.raw_name, c.source)?;
    }
    Ok(())
}
