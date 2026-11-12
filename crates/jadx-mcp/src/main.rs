//! `jadx-mcp` binary: MCP server over stdio or a local socket.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(message) = jadx_mcp::cli::run(&args) {
        eprintln!("{message}");
        std::process::exit(2);
    }
}
