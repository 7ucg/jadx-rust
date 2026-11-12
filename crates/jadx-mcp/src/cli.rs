//! Command line of the server: `jadx-mcp [--socket PATH] [--verify-checksum] [INPUT]`.
//! The same entry point backs the `mcp` subcommand of `jadx-rs`.

use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::EngineLoader;
use crate::project::LoadOptions;
use crate::protocol::serve;
use crate::server::SharedState;
use crate::socket::serve_unix;

pub const USAGE: &str = "usage: jadx-mcp [--socket PATH] [--verify-checksum] [INPUT]\n\
\n\
Model Context Protocol server for the decompiler engine.\n\
Serves JSON-RPC over stdio, or over a Unix domain socket with --socket.\n\
INPUT (apk, xapk, dex, jar, class) is loaded at start; without it use the open_project tool.";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CliArgs {
    pub socket: Option<PathBuf>,
    pub verify_checksum: bool,
    pub input: Option<PathBuf>,
    pub help: bool,
}

pub fn parse_args(args: &[String]) -> Result<CliArgs, String> {
    let mut out = CliArgs::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => out.help = true,
            "--verify-checksum" => out.verify_checksum = true,
            "--socket" => {
                let path = iter.next().ok_or("--socket needs a path")?;
                out.socket = Some(PathBuf::from(path));
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other => {
                if out.input.replace(PathBuf::from(other)).is_some() {
                    return Err("only one input file can be given".into());
                }
            }
        }
    }
    Ok(out)
}

/// Run the server with the given command line arguments (without the program name).
pub fn run(args: &[String]) -> Result<(), String> {
    let cli = parse_args(args).map_err(|e| format!("{e}\n{USAGE}"))?;
    if cli.help {
        println!("{USAGE}");
        return Ok(());
    }
    let shared = SharedState::new(Arc::new(EngineLoader));
    if let Some(input) = &cli.input {
        let path = std::fs::canonicalize(input)
            .map_err(|e| format!("cannot open {}: {e}", input.display()))?;
        let options = LoadOptions {
            verify_checksum: cli.verify_checksum,
        };
        shared
            .open_project(&path, &options)
            .map_err(|e| e.to_string())?;
    }
    match &cli.socket {
        Some(path) => serve_unix(path, shared).map_err(|e| format!("socket error: {e}")),
        None => {
            let stdin = BufReader::new(std::io::stdin());
            serve(stdin, std::io::stdout(), shared).map_err(|e| format!("transport error: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_options_and_input() {
        let cli = parse_args(&args(&["--socket", "/tmp/x", "--verify-checksum", "a.apk"])).unwrap();
        assert_eq!(cli.socket, Some(PathBuf::from("/tmp/x")));
        assert!(cli.verify_checksum);
        assert_eq!(cli.input, Some(PathBuf::from("a.apk")));
        assert!(parse_args(&args(&["--bogus"])).is_err());
        assert!(parse_args(&args(&["--socket"])).is_err());
        assert!(parse_args(&args(&["a", "b"])).is_err());
        assert!(parse_args(&args(&["-h"])).unwrap().help);
    }
}
