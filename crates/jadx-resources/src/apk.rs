//! Resource access of an APK: finds the manifest, binary XML files and resource tables of the
//! archive and decodes them to text files.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::axml::{BinaryXmlParser, ClassAliasFn, XmlContext};
use crate::error::Result;
use crate::manifest_attrs::ManifestAttributes;
use crate::names::is_valid_entry_name;
use crate::res_xml_gen::{make_public_xml, make_resources_xml};
use crate::table::{ResOptions, ResTable};
use crate::values::ValuesParser;

/// Type of an archive entry, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    Manifest,
    Xml,
    Arsc,
    Other,
}

impl ResourceType {
    /// `ResourceType.getFileType` restricted to the types this crate decodes.
    pub fn from_name(name: &str) -> ResourceType {
        if name.ends_with("/resources.pb") {
            return ResourceType::Arsc;
        }
        match name.rfind('.') {
            Some(dot) => match name[dot..].to_lowercase().as_str() {
                ".xml" if name == "AndroidManifest.xml" => ResourceType::Manifest,
                ".xml" => ResourceType::Xml,
                ".arsc" => ResourceType::Arsc,
                _ => ResourceType::Other,
            },
            None => ResourceType::Other,
        }
    }
}

/// A decoded text resource.
#[derive(Debug, Clone)]
pub struct DecodedFile {
    /// Output path, relative to the resource output directory.
    pub path: String,
    pub text: String,
}

/// Decoder settings.
#[derive(Default)]
pub struct ApkOptions<'a> {
    pub res: ResOptions,
    /// Put all attributes of an XML element on one line.
    pub skip_xml_pretty_print: bool,
    /// Lookup of renamed classes for the manifest and layouts.
    pub class_alias: Option<ClassAliasFn<'a>>,
}

/// Everything decoded from an APK.
#[derive(Debug, Default)]
pub struct ApkResources {
    /// Decoded text files in processing order: the manifest, then archive order. A later file
    /// with the same path replaces an earlier one.
    pub files: Vec<DecodedFile>,
    /// Resource id to `type/name` map of the first resource table.
    pub resource_names: HashMap<i32, String>,
    /// The first resource table of the archive, if it could be parsed.
    pub table: Option<ResTable>,
    /// Problems that did not stop the decoding.
    pub warnings: Vec<String>,
}

struct Entry {
    index: usize,
    name: String,
}

/// Decodes the manifest, binary XML files and resource tables of an APK.
pub fn decode_apk(path: &Path, opts: &ApkOptions) -> Result<ApkResources> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut entries = Vec::new();
    for index in 0..zip.len() {
        let e = zip.by_index_raw(index)?;
        if e.is_dir() {
            continue;
        }
        let name = String::from_utf8_lossy(e.name_raw()).into_owned();
        if !is_valid_entry_name(&name) {
            continue;
        }
        entries.push(Entry { index, name });
    }

    let mut result = ApkResources::default();
    let read_entry = |zip: &mut zip::ZipArchive<std::fs::File>, index: usize| -> Result<Vec<u8>> {
        let mut f = zip.by_index(index)?;
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf)?;
        Ok(buf)
    };

    // resource table first: names of resources and of obfuscated files, app attributes
    let mut attrs = ManifestAttributes::new();
    let mut table_entry: Option<usize> = None;
    let mut table_data: Vec<u8> = Vec::new();
    if let Some(pos) = entries
        .iter()
        .position(|e| ResourceType::from_name(&e.name) == ResourceType::Arsc)
    {
        table_data = read_entry(&mut zip, entries[pos].index)?;
        // The names used for references are assigned before the constants of the app code are
        // known, so the code field names only apply to the tables decoded to values files.
        let mut names_opts = opts.res.clone();
        names_opts.code_names.clear();
        match ResTable::parse(&table_data, &names_opts) {
            Ok(table) => {
                result.resource_names = table.resource_names();
                attrs.set_app_attrs(table.app_attrs());
                result.table = Some(table);
                table_entry = Some(pos);
            }
            Err(e) => result
                .warnings
                .push(format!("Failed to parse resource table: {e}")),
        }
    }

    // files with a name that is the value of a resource entry get the name of the entry
    let mut display_names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    if let Some(table) = &result.table {
        let vp = ValuesParser::new(table.strings(), &result.resource_names);
        let mut by_value: HashMap<String, &crate::table::ResourceEntry> = HashMap::new();
        for re in table.storage().resources() {
            if let Some(v) = re.simple_value.as_ref().and_then(|s| vp.decode_raw(s)) {
                by_value.insert(v, re);
            }
        }
        for (i, e) in entries.iter().enumerate() {
            if let Some(re) = by_value.get(&e.name) {
                let alias = file_alias(re, &e.name);
                if alias != e.name {
                    display_names[i] = alias;
                }
            }
        }
    }

    let mut order: Vec<usize> = Vec::with_capacity(entries.len());
    if let Some(m) = display_names
        .iter()
        .position(|n| ResourceType::from_name(n) == ResourceType::Manifest)
    {
        order.push(m);
    }
    for (i, n) in display_names.iter().enumerate() {
        if ResourceType::from_name(n) != ResourceType::Manifest {
            order.push(i);
        }
    }

    let ctx = XmlContext {
        res_names: &result.resource_names,
        attrs: &attrs,
        class_alias: opts.class_alias,
        skip_pretty_print: opts.skip_xml_pretty_print,
    };
    let mut xml = BinaryXmlParser::new(ctx);
    let mut files: Vec<DecodedFile> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for i in order {
        let name = &display_names[i];
        match ResourceType::from_name(name) {
            ResourceType::Manifest | ResourceType::Xml => {
                let data = read_entry(&mut zip, entries[i].index)?;
                match xml.parse(&data) {
                    Ok(text) => files.push(DecodedFile {
                        path: name.clone(),
                        text,
                    }),
                    Err(e) => warnings.push(format!("Error decode {name}: {e}")),
                }
            }
            ResourceType::Arsc => {
                let parsed;
                let reuse = table_entry == Some(i) && opts.res.code_names.is_empty();
                let table: &ResTable = if reuse {
                    result.table.as_ref().expect("table parsed")
                } else {
                    let data = if table_entry == Some(i) {
                        table_data.clone()
                    } else {
                        read_entry(&mut zip, entries[i].index)?
                    };
                    match ResTable::parse(&data, &opts.res) {
                        Ok(t) => {
                            parsed = t;
                            &parsed
                        }
                        Err(e) => {
                            warnings.push(format!("Error decode {name}: {e}"));
                            continue;
                        }
                    }
                };
                files.push(DecodedFile {
                    path: "res/values/public.xml".to_string(),
                    text: make_public_xml(table),
                });
                for (p, text) in make_resources_xml(table, &attrs) {
                    files.push(DecodedFile { path: p, text });
                }
            }
            ResourceType::Other => {}
        }
    }
    result.files = files;
    result.warnings.extend(warnings);
    Ok(result)
}

