//! Enum and flag attribute decoding for binary XML values (`ManifestAttributes`).

use std::collections::HashMap;

use crate::android_map::{android_attrs, AttrDef, AttrKind};

/// Framework attribute table plus the enum/flag attributes the app itself declares.
#[derive(Debug, Default, Clone)]
pub struct ManifestAttributes {
    app_attrs: HashMap<String, AttrDef>,
}

impl ManifestAttributes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the app attribute definitions (taken from the resource table).
    pub fn set_app_attrs(&mut self, attrs: HashMap<String, AttrDef>) {
        self.app_attrs = attrs;
    }

    /// Symbolic text of an enum value or flag set; `None` if the attribute has no such table or
    /// the enum value is unknown.
    pub fn decode(&self, attr_name: &str, value: i64) -> Option<String> {
        let attr = match android_attrs().get(attr_name) {
            Some(a) => a,
            None => {
                let name = match attr_name.split_once(':') {
                    Some((_, rest)) => rest,
                    None => attr_name,
                };
                self.app_attrs.get(name)?
            }
        };
        match attr.kind {
            AttrKind::Enum => attr.values.get(&value).cloned(),
            AttrKind::Flag => {
                let mut value = value;
                let mut flags: Vec<&str> = Vec::new();
                for (key, name) in attr.values.iter().rev() {
                    let key = *key;
                    if value == key {
                        flags.push(name);
                        break;
                    } else if key != 0 && (value & key) == key {
                        flags.push(name);
                        value ^= key;
                    }
                }
                Some(flags.join("|"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_and_flags() {
        let m = ManifestAttributes::new();
        assert_eq!(
            m.decode("android:screenOrientation", 1).as_deref(),
            Some("portrait")
        );
        assert_eq!(m.decode("android:screenOrientation", 9999), None);
        assert_eq!(m.decode("android:name", 1), None);
        // configChanges is a flag attribute
        let v = m
            .decode("android:configChanges", 0x80 | 0x400)
            .expect("flags");
        assert!(v.contains('|'));
    }
}
