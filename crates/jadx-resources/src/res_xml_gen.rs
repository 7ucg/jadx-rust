//! Generation of `res/values*/*.xml` files and `public.xml` from a resource table.

use std::collections::{BTreeMap, HashSet};

use crate::consts::*;
use crate::manifest_attrs::ManifestAttributes;
use crate::table::{ResTable, ResourceEntry};
use crate::text::{
    escape_res_str_value, escape_res_value, has_multiple_non_positional_substitutions, CodeWriter,
};
use crate::values::{attr_type_as_string, RawNamedValue, ValuesParser};

/// Resource types that live in files, not in `values` XML.
const SKIP_RES_TYPES: [&str; 12] = [
    "anim",
    "animator",
    "font",
    "id",
    "interpolator",
    "layout",
    "menu",
    "mipmap",
    "navigation",
    "raw",
    "transition",
    "xml",
];

const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>";

/// `res/values/public.xml`: one `<public>` line per type and name.
pub fn make_public_xml(table: &ResTable) -> String {
    let mut w = CodeWriter::new();
    w.add(XML_HEADER);
    w.start_line_str("<resources>");
    w.inc_indent();
    let mut added: HashSet<String> = HashSet::new();
    for ri in table.storage().resources() {
        if added.insert(format!("{}.{}", ri.type_name, ri.key_name)) {
            w.start_line_str(&format!(
                "<public type=\"{}\" name=\"{}\" id=\"0x{:08x}\" />",
                ri.type_name, ri.key_name, ri.id as u32
            ));
        }
    }
    w.dec_indent();
    w.start_line_str("</resources>");
    w.finish()
}

/// The values XML files of the table keyed by path (`res/values-de/strings.xml`).
pub fn make_resources_xml(
    table: &ResTable,
    attrs: &ManifestAttributes,
) -> BTreeMap<String, String> {
    let names = table.resource_names();
    let vp = ValuesParser::new(table.strings(), &names);
    let mut files: BTreeMap<String, CodeWriter> = BTreeMap::new();
    for ri in table.storage().resources() {
        if SKIP_RES_TYPES.contains(&ri.type_name.as_str()) {
            continue;
        }
        let fname = get_file_name(ri);
        let cw = files.entry(fname).or_insert_with(|| {
            let mut cw = CodeWriter::new();
            cw.add(XML_HEADER);
            cw.start_line_str("<resources>");
            cw.inc_indent();
            cw
        });
        add_value(cw, ri, &vp, attrs);
    }
    files
        .into_iter()
        .map(|(name, mut cw)| {
            cw.dec_indent();
            cw.start_line_str("</resources>");
            (name, cw.finish())
        })
        .collect()
}

fn get_file_name(ri: &ResourceEntry) -> String {
    let mut s = String::from("res/values");
    s.push_str(&ri.config);
    s.push('/');
    s.push_str(&ri.type_name);
    if !ri.type_name.ends_with('s') {
        s.push('s');
    }
    s.push_str(".xml");
    s
}

fn add_value(
    cw: &mut CodeWriter,
    ri: &ResourceEntry,
    vp: &ValuesParser,
    attrs: &ManifestAttributes,
) {
    if let Some(simple) = &ri.simple_value {
        let value = vp.decode_raw(simple);
        add_simple_value(
            cw,
            &ri.type_name,
            &ri.type_name,
            Some("name"),
            Some(&ri.key_name),
            value.as_deref(),
        );
        return;
    }
    let empty: Vec<RawNamedValue> = Vec::new();
    let named = ri.named_values.as_ref().unwrap_or(&empty);
    let mut skip_named_values = false;
    cw.start_line();
    cw.add_char('<').add(&ri.type_name).add(" name=\"");
    let mut item_tag = "item";
    if ri.type_name == "attr" && !named.is_empty() {
        cw.add(&ri.key_name);
        let ty = named[0].value.data;
        if ty & ATTR_TYPE_ENUM != 0 {
            item_tag = "enum";
        } else if ty & ATTR_TYPE_FLAGS != 0 {
            item_tag = "flag";
        }
        if let Some(format) = attr_type_as_string(ty) {
            cw.add("\" format=\"").add(&format);
        }
        if named.len() > 1 {
            for rv in named {
                if rv.name_ref == ATTR_MIN {
                    cw.add("\" min=\"").add(&rv.value.data.to_string());
                    skip_named_values = true;
                }
            }
        }
    } else {
        cw.add(&ri.key_name);
    }
    if ri.type_name == "style" || ri.parent_ref != 0 {
        cw.add("\" parent=\"");
        if ri.parent_ref != 0 {
            let parent = vp.decode_value(TYPE_REFERENCE, ri.parent_ref);
            cw.add(parent.as_deref().unwrap_or("null"));
        }
    }
    cw.add("\">");
    if !skip_named_values {
        cw.inc_indent();
        for value in named {
            add_item(cw, item_tag, &ri.type_name, value, vp, attrs);
        }
        cw.dec_indent();
    }
    cw.start_line().add("</").add(&ri.type_name).add_char('>');
}