/// Reads and parses the first resource table (`*.arsc`) of an APK without decoding anything else.
///
/// This is the entry point for the resource id to name map:
/// `table.resource_names()` gives `id -> "type/name"` (as used for references in decoded XML) and
/// `table.r_fields()` the `R.<type>.<name> = id` constants to add to the app `R` class.
/// Returns `Ok(None)` if the archive has no resource table.
pub fn read_resource_table(path: &Path, opts: &ResOptions) -> Result<Option<ResTable>> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for index in 0..zip.len() {
        let name = String::from_utf8_lossy(zip.by_index_raw(index)?.name_raw()).into_owned();
        if is_valid_entry_name(&name) && ResourceType::from_name(&name) == ResourceType::Arsc {
            let mut f = zip.by_index(index)?;
            let mut buf = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut buf)?;
            return ResTable::parse(&buf, opts).map(Some);
        }
    }
    Ok(None)
}

/// Output names of the files that the resource table refers to by their original name
/// (obfuscated or shrunk resource file names): original name to `res/<type><config>/<key><ext>`.
pub fn file_alias_map(table: &ResTable) -> HashMap<String, String> {
    let names = table.resource_names();
    let vp = ValuesParser::new(table.strings(), &names);
    let mut by_value: HashMap<String, &crate::table::ResourceEntry> = HashMap::new();
    for re in table.storage().resources() {
        if let Some(v) = re.simple_value.as_ref().and_then(|s| vp.decode_raw(s)) {
            by_value.insert(v, re);
        }
    }
    by_value
        .into_iter()
        .map(|(name, re)| {
            let alias = file_alias(re, &name);
            (name, alias)
        })
        .collect()
}

/// New name of a resource file that is referenced by a resource entry
/// (`res/<type><config>/<key><ext>`).
fn file_alias(re: &crate::table::ResourceEntry, name: &str) -> String {
    let mut s = String::from("res/");
    s.push_str(&re.type_name);
    s.push_str(&re.config);
    s.push('/');
    s.push_str(&re.key_name);
    s.push_str(&ext_from_name(name));
    if !is_safe_output_path(&s) {
        // never produce a path leaving the output directory (names come from the input file)
        return name.to_string();
    }
    s
}

/// Relative output path without parent components, absolute roots, backslashes or NUL.
pub fn is_safe_output_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && !path.contains('\\')
        && std::path::Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn ext_from_name(name: &str) -> String {
    // the .9.png extension is always kept
    if name.contains(".9.png") {
        return ".9.png".to_string();
    }
    match name.rfind('.') {
        Some(p) => name[p..].to_string(),
        None => String::new(),
    }
}
