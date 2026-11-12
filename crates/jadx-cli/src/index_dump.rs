//! Developer subcommand `xref <file> <query> ...`: queries of `jadx-index` on the dex files of an
//! input.
//!
//!   xref <file> stats                          index statistics
//!   xref <file> callers <Lcls;->name(args)ret> invoke sites of a method
//!   xref <file> callees <Lcls;->name(args)ret> invoke instructions of a method
//!   xref <file> field <Lcls;->name:type> [read|write]
//!   xref <file> string <text>                  exact `const-string` sites
//!   xref <file> grep <text>                    string literals containing the text
//!   xref <file> int <value>                    integer literal sites
//!   xref <file> type <Lcls;>                   sites using a class
//!   xref <file> names <exact|prefix|substring|glob|camel|word> <pattern>
//!   xref <file> graph <Lcls;->name(args)ret> [callers|callees|both] [depth]
//!   xref <file> check-usage                    compare callers with the usage pass of the core
//!
//! `--disk` keeps only 16 MiB of postings in memory and spills the rest to disk.

use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use jadx_core::model::node::{MethodId, Root};
use jadx_dex_input::{load_path, DexCodeRefs, DexFile, LoadOptions};
use jadx_index::{
    build_call_graph, display_method, parse_field_ref, parse_method_ref, Access, BuildOptions,
    DexCodeSource, Direction, GraphLimits, MatchMode, NameIndex, NameQuery, PackageScope, Query,
    Resolver, Sites, StoreConfig, UsageIndex,
};

fn load_root(path: &Path) -> Result<(Root, Vec<DexFile>, Vec<usize>)> {
    use jadx_core::passes::pre_decompile::{run_pre_decompile, PreDecompileOptions};
    let res = load_path(path, &LoadOptions::default())
        .with_context(|| format!("loading {}", path.display()))?;
    let dexes = res.dex_files;
    let mut classes = Vec::new();
    let mut class_dex = Vec::new();
    for (i, dex) in dexes.iter().enumerate() {
        for def in dex.class_defs() {
            classes.push(dex.class_api(&def?)?);
            class_dex.push(i);
        }
    }
    let mut root = Root::build(classes, true).map_err(|e| anyhow::anyhow!(e))?;
    let refs = DexCodeRefs {
        dexes: &dexes,
        class_dex: &class_dex,
    };
    run_pre_decompile(&mut root, &refs, &PreDecompileOptions::default());
    Ok((root, dexes, class_dex))
}

fn print_sites(root: &Root, sites: &Sites) {
    for s in &sites.sites {
        println!(
            "{}\t@{}\t{}",
            display_method(root, s.method_id()),
            s.offset,
            s.kind.name()
        );
    }
    if sites.truncated {
        println!("... truncated");
    }
}

fn find_method(root: &Root, text: &str) -> Result<MethodId> {
    let r = parse_method_ref(text).context("expected Lcls;->name(args)ret")?;
    Resolver::new(root)
        .resolve_method(&r)
        .context("method not found in the input")
}

