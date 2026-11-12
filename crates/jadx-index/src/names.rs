//! Name search over the classes, methods and fields of a root.
//!
//! Inspired by the identifier and source search of RDX (Apache-2.0, Ch0pin and RDX
//! contributors: `src/identifier_search.rs`, `src/search.rs`); this index covers declared names
//! only and is kept in memory with a hard entry bound.

use jadx_core::model::node::Root;

use crate::error::{IndexError, Result};
use crate::resolve::{display_field, display_method};
use crate::scope::PackageScope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NameKind {
    Class,
    Method,
    Field,
}

/// How the pattern is compared with a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    Exact,
    Prefix,
    Substring,
    /// `*` matches any run of characters, `?` one character; the whole name must match.
    Glob,
    /// The pattern is a prefix of the initials of the camel case words (`NPE` finds
    /// `NullPointerException`); case sensitive.
    CamelCase,
    /// A word of the name (split at case changes, digits, `_`, `$` and `.`) equals the pattern.
    Word,
}

#[derive(Debug, Clone)]
pub struct NameQuery {
    pub pattern: String,
    pub mode: MatchMode,
    /// Kinds to search; empty means all.
    pub kinds: Vec<NameKind>,
    pub case_sensitive: bool,
    pub scope: PackageScope,
    pub limit: usize,
}

