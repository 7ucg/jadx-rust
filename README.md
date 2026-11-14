# jadx-rust

`jadx-rs`: a Rust reimplementation of the [jadx](https://github.com/skylot/jadx) decompiler
engine (Android DEX / Java bytecode to Java source). Work in progress, built bottom-up and
verified against the reference Java implementation at every stage.

## Status

Work in progress. The full pipeline (DEX/class input, SSA, type inference, regions, codegen,
resources, CLI, MCP server) is implemented and continuously checked against the reference
Java jadx; current parity on a large real-world app is in the 90%+ range for both whole-class
and member-level source comparison. See [CHANGELOG.md](CHANGELOG.md) for what has shipped and
how each change was verified.

## Layout

```
crates/
  jadx-core/        engine: model, passes, type inference, regions, codegen
  jadx-api/         public library API
  jadx-input-api/   input plugin traits
  jadx-dex-input/   DEX reader (header, tables, MUTF-8, apk/zip/v41 containers)
  jadx-java-input/  JVM .class reader
  jadx-package/     package containers (xapk/apks/apkm), multi-source input loading
  jadx-index/       cross reference index: callers, field access, literals, call graph, name search
  jadx-cli/         `jadx-rs` command line tool
  jadx-mcp/         MCP server (stdio / local socket): `jadx-mcp` binary, `jadx-rs mcp`
tests/oracle/       comparison harness against the Java jadx
```

## Requirements

- Rust 1.82 or newer (stable toolchain), installed via [rustup](https://rustup.rs). No other
  system packages are needed: every dependency is pure Rust.
- To run the oracle checks against the reference decompiler (optional, only needed for
  development): a JDK (11+) and a local checkout of
  [jadx](https://github.com/skylot/jadx) built with `./gradlew :jadx-cli:installDist`.

## Setup

Clone the repository and build the workspace:

```
git clone <repository-url>
cd jadx-rust
cargo build --release
```

This produces two binaries under `target/release/`: `jadx-rs` (the CLI) and `jadx-mcp` (the MCP
server). Run either directly from there, or install them onto your `PATH`:

```
cargo install --path crates/jadx-cli   # installs `jadx-rs`
cargo install --path crates/jadx-mcp   # installs `jadx-mcp`
```

Run the test suite to confirm the build is healthy:

```
cargo test --workspace
```

## Usage

```
jadx-rs -d out app.apk                       # decompile to out/sources, out/resources
jadx-rs -d out --no-res app.apk              # sources only, skip resource decoding
jadx-rs -d out --export-gradle app.apk       # also write an Android Studio / Gradle project
jadx-rs -d out --output-format json app.apk  # JSON output with bytecode offset mapping
jadx-rs --help                               # full flag list (deobfuscation, decompilation mode, …)
```

Developer subcommands for inspecting individual pipeline stages (`smali`, `dex-info`,
`dex-classes`, `insn-dump`, `insn-ssa`, `xref`, …) are listed in `jadx-rs --help`.

## MCP server

`jadx-rs mcp [--socket PATH] [INPUT]` (or the `jadx-mcp` binary) runs a Model Context Protocol
server over stdio, or over a Unix domain socket with `--socket`. `INPUT` (apk, xapk, dex, jar,
class) is loaded at start; otherwise call the `open_project` tool. Tools cover classes and
packages, class and method source, smali, manifest and resources, strings, hierarchy and usages.
`get_class_source` returns real decompiled code (`source_kind: decompiled`) when the class
decompiles; otherwise it falls back to a declaration skeleton (`source_kind: skeleton`).

Client configuration example:

```json
{"mcpServers": {"jadx": {"command": "jadx-mcp", "args": ["/path/to/app.apk"]}}}
```

## Oracle checks

The harness in `tests/oracle/` compares output with a locally built Java jadx
(`./gradlew :jadx-cli:installDist` in a jadx checkout, pointed to by `JADX_REF`).
Test inputs are never committed.

## License

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE).