pub fn run(args: &[String]) -> Result<()> {
    // `--disk`: keep only 16 MiB of postings in memory and spill the rest to disk
    let disk = args.iter().any(|a| a == "--disk");
    let args: Vec<String> = args.iter().filter(|a| *a != "--disk").cloned().collect();
    let [file, query, rest @ ..] = args.as_slice() else {
        bail!("usage: jadx-rs xref <file> <stats|callers|callees|field|string|grep|int|type|names|graph|check-usage> ...");
    };
    let (root, dexes, class_dex) = load_root(Path::new(file))?;
    let source = DexCodeSource {
        dexes: &dexes,
        class_dex: &class_dex,
    };
    let t = Instant::now();
    let opts = BuildOptions {
        store: if disk {
            StoreConfig::disk(std::env::temp_dir(), 16 << 20)
        } else {
            StoreConfig::default()
        },
        ..BuildOptions::default()
    };
    let index = UsageIndex::build(&root, &source, &opts)?;
    eprintln!(
        "index: {} classes, {} methods scanned ({} undecodable), {} sites, {} ms{}",
        root.classes.len(),
        index.stats().methods_scanned,
        index.stats().methods_undecodable,
        index.stats().sites,
        t.elapsed().as_millis(),
        if index.stats().on_disk {
            " (spilled to disk)"
        } else {
            ""
        }
    );
    let q = Query {
        limit: 200,
        ..Query::default()
    };
    let arg = |i: usize| rest.get(i).map(String::as_str);
    match query.as_str() {
        "stats" => {}
        "callers" => {
            let r = parse_method_ref(arg(0).context("method")?).context("bad method")?;
            print_sites(&root, &index.callers_of_ref(&r, &q)?);
        }
        "callees" => {
            let id = find_method(&root, arg(0).context("method")?)?;
            for c in index.callees_of(id)? {
                let target = match c.target {
                    jadx_index::CallTarget::Method(m) => display_method(&root, m),
                    jadx_index::CallTarget::External(r) => jadx_index::resolve::method_ref_text(&r),
                };
                println!("@{}\t{}\t{target}", c.offset, c.kind.name());
            }
        }
        "field" => {
            let r = parse_field_ref(arg(0).context("field")?).context("bad field")?;
            let access = match arg(1) {
                Some("read") => Access::Read,
                Some("write") => Access::Write,
                _ => Access::Any,
            };
            print_sites(&root, &index.field_sites_of_ref(&r, access, &q)?);
        }
        "string" => print_sites(&root, &index.string_sites(arg(0).context("text")?, &q)?),
        "grep" => {
            let hits = index.search_strings(
                arg(0).context("text")?,
                true,
                &PackageScope::all(),
                200,
                None,
            )?;
            for (s, v) in hits {
                println!(
                    "{}\t@{}\t{v:?}",
                    display_method(&root, s.method_id()),
                    s.offset
                );
            }
        }
        "int" => {
            let v = arg(0).context("value")?;
            let v = match v.strip_prefix("0x") {
                Some(h) => i64::from_str_radix(h, 16)?,
                None => v.parse()?,
            };
            print_sites(&root, &index.int_sites(v, &q)?);
        }
        "type" => print_sites(&root, &index.type_sites(arg(0).context("descriptor")?, &q)?),
        "names" => {
            let mode = match arg(0) {
                Some("exact") => MatchMode::Exact,
                Some("prefix") => MatchMode::Prefix,
                Some("substring") => MatchMode::Substring,
                Some("glob") => MatchMode::Glob,
                Some("camel") => MatchMode::CamelCase,
                Some("word") => MatchMode::Word,
                _ => bail!("mode: exact|prefix|substring|glob|camel|word"),
            };
            let names = NameIndex::build(&root, jadx_index::names::DEFAULT_MAX_ENTRIES)?;
            let res = names.search(&NameQuery::new(arg(1).context("pattern")?, mode));
            for h in res.hits {
                println!("{}", h.display);
            }
        }
        "graph" => {
            let id = find_method(&root, arg(0).context("method")?)?;
            let direction = arg(1).and_then(Direction::parse).unwrap_or_default();
            let limits = GraphLimits {
                depth: arg(2).map_or(Ok(3), str::parse)?,
                ..GraphLimits::default()
            };
            let g = build_call_graph(&index, id, direction, &limits, None)?;
            for (i, n) in g.nodes.iter().enumerate() {
                let comp = n.component.map(|c| c.label()).unwrap_or("");
                println!("N{i}\tdepth={}\t{}\t{comp}", n.depth, n.text);
            }
            for e in &g.edges {
                println!("E\tN{} -> N{}\tsites={}", e.from, e.to, e.sites);
            }
            if g.truncated {
                println!("... truncated");
            }
        }
        "check-usage" => check_usage(&root, &index)?,
        other => bail!("unknown query {other}"),
    }
    Ok(())
}

/// Compare the caller sets of the index with `use_in` of the core usage pass.
fn check_usage(root: &Root, index: &UsageIndex<'_>) -> Result<()> {
    let q = Query {
        limit: 100_000,
        verify: false,
        ..Query::default()
    };
    let (mut same, mut only_core, mut only_index, mut total) = (0usize, 0usize, 0usize, 0usize);
    let mut examples = Vec::new();
    for (ci, c) in root.classes.iter().enumerate() {
        for (mi, m) in c.methods.iter().enumerate() {
            total += 1;
            let mut from_index: Vec<MethodId> = index
                .callers_of((ci, mi), &q)?
                .sites
                .iter()
                .map(|s| s.method_id())
                .filter(|id| *id != (ci, mi))
                .collect();
            from_index.sort();
            from_index.dedup();
            let mut core: Vec<MethodId> = m.use_in.clone();
            core.sort();
            core.dedup();
            if from_index == core {
                same += 1;
                continue;
            }
            only_core += core.iter().filter(|x| !from_index.contains(x)).count();
            only_index += from_index.iter().filter(|x| !core.contains(x)).count();
            if examples.len() < 5 {
                examples.push(format!(
                    "{}: core={} index={}",
                    display_method(root, (ci, mi)),
                    core.len(),
                    from_index.len()
                ));
            }
        }
    }
    println!("methods: {total}, identical caller sets: {same}");
    println!("callers only in core: {only_core}, only in index: {only_index}");
    for e in examples {
        println!("  {e}");
    }
    Ok(())
}