impl NameQuery {
    pub fn new(pattern: impl Into<String>, mode: MatchMode) -> Self {
        NameQuery {
            pattern: pattern.into(),
            mode,
            kinds: Vec::new(),
            case_sensitive: false,
            scope: PackageScope::all(),
            limit: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameHit {
    pub kind: NameKind,
    pub class: usize,
    /// Method or field index inside the class, `None` for classes.
    pub member: Option<usize>,
    /// The simple name that matched.
    pub name: String,
    /// Display text (`pkg.Cls`, `pkg.Cls.method(args):ret`, `pkg.Cls.field:type`).
    pub display: String,
}

#[derive(Debug, Default)]
pub struct NameResults {
    pub hits: Vec<NameHit>,
    pub truncated: bool,
}

struct Entry {
    kind: NameKind,
    class: u32,
    member: u32,
    name: Box<str>,
    lower: Box<str>,
}

/// In-memory index of all declared names.
pub struct NameIndex<'a> {
    root: &'a Root,
    entries: Vec<Entry>,
}

/// Default bound of [`NameIndex::build`].
pub const DEFAULT_MAX_ENTRIES: usize = 4_000_000;

impl<'a> NameIndex<'a> {
    /// Index all class, method and field names; fails when there are more than `max_entries`.
    pub fn build(root: &'a Root, max_entries: usize) -> Result<NameIndex<'a>> {
        let total: usize = root
            .classes
            .iter()
            .map(|c| 1 + c.methods.len() + c.fields.len())
            .sum();
        if total > max_entries {
            return Err(IndexError::Limit(format!(
                "{total} names exceed the name index bound of {max_entries}"
            )));
        }
        let mut entries = Vec::with_capacity(total);
        let mut push = |kind: NameKind, class: usize, member: usize, name: &str| {
            entries.push(Entry {
                kind,
                class: class as u32,
                member: member as u32,
                lower: name.to_lowercase().into_boxed_str(),
                name: name.into(),
            });
        };
        for (ci, c) in root.classes.iter().enumerate() {
            let raw = c.info.raw_name();
            let simple = raw.rsplit('.').next().unwrap_or(&raw);
            push(NameKind::Class, ci, 0, simple);
            for (mi, m) in c.methods.iter().enumerate() {
                push(NameKind::Method, ci, mi, &m.info.name);
            }
            for (fi, f) in c.fields.iter().enumerate() {
                push(NameKind::Field, ci, fi, &f.info.name);
            }
        }
        Ok(NameIndex { root, entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Search names. Hits come in root class order (classes before their members).
    pub fn search(&self, q: &NameQuery) -> NameResults {
        let pattern: String = if q.case_sensitive {
            q.pattern.clone()
        } else {
            q.pattern.to_lowercase()
        };
        let glob: Vec<char> = pattern.chars().collect();
        let mut res = NameResults::default();
        let mut scope_cache: Option<(u32, bool)> = None;
        for e in &self.entries {
            if !q.kinds.is_empty() && !q.kinds.contains(&e.kind) {
                continue;
            }
            let hay: &str = if q.case_sensitive { &e.name } else { &e.lower };
            let ok = match q.mode {
                MatchMode::Exact => hay == pattern,
                MatchMode::Prefix => hay.starts_with(&pattern),
                MatchMode::Substring => hay.contains(&pattern),
                MatchMode::Glob => glob_match(&glob, &hay.chars().collect::<Vec<_>>()),
                MatchMode::CamelCase => camel_initials(&e.name).starts_with(&q.pattern),
                MatchMode::Word => words(&e.name).iter().any(|w| {
                    if q.case_sensitive {
                        *w == q.pattern
                    } else {
                        w.to_lowercase() == pattern
                    }
                }),
            };
            if !ok {
                continue;
            }
            if !q.scope.is_unrestricted() {
                let in_scope = match scope_cache {
                    Some((c, v)) if c == e.class => v,
                    _ => {
                        let v = q
                            .scope
                            .matches(&self.root.classes[e.class as usize].info.raw_name());
                        scope_cache = Some((e.class, v));
                        v
                    }
                };
                if !in_scope {
                    continue;
                }
            }
            if res.hits.len() >= q.limit {
                res.truncated = true;
                break;
            }
            let (class, member) = (e.class as usize, e.member as usize);
            let display = match e.kind {
                NameKind::Class => self.root.classes[class].info.raw_name(),
                NameKind::Method => display_method(self.root, (class, member)),
                NameKind::Field => display_field(self.root, (class, member)),
            };
            res.hits.push(NameHit {
                kind: e.kind,
                class,
                member: (e.kind != NameKind::Class).then_some(member),
                name: e.name.to_string(),
                display,
            });
        }
        res
    }
}

/// `*`/`?` wildcard match of the whole text.
fn glob_match(pat: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while t < text.len() {
        if p < pat.len() && (pat[p] == '?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == '*' {
        p += 1;
    }
    p == pat.len()
}

/// Words of an identifier: runs of lower case letters (with a leading capital), runs of capitals
/// (acronyms), digit runs; `_`, `$`, `.` and other characters separate.
pub fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let s = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            out.push(chars[s..i].iter().collect());
        } else if c.is_uppercase() {
            let s = i;
            i += 1;
            if i < chars.len() && chars[i].is_lowercase() {
                while i < chars.len() && chars[i].is_lowercase() {
                    i += 1;
                }
            } else {
                // acronym: capitals up to a capital that starts a lower case word
                while i < chars.len()
                    && chars[i].is_uppercase()
                    && !(i + 1 < chars.len() && chars[i + 1].is_lowercase())
                {
                    i += 1;
                }
            }
            out.push(chars[s..i].iter().collect());
        } else if c.is_alphabetic() {
            let s = i;
            while i < chars.len() && chars[i].is_lowercase() {
                i += 1;
            }
            if i == s {
                i += 1;
            }
            out.push(chars[s..i].iter().collect());
        } else {
            i += 1;
        }
    }
    out
}

/// First characters of the words of an identifier (`NullPointerException` -> `NPE`).
pub fn camel_initials(name: &str) -> String {
    words(name)
        .iter()
        .filter_map(|w| w.chars().next())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(p: &str, t: &str) -> bool {
        glob_match(
            &p.chars().collect::<Vec<_>>(),
            &t.chars().collect::<Vec<_>>(),
        )
    }

    #[test]
    fn glob() {
        assert!(g("*", ""));
        assert!(g("on*Create", "onActivityCreate"));
        assert!(g("a?c", "abc"));
        assert!(!g("a?c", "ac"));
        assert!(g("*a*b*", "xxaxxbxx"));
        assert!(!g("a*b", "ac"));
        assert!(g("", ""));
        assert!(!g("", "a"));
    }

    #[test]
    fn word_splitting() {
        assert_eq!(
            words("NullPointerException"),
            ["Null", "Pointer", "Exception"]
        );
        assert_eq!(words("getHTTPResponse2"), ["get", "HTTP", "Response", "2"]);
        assert_eq!(words("my_field$Inner"), ["my", "field", "Inner"]);
        assert_eq!(camel_initials("NullPointerException"), "NPE");
        assert_eq!(camel_initials("a"), "a");
    }
}
