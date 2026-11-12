//! Reader for the bundletool `toc.pb` table of contents of `.apks` archives.
//!
//! Adapted from RDX (Apache-2.0, Ch0pin and RDX contributors, `src/package_container.rs`):
//! the protobuf wire reader and variant parser were rewritten without the surrounding engine and
//! with the crate's own error type.

use crate::error::{ensure, PackageError, Result};

const MAX_FIELDS: usize = 100_000;

/// One variant of the bundle: the APK paths grouped by kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocVariant {
    pub number: u64,
    /// APKs of kind `SPLIT` / `INSTANT`-less persistent split set (field 3 of the APK
    /// descriptor).
    pub split_paths: Vec<String>,
    /// Standalone (universal) APKs (field 4).
    pub standalone_paths: Vec<String>,
}

/// Decoded `toc.pb`: package name and its variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toc {
    pub package: String,
    pub variants: Vec<TocVariant>,
}

fn varint(bytes: &[u8], pos: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..=63).step_by(7) {
        let byte = *bytes
            .get(*pos)
            .ok_or_else(|| PackageError::invalid("truncated toc.pb varint"))?;
        *pos += 1;
        if shift == 63 {
            ensure!(byte <= 1, "invalid toc.pb varint overflow");
        }
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(PackageError::invalid("invalid toc.pb varint"))
}

type Field<'a> = (u32, u64, &'a [u8]);

fn take(bytes: &[u8], pos: usize, len: usize) -> Result<&[u8]> {
    let end = pos
        .checked_add(len)
        .ok_or_else(|| PackageError::invalid("toc.pb overflow"))?;
    bytes
        .get(pos..end)
        .ok_or_else(|| PackageError::invalid("truncated toc.pb field"))
}

/// All `(field number, varint value, length-delimited/fixed payload)` triples of a message.
fn fields(bytes: &[u8]) -> Result<Vec<Field<'_>>> {
    let mut pos = 0;
    let mut result: Vec<Field> = Vec::new();
    while pos < bytes.len() {
        ensure!(result.len() < MAX_FIELDS, "toc.pb field budget exceeded");
        let key = varint(bytes, &mut pos)?;
        ensure!(
            (1..=(1 << 29) - 1).contains(&(key >> 3)),
            "invalid toc.pb field number"
        );
        let number = (key >> 3) as u32;
        match key & 7 {
            0 => result.push((number, varint(bytes, &mut pos)?, &[])),
            1 => {
                result.push((number, 0, take(bytes, pos, 8)?));
                pos += 8;
            }
            2 => {
                let len = usize::try_from(varint(bytes, &mut pos)?)
                    .map_err(|_| PackageError::invalid("toc.pb overflow"))?;
                result.push((number, 0, take(bytes, pos, len)?));
                pos += len;
            }
            5 => {
                result.push((number, 0, take(bytes, pos, 4)?));
                pos += 4;
            }
            _ => return Err(PackageError::invalid("unsupported toc.pb wire type")),
        }
    }
    Ok(result)
}

fn utf8(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| PackageError::invalid("toc.pb string is not UTF-8"))
}

