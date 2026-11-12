//! Package identity of an APK read from its `AndroidManifest.xml` (binary or plain text).

use std::collections::HashMap;

use jadx_resources::{BinaryXmlParser, ManifestAttributes, XmlContext};

use crate::error::{ensure, PackageError, Result};

/// Attributes of the `<manifest>` element that identify a base or split APK.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitMeta {
    pub package: String,
    pub version_code: String,
    /// `split` attribute: name of a split APK, `None` for the base APK.
    pub split: Option<String>,
    /// `configForSplit` attribute: the feature split a configuration split belongs to (`""`
    /// stands for the base).
    pub config_for_split: Option<String>,
}

/// Decode `AndroidManifest.xml` bytes (binary XML or plain text) and read the identity.
pub fn parse_manifest_meta(bytes: &[u8]) -> Result<SplitMeta> {
    let res_names = HashMap::new();
    let attrs = ManifestAttributes::new();
    let mut parser = BinaryXmlParser::new(XmlContext {
        res_names: &res_names,
        attrs: &attrs,
        class_alias: None,
        skip_pretty_print: true,
    });
    let text = parser
        .parse(bytes)
        .map_err(|e| PackageError::invalid(format!("cannot decode AndroidManifest.xml: {e}")))?;
    manifest_meta_from_text(&text)
}

/// Read the identity from the text form of a manifest.
pub fn manifest_meta_from_text(text: &str) -> Result<SplitMeta> {
    let start = text
        .match_indices("<manifest")
        .find(|(i, _)| {
            text[i + "<manifest".len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
        })
        .map(|(i, _)| i)
        .ok_or_else(|| PackageError::invalid("APK manifest has no manifest element"))?;
    let tag = &text[start + "<manifest".len()..];
    let mut meta = SplitMeta::default();
    let mut found_package = false;
    let mut rest = tag;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with('>') || rest.starts_with("/>") {
            break;
        }
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let body = &after[1..];
        let Some(end) = body.find(quote) else { break };
        let value = unescape(&body[..end]);
        rest = &body[end + 1..];
        match name {
            "package" => {
                meta.package = value;
                found_package = true;
            }
            "android:versionCode" | "versionCode" => meta.version_code = value,
            "split" => meta.split = Some(value),
            "android:configForSplit" | "configForSplit" => meta.config_for_split = Some(value),
            _ => {}
        }
    }
    ensure!(found_package, "APK manifest has no package identity");
    Ok(meta)
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_manifest() {
        let m = parse_manifest_meta(
            br#"<manifest package="sample.test" android:versionCode="7" split="config.en" android:configForSplit="feat"/>"#,
        )
        .unwrap();
        assert_eq!(m.package, "sample.test");
        assert_eq!(m.version_code, "7");
        assert_eq!(m.split.as_deref(), Some("config.en"));
        assert_eq!(m.config_for_split.as_deref(), Some("feat"));
    }

    #[test]
    fn rejects_missing_identity() {
        assert!(parse_manifest_meta(b"<application/>").is_err());
        assert!(parse_manifest_meta(b"<manifest/>").is_err());
        // a longer element name must not be taken for the manifest element
        assert!(manifest_meta_from_text(r#"<manifestx package="a"/>"#).is_err());
    }
}
