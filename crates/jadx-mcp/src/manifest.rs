//! Structured summary of a decoded `AndroidManifest.xml`.
//!
//! Only a small XML element scanner is needed: the manifest decoder emits well formed text.

use serde_json::{json, Value};

/// An XML element with its attributes and child elements; text content is ignored.
#[derive(Debug, Default, Clone)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name || k.rsplit(':').next() == Some(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parse the first root element of an XML document.
pub fn parse(text: &str) -> Option<Element> {
    let bytes = text.as_bytes();
    let mut pos = 0;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    while pos < bytes.len() {
        let Some(lt) = text[pos..].find('<') else {
            break;
        };
        pos += lt;
        let rest = &text[pos..];
        if rest.starts_with("<!--") {
            pos += rest.find("-->").map(|e| e + 3)?;
            continue;
        }
        if rest.starts_with("<?") {
            pos += rest.find("?>").map(|e| e + 2)?;
            continue;
        }
        if rest.starts_with("<!") {
            pos += rest.find('>').map(|e| e + 1)?;
            continue;
        }
        if let Some(closing) = rest.strip_prefix("</") {
            let end = closing.find('>')?;
            pos += 2 + end + 1;
            let done = stack.pop()?;
            match stack.last_mut() {
                Some(parent) => parent.children.push(done),
                None => {
                    root = Some(done);
                    break;
                }
            }
            continue;
        }
        let (element, consumed, self_closing) = parse_start_tag(rest)?;
        pos += consumed;
        if self_closing {
            match stack.last_mut() {
                Some(parent) => parent.children.push(element),
                None => {
                    root = Some(element);
                    break;
                }
            }
        } else {
            stack.push(element);
        }
    }
    root
}

/// Parse `<name attr="v" ...>` or `<name .../>`; returns the element, bytes consumed and whether
/// the tag is self closing.
fn parse_start_tag(rest: &str) -> Option<(Element, usize, bool)> {
    let bytes = rest.as_bytes();
    let mut i = 1;
    let name_start = i;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/'
    {
        i += 1;
    }
    let mut element = Element {
        name: rest[name_start..i].to_string(),
        ..Element::default()
    };
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        match bytes.get(i)? {
            b'>' => return Some((element, i + 1, false)),
            b'/' => {
                if bytes.get(i + 1) == Some(&b'>') {
                    return Some((element, i + 2, true));
                }
                return None;
            }
            _ => {}
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = rest[key_start..i].to_string();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = *bytes.get(i)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        let value = unescape(rest.get(value_start..i)?);
        i += 1;
        element.attrs.push((key, value));
    }
}

const COMPONENT_TYPES: [&str; 5] = [
    "activity",
    "activity-alias",
    "service",
    "receiver",
    "provider",
];

fn flag(element: &Element, name: &str) -> Value {
    match element.attr(name) {
        Some("true") => json!(true),
        Some("false") => json!(false),
        Some(other) => json!(other),
        None => Value::Null,
    }
}

fn intent_filters(component: &Element) -> Vec<Value> {
    component
        .children_named("intent-filter")
        .map(|f| {
            let names = |tag: &str| -> Vec<String> {
                f.children_named(tag)
                    .filter_map(|e| e.attr("name").map(str::to_string))
                    .collect()
            };
            let data: Vec<Value> = f
                .children_named("data")
                .map(|d| {
                    let mut o = serde_json::Map::new();
                    for key in [
                        "scheme",
                        "host",
                        "port",
                        "path",
                        "pathPrefix",
                        "pathPattern",
                        "mimeType",
                    ] {
                        if let Some(v) = d.attr(key) {
                            o.insert(key.to_string(), json!(v));
                        }
                    }
                    Value::Object(o)
                })
                .collect();
            json!({"actions": names("action"), "categories": names("category"), "data": data})
        })
        .collect()
}

fn component_json(kind: &str, element: &Element) -> Value {
    let filters = intent_filters(element);
    let launcher = filters.iter().any(|f| {
        f["actions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x == "android.intent.action.MAIN"))
            && f["categories"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == "android.intent.category.LAUNCHER"))
    });
    json!({
        "type": kind,
        "name": element.attr("name"),
        "exported": flag(element, "exported"),
        "has_intent_filter": !filters.is_empty(),
        "enabled": flag(element, "enabled"),
        "permission": element.attr("permission"),
        "process": element.attr("process"),
        "authorities": element.attr("authorities"),
        "target_activity": element.attr("targetActivity"),
        "launcher": launcher,
        "intent_filters": filters,
    })
}

/// Options narrowing the component list of [`summary`].
#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub component_type: Option<String>,
    /// `Some(true)`: declared `exported="true"` or no declaration but an intent filter.
    pub exported: Option<bool>,
}

