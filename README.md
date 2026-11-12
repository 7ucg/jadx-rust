# jadx-rust

`jadx-rs`: a Rust reimplementation of the [jadx](https://github.com/skylot/jadx) decompiler
engine (Android DEX / Java bytecode to Java source). Work in progress, built bottom-up and
verified against the reference Java implementation at every stage.

## Status

Work in progress. See [CHANGELOG.md](CHANGELOG.md) for what has shipped and how it was verified.

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

## Build

```
cargo build --release
cargo test
```

## Usage (developer commands, the full CLI is not implemented yet)

```
jadx-rs smali       app.apk   # smali text of every class
jadx-rs dex-info    app.apk   # table counts
jadx-rs dex-classes app.apk   # class list
```

## MCP server

`jadx-rs mcp [--socket PATH] [INPUT]` (or the `jadx-mcp` binary) runs a Model Context Protocol
server over stdio, or over a Unix domain socket with `--socket`. `INPUT` (apk, xapk, dex, jar,
class) is loaded at start; otherwise call the `open_project` tool. Tools cover classes and
packages, class and method source, smali, manifest and resources, strings, hierarchy and usages.
Class source is currently a declaration skeleton (`source_kind: skeleton`), see the crate docs.

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
