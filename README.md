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
  jadx-cli/         `jadx-rs` command line tool
  jadx-mcp/         MCP server (planned)
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

## Oracle checks

The harness in `tests/oracle/` compares output with a locally built Java jadx
(`./gradlew :jadx-cli:installDist` in a jadx checkout, pointed to by `JADX_REF`).
Test inputs are never committed.

## License

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE).