/// Summary of a manifest: package, versions, SDK levels, permissions, application flags and
/// components.
pub fn summary(text: &str, filter: &Filter) -> Option<Value> {
    let root = parse(text)?;
    if root.name != "manifest" {
        return None;
    }
    let sdk = root.children_named("uses-sdk").next();
    let named = |tag: &str| -> Vec<Value> {
        root.children_named(tag)
            .map(|e| {
                json!({
                    "name": e.attr("name"),
                    "protection_level": e.attr("protectionLevel"),
                    "max_sdk_version": e.attr("maxSdkVersion"),
                })
            })
            .collect()
    };
    let application = root.children_named("application").next();
    let mut components = Vec::new();
    if let Some(app) = application {
        for child in &app.children {
            if COMPONENT_TYPES.contains(&child.name.as_str()) {
                let value = component_json(&child.name, child);
                if let Some(kind) = &filter.component_type {
                    if value["type"] != json!(kind) {
                        continue;
                    }
                }
                if let Some(want) = filter.exported {
                    let effective = match &value["exported"] {
                        Value::Bool(b) => *b,
                        _ => value["has_intent_filter"] == json!(true),
                    };
                    if effective != want {
                        continue;
                    }
                }
                components.push(value);
            }
        }
    }
    let application_json = application.map(|app| {
        json!({
            "name": app.attr("name"),
            "label": app.attr("label"),
            "debuggable": flag(app, "debuggable"),
            "allow_backup": flag(app, "allowBackup"),
            "uses_cleartext_traffic": flag(app, "usesCleartextTraffic"),
            "network_security_config": app.attr("networkSecurityConfig"),
            "extract_native_libs": flag(app, "extractNativeLibs"),
        })
    });
    Some(json!({
        "package": root.attr("package"),
        "version_code": root.attr("versionCode"),
        "version_name": root.attr("versionName"),
        "min_sdk": sdk.and_then(|s| s.attr("minSdkVersion")),
        "target_sdk": sdk.and_then(|s| s.attr("targetSdkVersion")),
        "uses_permissions": root
            .children_named("uses-permission")
            .chain(root.children_named("uses-permission-sdk-23"))
            .filter_map(|e| e.attr("name"))
            .collect::<Vec<_>>(),
        "declared_permissions": named("permission"),
        "uses_features": root
            .children_named("uses-feature")
            .filter_map(|e| e.attr("name"))
            .collect::<Vec<_>>(),
        "application": application_json,
        "component_count": components.len(),
        "components": components,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" android:versionCode="3" android:versionName="1.2" package="p.q">
    <uses-sdk android:minSdkVersion="21" android:targetSdkVersion="33" />
    <uses-permission android:name="android.permission.INTERNET" />
    <!-- a comment <with> markup -->
    <application android:name=".App" android:debuggable="true" android:label="a &amp; b">
        <activity android:name=".Main" android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
        <service android:name=".Svc" android:exported="false" />
        <receiver android:name=".Rcv">
            <intent-filter><action android:name="x.y.Z" /></intent-filter>
        </receiver>
    </application>
</manifest>"#;

    #[test]
    fn parses_elements_and_attributes() {
        let root = parse(MANIFEST).unwrap();
        assert_eq!(root.name, "manifest");
        assert_eq!(root.attr("package"), Some("p.q"));
        let app = root.children_named("application").next().unwrap();
        assert_eq!(app.attr("label"), Some("a & b"));
        assert_eq!(app.children.len(), 3);
    }

    #[test]
    fn summary_reports_components_and_filters() {
        let s = summary(MANIFEST, &Filter::default()).unwrap();
        assert_eq!(s["package"], json!("p.q"));
        assert_eq!(s["min_sdk"], json!("21"));
        assert_eq!(
            s["uses_permissions"],
            json!(["android.permission.INTERNET"])
        );
        assert_eq!(s["component_count"], json!(3));
        assert_eq!(s["components"][0]["launcher"], json!(true));
        assert_eq!(s["application"]["debuggable"], json!(true));
        let exported = summary(
            MANIFEST,
            &Filter {
                exported: Some(true),
                ..Filter::default()
            },
        )
        .unwrap();
        // the receiver has no declaration but an intent filter
        assert_eq!(exported["component_count"], json!(2));
        let services = summary(
            MANIFEST,
            &Filter {
                component_type: Some("service".into()),
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(services["component_count"], json!(1));
    }

    #[test]
    fn malformed_input_is_none() {
        assert!(parse("<a><b></a>").is_none());
        assert!(parse("<a").is_none());
        assert!(summary("<other/>", &Filter::default()).is_none());
        assert!(parse("<a b=c/>").is_none());
    }
}