fn add_item(
    cw: &mut CodeWriter,
    item_tag: &str,
    type_name: &str,
    value: &RawNamedValue,
    vp: &ValuesParser,
    attrs: &ManifestAttributes,
) {
    let name_str = vp.decode_name_ref(value.name_ref);
    let mut value_str = vp.decode_raw(&value.value);
    let data_type = value.value.data_type;

    if type_name != "attr" {
        if data_type == TYPE_REFERENCE && value_str.as_deref().is_none_or(|v| v == "0") {
            value_str = Some("@null".to_string());
        }
        if let Some(name) = &name_str {
            if data_type == TYPE_INT_DEC {
                if let Some(int_val) = value_str.as_deref().and_then(|v| v.parse::<i32>().ok()) {
                    if let Some(new_val) = attrs.decode(&clean_attr_name(name), i64::from(int_val))
                    {
                        value_str = Some(new_val);
                    }
                }
            }
            if data_type == TYPE_INT_HEX {
                if let Some(int_val) = value_str.as_deref().and_then(java_integer_decode) {
                    if let Some(new_val) = attrs.decode(&clean_attr_name(name), i64::from(int_val))
                    {
                        value_str = Some(new_val);
                    }
                }
            }
        }
    }
    match type_name {
        "attr" => {
            if let Some(name) = &name_str {
                add_simple_value(
                    cw,
                    type_name,
                    item_tag,
                    Some(name),
                    value_str.as_deref(),
                    Some(""),
                );
            }
        }
        "style" => {
            if let Some(name) = &name_str {
                add_simple_value(
                    cw,
                    type_name,
                    item_tag,
                    Some(name),
                    Some(""),
                    value_str.as_deref(),
                );
            }
        }
        "plurals" => {
            let quantity = plural_name(value.name_ref);
            add_simple_value(
                cw,
                type_name,
                item_tag,
                Some("quantity"),
                quantity,
                value_str.as_deref(),
            );
        }
        _ => add_simple_value(cw, type_name, item_tag, None, None, value_str.as_deref()),
    }
}

fn clean_attr_name(name: &str) -> String {
    name.replace("android:", "").replace("attr.", "")
}

/// `Integer.decode` of the `0x...` text of a hex value.
fn java_integer_decode(s: &str) -> Option<i32> {
    let hex = s.strip_prefix("0x")?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    i32::try_from(v).ok()
}

fn add_simple_value(
    cw: &mut CodeWriter,
    type_name: &str,
    item_tag: &str,
    attr_name: Option<&str>,
    attr_value: Option<&str>,
    value_str: Option<&str>,
) {
    let Some(value_str) = value_str else {
        return;
    };
    if value_str.starts_with("res/") {
        // duplicated file reference
        return;
    }
    cw.start_line();
    cw.add_char('<').add(item_tag);
    if let (Some(attr_name), Some(attr_value)) = (attr_name, attr_value) {
        if type_name == "attr" {
            cw.add_char(' ')
                .add("name=\"")
                .add(&attr_name.replace("id.", ""))
                .add("\" value=\"")
                .add(attr_value)
                .add_char('"');
        } else if type_name == "style" {
            cw.add_char(' ')
                .add("name=\"")
                .add(&attr_name.replace("attr.", ""))
                .add_char('"');
        } else {
            cw.add_char(' ')
                .add(attr_name)
                .add("=\"")
                .add(attr_value)
                .add_char('"');
        }
    }
    if item_tag == "string"
        && value_str.contains('%')
        && has_multiple_non_positional_substitutions(value_str)
    {
        cw.add(" formatted=\"false\"");
    }
    if value_str.is_empty() {
        cw.add(" />");
    } else {
        cw.add_char('>');
        if item_tag == "string" || (type_name == "array" && !value_str.starts_with('@')) {
            cw.add(&escape_res_str_value(value_str));
        } else {
            cw.add(&escape_res_value(value_str));
        }
        cw.add("</").add(item_tag).add_char('>');
    }
}
