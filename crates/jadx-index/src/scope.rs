//! Package scope filter: prefix lists and the `obfuscated-root` pseudo prefix for short
//! R8/ProGuard package names.
//!
//! Adapted from RDX (Apache-2.0, Ch0pin and RDX contributors, `src/package_scope.rs`).
//! Modifications: plain string input instead of JSON values, own error type.

use crate::error::{IndexError, Result};

/// Pseudo prefix selecting classes that live outside any readable package root.
pub const OBFUSCATED_ROOT: &str = "obfuscated-root";
/// Longest package segment still treated as an obfuscated name.
pub const OBFUSCATED_SEGMENT_CHARS: usize = 3;
const MAX_ENTRIES: usize = 64;

/// Zero or more package/class prefixes matched at `.` boundaries, optionally joined by the
/// obfuscated-root rule. Entries combine with OR; an empty scope is unrestricted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageScope {
    prefixes: Vec<String>,
    obfuscated_root: bool,
}

impl PackageScope {
    /// Unrestricted scope.
    pub fn all() -> Self {
        Self::default()
    }

    /// Parse prefixes. Trailing dots are ignored and duplicates merge; an empty list is
    /// unrestricted.
    pub fn parse<S: AsRef<str>>(entries: &[S]) -> Result<Self> {
        if entries.len() > MAX_ENTRIES {
            return Err(IndexError::Invalid(format!(
                "a package scope accepts at most {MAX_ENTRIES} entries"
            )));
        }
        let mut scope = Self::default();
        for entry in entries {
            let entry = entry.as_ref();
            if entry == OBFUSCATED_ROOT {
                scope.obfuscated_root = true;
                continue;
            }
            let prefix = entry.trim_end_matches('.');
            if prefix.is_empty() {
                continue;
            }
            if prefix.contains(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';')) {
                return Err(IndexError::Invalid(format!(
                    "scope entry {entry:?} is not a dotted package or class name; pass several prefixes as a list"
                )));
            }
            if !scope.prefixes.iter().any(|known| known == prefix) {
                scope.prefixes.push(prefix.to_owned());
            }
        }
        Ok(scope)
    }

    pub fn is_unrestricted(&self) -> bool {
        self.prefixes.is_empty() && !self.obfuscated_root
    }

    pub fn prefixes(&self) -> &[String] {
        &self.prefixes
    }

    /// Does the dotted class name (`a.b.C$D`) belong to the scope?
    pub fn matches(&self, class: &str) -> bool {
        self.is_unrestricted()
            || self
                .prefixes
                .iter()
                .any(|prefix| prefix_matches(prefix, class))
            || (self.obfuscated_root && is_obfuscated_root(class))
    }
}

/// `prefix` matches the class itself or any class below it at a `.` boundary.
pub fn prefix_matches(prefix: &str, class: &str) -> bool {
    class == prefix
        || class
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.starts_with('.'))
}

/// Default-package classes, or every package segment at most three characters.
pub fn is_obfuscated_root(class: &str) -> bool {
    match class.rsplit_once('.') {
        None => true,
        Some((package, _)) => package.split('.').all(|segment| {
            !segment.is_empty() && segment.chars().count() <= OBFUSCATED_SEGMENT_CHARS
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obfuscated_root_rule_admits_short_roots_and_rejects_readable_packages() {
        for class in ["wr.b$a", "tk.a", "a", "a.b.C", "com.a.B", "o0.O$1", "Top"] {
            assert!(is_obfuscated_root(class), "{class}");
        }
        for class in [
            "com.example.mobile.a",
            "kotlin.a",
            "androidx.a.b.C",
            "me.leolin.X",
            "sample.Target",
        ] {
            assert!(!is_obfuscated_root(class), "{class}");
        }
    }

    #[test]
    fn prefixes_match_at_dot_boundaries() {
        let scope = PackageScope::parse(&["app."]).unwrap();
        assert!(scope.matches("app"));
        assert!(scope.matches("app.sub.B"));
        assert!(!scope.matches("application.C"));
        assert!(!scope.matches("APP.D"));
        assert!(!scope.matches("wr.b$a"));
        for empty in [vec![], vec![""]] {
            let scope = PackageScope::parse(&empty).unwrap();
            assert!(scope.is_unrestricted());
            assert!(scope.matches("anything.At.All"));
        }
    }

    #[test]
    fn lists_and_pseudo_prefix_combine_with_or() {
        let scope = PackageScope::parse(&["app.", "vendor.sdk", OBFUSCATED_ROOT, "app"]).unwrap();
        assert_eq!(scope.prefixes(), ["app", "vendor.sdk"]);
        for class in ["app.A", "vendor.sdk.Client", "wr.b$a", "tk.a", "Top"] {
            assert!(scope.matches(class), "{class}");
        }
        for class in ["vendor.sdkx.Client", "com.example.app.A", "kotlin.jvm.a"] {
            assert!(!scope.matches(class), "{class}");
        }
        let only = PackageScope::parse(&[OBFUSCATED_ROOT]).unwrap();
        assert!(only.matches("tk.a") && !only.matches("sample.Target"));
    }

    #[test]
    fn malformed_scopes_are_rejected() {
        assert!(PackageScope::parse(&["com.a, com.b"]).is_err());
        assert!(PackageScope::parse(&["com.a com.b"]).is_err());
        let many: Vec<_> = (0..65).map(|i| format!("p{i}")).collect();
        assert!(PackageScope::parse(&many).is_err());
    }
}
