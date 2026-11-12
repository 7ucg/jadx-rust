//! `jadx-rs res-dump <apk> <outDir>`: writes the decoded text resources of an APK (manifest,
//! binary XML files, resource tables) below `outDir`, plus `resnames.txt` with one
//! `0x%08x type/name` line per resource id. Used to compare against the reference decoder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use jadx_resources::{decode_apk, ApkOptions};

pub fn res_dump(args: &[String]) -> Result<()> {
    let (apk, out, alias_file) = match args {
        [apk, out] => (PathBuf::from(apk), PathBuf::from(out), None),
        [apk, out, aliases] => (
            PathBuf::from(apk),
            PathBuf::from(out),
            Some(PathBuf::from(aliases)),
        ),
        _ => bail!("usage: jadx-rs res-dump <file.apk> <outDir> [classAliasFile]"),
    };
    // optional table of renamed classes: "<raw name> TAB <alias name>" per line
    let mut aliases: HashMap<String, String> = HashMap::new();
    if let Some(f) = alias_file {
        let text =
            std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?;
        for line in text.lines() {
            if let Some((raw, alias)) = line.split_once('\t') {
                aliases.insert(raw.to_string(), alias.to_string());
            }
        }
    }
    let lookup = |name: &str| aliases.get(name).cloned();
    let opts = ApkOptions {
        class_alias: if aliases.is_empty() {
            None
        } else {
            Some(&lookup)
        },
        ..ApkOptions::default()
    };
    let res = decode_apk(&apk, &opts).with_context(|| format!("decoding {}", apk.display()))?;
    for w in &res.warnings {
        eprintln!("warning: {w}");
    }
    for f in &res.files {
        let mut text = f.text.clone();
        text.push('\n');
        write_file(&out.join(&f.path), text.as_bytes())?;
    }
    let mut names: Vec<(u32, &String)> = res
        .resource_names
        .iter()
        .map(|(id, n)| (*id as u32, n))
        .collect();
    names.sort();
    let mut list = String::new();
    for (id, name) in names {
        list.push_str(&format!("0x{id:08x} {name}\n"));
    }
    write_file(&out.join("resnames.txt"), list.as_bytes())
}

fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, data).with_context(|| format!("writing {}", path.display()))
}
