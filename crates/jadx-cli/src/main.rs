//! `jadx-rs` command line front-end.
//!
//! Only developer subcommands exist so far:
//!   jadx-rs dex-info    <file>   header + table counts per dex
//!   jadx-rs dex-classes <file>   one line per class: type, flags, super, interfaces
//!   jadx-rs dex-map     <file>   map list of every dex
//!   jadx-rs smali       <file>   smali text of every class (oracle comparison format)
//!   jadx-rs dex-counts  <file>   id table counts (oracle comparison format)

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use jadx_dex_input::{load_path, DexFile, LoadOptions};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let path = args.next().map(PathBuf::from);
    let no_verify = args.any(|a| a == "--no-verify-checksum");
    let path = match (cmd.as_str(), path) {
        ("dex-info" | "dex-classes" | "dex-map" | "dex-counts" | "smali" | "dex-opcodes" | "insn-dump" | "insn-nodes" | "insn-blocks" | "insn-cfg" | "insn-ssa", Some(p)) => p,
        _ => bail!("usage: jadx-rs <dex-info|dex-classes|dex-map|dex-counts|smali|dex-opcodes|insn-dump|insn-nodes|insn-blocks|insn-cfg|insn-ssa> <file.dex|.apk|.jar> [--no-verify-checksum]"),
    };
    let opts = LoadOptions {
        verify_checksum: !no_verify,
    };
    let res = load_path(&path, &opts).with_context(|| format!("loading {}", path.display()))?;
    for w in &res.warnings {
        eprintln!("warning: {w}");
    }
    if matches!(
        cmd.as_str(),
        "insn-nodes" | "insn-blocks" | "insn-cfg" | "insn-ssa"
    ) {
        return insn_nodes(
            &res.dex_files,
            cmd == "insn-blocks",
            cmd == "insn-cfg",
            cmd == "insn-ssa",
        );
    }
    for dex in &res.dex_files {
        match cmd.as_str() {
            "dex-info" => dex_info(dex),
            "dex-map" => dex_map(dex),
            "dex-counts" => {
                let h = dex.header();
                println!(
                    "COUNTS\ttypes={}\tprotos={}\tfields={}\tmethods={}\tclasses={}",
                    h.type_ids_size,
                    h.proto_ids_size,
                    h.field_ids_size,
                    h.method_ids_size,
                    h.class_defs_size
                );
            }
            "smali" => dex_smali(dex)?,
            "dex-opcodes" => dex_opcodes(dex)?,
            "insn-dump" => insn_dump(dex)?,
            _ => dex_classes(dex)?,
        }
    }
    Ok(())
}

fn dex_info(dex: &DexFile) {
    let h = dex.header();
    println!("file: {} (offset 0x{:x})", dex.name(), dex.base_offset());
    println!("  version:  {}", h.version);
    println!("  strings:  {}", h.string_ids_size);
    println!("  types:    {}", h.type_ids_size);
    println!("  protos:   {}", h.proto_ids_size);
    println!("  fields:   {}", h.field_ids_size);
    println!("  methods:  {}", h.method_ids_size);
    println!("  classes:  {}", h.class_defs_size);
    println!("  callsites:{}", h.call_site_ids_size);
    println!("  handles:  {}", h.method_handles_size);
}

fn dex_map(dex: &DexFile) {
    println!("file: {} (offset 0x{:x})", dex.name(), dex.base_offset());
    for m in &dex.header().map {
        println!(
            "  {:<28} size={:<8} off=0x{:x}",
            m.kind_name(),
            m.size,
            m.offset
        );
    }
}

fn dex_classes(dex: &DexFile) -> Result<()> {
    for def in dex.class_defs() {
        let def = def?;
        let ty = dex.type_descriptor(def.class_idx)?;
        let sup = dex
            .opt_type_descriptor(def.superclass_idx)?
            .unwrap_or_else(|| "null".into());
        let ifaces = dex.type_list(def.interfaces_off)?.join(",");
        println!("{ty}\t0x{:x}\t{sup}\t{ifaces}", def.access_flags);
    }
    Ok(())
}