/// Parse a `toc.pb` document.
pub fn parse_toc(bytes: &[u8]) -> Result<Toc> {
    let mut package = String::new();
    let mut variants = Vec::new();
    for (number, _, value) in fields(bytes)? {
        match number {
            4 => package = utf8(value)?,
            1 => {
                let mut variant = TocVariant {
                    number: 0,
                    split_paths: Vec::new(),
                    standalone_paths: Vec::new(),
                };
                for (field, int, value) in fields(value)? {
                    match field {
                        3 => variant.number = int,
                        2 => {
                            for (set_field, _, set_value) in fields(value)? {
                                if set_field != 2 {
                                    continue;
                                }
                                let mut path = None;
                                let mut kind = None;
                                for (apk_field, _, apk_value) in fields(set_value)? {
                                    match apk_field {
                                        2 => path = Some(utf8(apk_value)?),
                                        3 => kind = Some(3),
                                        4 => kind = Some(4),
                                        5..=9 => kind = Some(0),
                                        _ => {}
                                    }
                                }
                                if let Some(path) = path {
                                    match kind {
                                        Some(3) => variant.split_paths.push(path),
                                        Some(4) => variant.standalone_paths.push(path),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                variants.push(variant);
            }
            _ => {}
        }
    }
    ensure!(
        !package.is_empty() && !variants.is_empty(),
        "APKS toc.pb lacks package or variants"
    );
    Ok(Toc { package, variants })
}

impl Toc {
    /// Variant used for analysis: the highest numbered variant with a persistent split set, else
    /// the highest one with exactly one standalone APK. Returns the variant and its APK paths.
    pub fn select_variant(&self) -> Result<(&TocVariant, &[String])> {
        let mut sorted: Vec<&TocVariant> = self.variants.iter().collect();
        sorted.sort_by_key(|v| std::cmp::Reverse(v.number));
        let chosen = sorted
            .iter()
            .find(|v| !v.split_paths.is_empty())
            .or_else(|| sorted.iter().find(|v| v.standalone_paths.len() == 1))
            .ok_or_else(|| {
                PackageError::invalid("APKS has no persistent split or standalone APK variant")
            })?;
        let paths = if chosen.split_paths.is_empty() {
            &chosen.standalone_paths
        } else {
            &chosen.split_paths
        };
        Ok((chosen, paths))
    }
}

/// Test helpers that encode a `toc.pb` (shared with the container tests).
#[cfg(test)]
pub(crate) mod enc {
    pub fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        while value >= 128 {
            out.push((value as u8) | 128);
            value >>= 7;
        }
        out.push(value as u8);
        out
    }
    pub fn bytes(field: u64, value: &[u8]) -> Vec<u8> {
        let mut out = varint(field << 3 | 2);
        out.extend(varint(value.len() as u64));
        out.extend(value);
        out
    }
    pub fn int(field: u64, value: u64) -> Vec<u8> {
        let mut out = varint(field << 3);
        out.extend(varint(value));
        out
    }
    pub fn variant(number: u64, paths: &[(&str, u64)]) -> Vec<u8> {
        let mut variant = int(3, number);
        let mut set = Vec::new();
        for (path, kind) in paths {
            let mut apk = bytes(2, path.as_bytes());
            apk.extend(bytes(*kind, &[]));
            set.extend(bytes(2, &apk));
        }
        variant.extend(bytes(2, &set));
        bytes(1, &variant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_highest_persistent_split_variant() {
        let mut toc = enc::bytes(4, b"sample.test");
        toc.extend(enc::variant(1, &[("old.apk", 4)]));
        toc.extend(enc::variant(3, &[("base.apk", 3), ("feature.apk", 3)]));
        toc.extend(enc::variant(9, &[("instant.apk", 5)]));
        let toc = parse_toc(&toc).unwrap();
        assert_eq!(toc.package, "sample.test");
        let (variant, paths) = toc.select_variant().unwrap();
        assert_eq!(variant.number, 3);
        assert_eq!(paths, ["base.apk", "feature.apk"]);
    }

    #[test]
    fn falls_back_to_single_standalone_apk() {
        let mut toc = enc::bytes(4, b"p");
        toc.extend(enc::variant(2, &[("universal.apk", 4)]));
        let toc = parse_toc(&toc).unwrap();
        assert_eq!(toc.select_variant().unwrap().1, ["universal.apk"]);
    }

    #[test]
    fn rejects_malformed_documents() {
        assert!(parse_toc(&[0xff]).is_err());
        assert!(parse_toc(&[]).is_err());
        // length-delimited field larger than the buffer
        assert!(parse_toc(&[0x22, 0x7f, 0x41]).is_err());
    }
}
