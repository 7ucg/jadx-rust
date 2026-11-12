//! `jadx-rs`: decompiler command line.
//!
//! `jadx-rs [options] <input files>` is the user facing interface (see `--help`). The first
//! argument selecting one of the developer subcommands (`dex-info`, `smali`, `pre-info`, ...)
//! runs an inspection helper instead.

mod cli;
mod dev;
mod index_dump;
mod input_dump;
mod java_dump;
mod res_dump;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .first()
        .is_some_and(|a| dev::COMMANDS.contains(&a.as_str()))
    {
        if let Err(e) = dev::run() {
            eprintln!("Error: {e:?}");
            std::process::exit(1);
        }
        return;
    }
    std::process::exit(cli::run(&args));
}