fn dex_smali(dex: &DexFile) -> Result<()> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for i in 0..dex.class_count() {
        match dex.class_smali(i) {
            Ok(text) => out.write_all(text.as_bytes())?,
            Err(e) => writeln!(out, "# error: class #{i}: {e}")?,
        }
        out.write_all(b"\n#---end-class---\n")?;
    }
    Ok(())
}

/// Histogram of opcodes used in all methods (`name count`), used to track corpus coverage.
fn dex_opcodes(dex: &DexFile) -> Result<()> {
    use std::collections::BTreeMap;
    let mut hist: BTreeMap<&'static str, u64> = BTreeMap::new();
    for def in dex.class_defs() {
        let def = def?;
        let Some(data) = dex.class_data(&def)? else {
            continue;
        };
        for m in data.direct_methods.iter().chain(&data.virtual_methods) {
            if m.code_off == 0 {
                continue;
            }
            let code = dex.code_item(m.code_off)?;
            for insn in jadx_dex_input::insns::decode_all(&code.insns)? {
                *hist.entry(insn.info.name).or_default() += 1;
            }
        }
    }
    for (name, n) in hist {
        println!("{name} {n}");
    }
    Ok(())
}

/// One line per instruction in the normalised form (oracle comparison format).
fn insn_dump(dex: &DexFile) -> Result<()> {
    use jadx_input_api::{ArrayData, InsnIndexType, Opcode, Payload};
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let ints = |v: &[i32]| {
        format!(
            "[{}]",
            v.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    for def in dex.class_defs() {
        let def = def?;
        let Some(data) = dex.class_data(&def)? else {
            continue;
        };
        for m in data.direct_methods.iter().chain(&data.virtual_methods) {
            if m.code_off == 0 {
                continue;
            }
            let code = dex.code_item(m.code_off)?;
            let mr = dex.method_ref(m.method_idx)?;
            writeln!(
                out,
                "M {}->{}({}){} regs={} units={}",
                mr.class,
                mr.name,
                mr.arg_types.concat(),
                mr.return_type,
                code.registers_size,
                code.insns.len()
            )?;
            for d in jadx_dex_input::insns::decode_insn_data(&code.insns)? {
                write!(out, "I {:x} {}", d.offset, d.opcode.name())?;
                if d.opcode == Opcode::Unknown {
                    writeln!(out, " raw={:x}", d.raw_opcode_unit)?;
                    continue;
                }
                let regs: Vec<String> = d.regs.iter().map(|r| r.to_string()).collect();
                let it = match d.index_type {
                    InsnIndexType::None => "NONE",
                    InsnIndexType::TypeRef => "TYPE_REF",
                    InsnIndexType::StringRef => "STRING_REF",
                    InsnIndexType::FieldRef => "FIELD_REF",
                    InsnIndexType::MethodRef => "METHOD_REF",
                    InsnIndexType::CallSite => "CALL_SITE",
                };
                write!(
                    out,
                    " r=[{}] l={} t={} i={} it={}",
                    regs.join(","),
                    d.literal,
                    d.target,
                    d.index,
                    it
                )?;
                match &d.payload {
                    Some(Payload::Switch(sp)) => write!(
                        out,
                        " switch keys={} targets={}",
                        ints(&sp.keys),
                        ints(&sp.targets)
                    )?,
                    Some(Payload::Array(ap)) => {
                        let data = match &ap.data {
                            ArrayData::Empty => String::new(),
                            ArrayData::Bytes(v) => format!(
                                "[{}]",
                                v.iter()
                                    .map(|b| (*b as i8).to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            ArrayData::Shorts(v) => {
                                format!(
                                    "[{}]",
                                    v.iter()
                                        .map(|x| x.to_string())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            }
                            ArrayData::Ints(v) => ints(v),
                            ArrayData::Longs(v) => {
                                format!(
                                    "[{}]",
                                    v.iter()
                                        .map(|x| x.to_string())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            }
                        };
                        write!(
                            out,
                            " array size={} elem={} data={}",
                            ap.size, ap.element_size, data
                        )?
                    }
                    None => {}
                }
                writeln!(out)?;
            }
        }
    }
    Ok(())
}

/// Decode every method with the core decoder + `ProcessInstructions` and dump the nodes
/// (oracle comparison format, see `jadx_core::dump`).
fn insn_nodes(dexes: &[DexFile], blocks: bool, analysis: bool, ssa: bool) -> Result<()> {
    use jadx_core::decode::{InsnDecoder, MethodCtx};
    use jadx_core::dump::dump_insn;
    use jadx_core::passes::process_insns::process_instructions;
    use std::io::Write;

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut classes = Vec::new();
    let mut class_dex = Vec::new();
    for (i, dex) in dexes.iter().enumerate() {
        for def in dex.class_defs() {
            classes.push(dex.class_api(&def?)?);
            class_dex.push(i);
        }
    }
    let mut root =
        jadx_core::model::node::Root::build(classes, true).map_err(|e| anyhow::anyhow!(e))?;
    jadx_core::passes::signature::process_signatures(&mut root);
    jadx_core::passes::override_methods::process_overrides(&mut root);
    let root = std::sync::Arc::new(root);
    for (ci, cls) in root.classes.iter().enumerate() {
        let dex = &dexes[class_dex[ci]];
        for m in &cls.methods {
            let Some(code_off) = m.code_offset else {
                continue;
            };
            let id = format!("{}.{}", cls.info.raw_name(), m.info.short_id());
            let code = dex.code_item(code_off)?;
            let raw = jadx_dex_input::insns::decode_insn_data(&code.insns)?;
            let ctx = MethodCtx {
                return_type: m.return_type.clone(),
                parent_class: cls.info.clone(),
                root: root.clone(),
            };
            let decoded = InsnDecoder::new(dex, &ctx).process(&raw, code.insns.len());
            if blocks || analysis || ssa {
                let decoded = match decoded {
                    Ok(d) => d,
                    Err(_) => {
                        writeln!(out, "M {id} LOAD_ERROR")?;
                        continue;
                    }
                };
                let tries = dex.tries_api(&code)?;
                let void = m.info.return_type.is_void();
                let built = if ssa {
                    let frame = jadx_core::passes::pipeline::FrameInfo {
                        regs_count: code.registers_size as usize,
                        is_static: m.access & jadx_core::model::node::ACC_STATIC != 0,
                        this_type: cls.info.arg_type().clone(),
                        arg_types: m.arg_types.clone(),
                    };
                    jadx_core::passes::pipeline::build_ssa(
                        decoded, &tries, &root.ctx, &root.clsp, void, &frame,
                    )
                } else if analysis {
                    jadx_core::passes::pipeline::build_processed_blocks(
                        decoded, &tries, &root.ctx, &root.clsp, void,
                    )
                } else {
                    jadx_core::passes::pipeline::build_blocks(decoded, &tries, &root.ctx, void)
                };
                match built {
                    Ok(cfg) => {
                        if ssa {
                            write!(out, "{}", jadx_core::dump::dump_cfg_ssa(&id, &cfg))?
                        } else {
                            write!(
                                out,
                                "{}",
                                jadx_core::dump::dump_cfg_ext(&id, &cfg, analysis)
                            )?
                        }
                    }
                    Err(_) => writeln!(out, "M {id} LOAD_ERROR")?,
                }
                continue;
            }
            let mut decoded = match decoded {
                Ok(d) => d,
                Err(_) => {
                    writeln!(out, "M {id} LOAD_ERROR")?;
                    continue;
                }
            };
            if process_instructions(&mut decoded.insns).is_err() {
                writeln!(out, "M {id} LOAD_ERROR")?;
                continue;
            }
            writeln!(out, "M {id}")?;
            for n in decoded.insns.iter().flatten() {
                writeln!(out, "{}", dump_insn(n))?;
            }
        }
    }
    Ok(())
}
