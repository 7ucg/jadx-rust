//! `insn-regions` for the stages that need the class driver: runs the class pass pipeline over the
//! whole input and dumps the region tree of every method (oracle comparison format, see
//! `tests/oracle/java/PipelineDump.java`).

use std::io::Write as _;

use anyhow::Result;
use jadx_core::decode::{InsnDecoder, MethodCtx};
use jadx_core::model::node::Root;
use jadx_core::passes::driver::{ClassDriver, CodeSource, DriverOptions, LoadedCode};
use jadx_dex_input::DexFile;

/// Method code read from DEX files.
pub(crate) struct DexCodeSource<'a> {
    pub dexes: &'a [DexFile],
    pub class_dex: &'a [usize],
}

impl CodeSource for DexCodeSource<'_> {
    fn load_code(&self, root: &Root, cls: usize, mth: usize) -> Result<LoadedCode, String> {
        let c = &root.classes[cls];
        let m = &c.methods[mth];
        let dex = &self.dexes[self.class_dex[cls]];
        let off = m.code_offset.ok_or("method without code")?;
        let code = dex.code_item(off).map_err(|e| e.to_string())?;
        let raw =
            jadx_dex_input::insns::decode_insn_data(&code.insns).map_err(|e| e.to_string())?;
        let ctx = MethodCtx {
            return_type: m.return_type.clone(),
            parent_class: c.info.clone(),
            root,
        };
        let decoded = InsnDecoder::new(dex, &ctx)
            .process(&raw, code.insns.len())
            .map_err(|e| e.to_string())?;
        let tries = dex.tries_api(&code).map_err(|e| e.to_string())?;
        Ok(LoadedCode {
            decoded,
            tries,
            regs_count: code.registers_size as usize,
        })
    }
}

pub(crate) fn insn_regions_driver(dexes: &[DexFile], stage: u32) -> Result<()> {
    let (root, class_dex) = crate::dev::build_root(dexes)?;
    let src = DexCodeSource {
        dexes,
        class_dex: &class_dex,
    };
    let opts = DriverOptions {
        stage,
        extract_finally: std::env::var("JADX_RS_FINALLY").is_ok_and(|v| v == "1"),
    };
    let mut driver = ClassDriver::new(root, &src, opts);
    driver.process_all();

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for ci in driver.root.sorted_class_order() {
        if stage >= 33 {
            write!(
                out,
                "{}",
                jadx_core::class_state_dump::dump_class_state(&driver.root, ci, &|id| {
                    driver.methods.get(&id).and_then(|s| s.cfg.as_ref())
                })
            )?;
        }
        let cls = &driver.root.classes[ci];
        for (mi, m) in cls.methods.iter().enumerate() {
            if m.code_offset.is_none() {
                continue;
            }
            let id = format!("{}.{}", cls.info.raw_name(), m.info.short_id());
            match driver.methods.get(&(ci, mi)) {
                Some(st) if st.failed.is_none() && st.cfg.is_some() => {
                    let cfg = st.cfg.as_ref().expect("cfg");
                    write!(
                        out,
                        "{}",
                        jadx_core::region_dump::dump_regions_stage(&id, cfg, stage >= 30)
                    )?;
                }
                other => {
                    if let (Some(st), true) = (other, std::env::var("JADX_RS_ERRORS").is_ok()) {
                        eprintln!("ERROR {id}: {}", st.failed.as_deref().unwrap_or("?"));
                    }
                    writeln!(out, "M {id} LOAD_ERROR")?;
                }
            }
        }
    }
    Ok(())
}
