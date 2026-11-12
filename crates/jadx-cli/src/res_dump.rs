//! `jadx-rs res-dump <apk> <outDir>`: writes the decoded text resources of an APK (manifest,
//! binary XML files, resource tables) below `outDir`, plus `resnames.txt` with one
//! `0x%08x type/name` line per resource id. Used to compare against the reference decoder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use jadx_resources::{decode_apk, ApkOptions};

pub fn res_dump(args: &[String]) -> Result<()> {
    let (apk, out, aux_dir) = match args {
        [apk, out] => (PathBuf::from(apk), PathBuf::from(out), None),
        [apk, out, aux] => (
            PathBuf::from(apk),
            PathBuf::from(out),
            Some(PathBuf::from(aux)),
        ),
        _ => bail!("usage: jadx-rs res-dump <file.apk> <outDir> [auxDir]"),
    };
    // optional tables that normally come from the decompiled code:
    // class-aliases.txt: "<raw class name> TAB <alias name>" per line
    // code-names.txt: "0x<resource id> TAB <constant field name>" per line
    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut opts = ApkOptions::default();
    if let Some(dir) = aux_dir {
        for line in read_lines(&dir.join("class-aliases.txt"))? {
            if let Some((raw, alias)) = line.split_once('\t') {
                aliases.insert(raw.to_string(), alias.to_string());
            }
        }
        for line in read_lines(&dir.join("code-names.txt"))? {
            if let Some((id, name)) = line.split_once('\t') {
                if let Ok(id) = u32::from_str_radix(id.trim_start_matches("0x"), 16) {
                    opts.res.code_names.insert(id as i32, name.to_string());
                }
            }
        }
    }
    let lookup = |name: &str| aliases.get(name).cloned();
    if !aliases.is_empty() {
        opts.class_alias = Some(&lookup);
    }
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

fn read_lines(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text.lines().map(str::to_string).collect())
}

fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, data).with_context(|| format!("writing {}", path.display()))
}
