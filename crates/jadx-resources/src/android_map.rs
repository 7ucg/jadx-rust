//! Android framework data tables: resource id to name map and attribute enum/flag values.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

const RES_MAP: &str = include_str!("../data/res-map.txt");
const ATTRS: &str = include_str!("../data/attrs.txt");

fn res_map() -> &'static HashMap<u32, &'static str> {
    static MAP: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for line in RES_MAP.lines() {
            if line.len() < 9 {
                continue;
            }
            if let Ok(id) = u32::from_str_radix(&line[..8], 16) {
                map.insert(id, &line[9..]);
            }
        }
        map
    })
}

/// Name (`type/name`) of an Android framework resource id.
pub fn android_res_name(id: i32) -> Option<&'static str> {
    res_map().get(&(id as u32)).copied()
}

/// Kind of an attribute with named values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrKind {
    Enum,
    Flag,
}

/// Enum or flag definition of one attribute.
#[derive(Debug, Clone)]
pub struct AttrDef {
    pub kind: AttrKind,
    pub values: BTreeMap<i64, String>,
}

/// Framework attribute definitions keyed by `android:name`.
pub fn android_attrs() -> &'static HashMap<String, AttrDef> {
    static MAP: OnceLock<HashMap<String, AttrDef>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for line in ATTRS.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(name), Some(kind), Some(values)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let mut def = AttrDef {
                kind: if kind == "E" {
                    AttrKind::Enum
                } else {
                    AttrKind::Flag
                },
                values: BTreeMap::new(),
            };
            for item in values.split(' ') {
                if let Some((k, v)) = item.split_once('=') {
                    if let Ok(k) = k.parse::<i64>() {
                        def.values.insert(k, v.to_string());
                    }
                }
            }
            map.insert(name.to_string(), def);
        }
        map
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_names() {
        assert_eq!(android_res_name(0x0101_0003), Some("attr/name"));
        assert_eq!(android_res_name(0x7f01_0000), None);
    }

    #[test]
    fn framework_attrs() {
        let a = &android_attrs()["android:screenOrientation"];
        assert_eq!(a.kind, AttrKind::Enum);
        assert_eq!(a.values.get(&1).map(String::as_str), Some("portrait"));
    }
}
