//! Duplicate class selection of the reference root (`RootNode.fixDuplicatedClasses` and
//! `SelectFromDuplicates`): of several classes with the same name the one from the dex file
//! `classesN.dex` with the smallest `N` is kept; without such a source the first loaded class
//! wins.

use std::collections::HashMap;

/// Where a loaded class lives in an [`InputSet`](crate::InputSet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Index into `InputSet::dex_files`.
    Dex(usize),
    /// Index into `InputSet::java_classes`.
    Java(usize),
}

/// A loaded class: dotted raw name (`a.b.C$D`), input file name, location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassSource {
    pub raw_name: String,
    /// Name of the input the class came from (dex entry name, or the class path inside nested
    /// archives separated by `:`).
    pub source: String,
    pub origin: Origin,
}

/// `N` of `classesN.dex` (`classes.dex` counts as 1), `None` for other sources and for the
/// spelling `classes1.dex`.
fn classes_index(source: &str) -> Option<u32> {
    if source == "classes.dex" {
        return Some(1);
    }
    let digits = source.strip_prefix("classes")?.strip_suffix(".dex")?;
    if digits.is_empty()
        || digits.starts_with('0')
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || digits == "1"
    {
        return None;
    }
    digits.parse().ok()
}

/// Choose one class of a group of same-named classes.
fn select(group: &[&ClassSource]) -> usize {
    let mut best = 0;
    let mut best_index: Option<u32> = None;
    for (i, cls) in group.iter().enumerate() {
        let select_current = if i == 0 {
            true
        } else {
            match (classes_index(&cls.source), best_index) {
                (Some(idx), Some(best_idx)) => idx < best_idx,
                (Some(_), None) => true,
                (None, _) => false,
            }
        };
        if select_current {
            best = i;
            best_index = classes_index(&cls.source);
        }
    }
    best
}

/// Keep one class per raw name and sort the result by name (UTF-16 order like the reference).
/// Returns the kept classes and one warning per duplicated name.
pub fn select_from_duplicates(classes: Vec<ClassSource>) -> (Vec<ClassSource>, Vec<String>) {
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, c) in classes.iter().enumerate() {
        groups.entry(c.raw_name.clone()).or_default().push(i);
    }
    let mut warnings = Vec::new();
    let mut kept = Vec::with_capacity(groups.len());
    let mut names: Vec<&String> = groups.keys().collect();
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    for name in names {
        let idxs = &groups[name];
        if idxs.len() == 1 {
            kept.push(classes[idxs[0]].clone());
            continue;
        }
        let group: Vec<&ClassSource> = idxs.iter().map(|&i| &classes[i]).collect();
        let chosen = group[select(&group)];
        let mut sources: Vec<&str> = group.iter().map(|c| c.source.as_str()).collect();
        sources.sort_unstable();
        warnings.push(format!(
            "Found duplicated class: {name}, count: {}, sources:\n  {}\n Keep class with source: {}, others will be removed.",
            group.len(),
            sources.join("\n  "),
            chosen.source
        ));
        kept.push(chosen.clone());
    }
    (kept, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, source: &str, i: usize) -> ClassSource {
        ClassSource {
            raw_name: name.into(),
            source: source.into(),
            origin: Origin::Dex(i),
        }
    }

    #[test]
    fn classes_index_matches_reference_rules() {
        assert_eq!(classes_index("classes.dex"), Some(1));
        assert_eq!(classes_index("classes2.dex"), Some(2));
        assert_eq!(classes_index("classes12.dex"), Some(12));
        assert_eq!(classes_index("classes1.dex"), None);
        assert_eq!(classes_index("classes02.dex"), None);
        assert_eq!(classes_index("x.apk:classes2.dex"), None);
        assert_eq!(classes_index("classes.dexx"), None);
    }

    #[test]
    fn lowest_classes_dex_wins() {
        let (kept, warnings) = select_from_duplicates(vec![
            c("a.B", "classes3.dex", 0),
            c("a.B", "classes2.dex", 1),
            c("a.B", "other.bin", 2),
            c("a.A", "x", 3),
        ]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].raw_name, "a.A");
        assert_eq!(kept[1].origin, Origin::Dex(1));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("count: 3"));
    }

    #[test]
    fn valid_dex_name_beats_earlier_unnamed_source() {
        let (kept, _) = select_from_duplicates(vec![
            c("a.B", "assets/x.bin", 0),
            c("a.B", "classes.dex", 1),
        ]);
        assert_eq!(kept[0].origin, Origin::Dex(1));
        // without a valid dex name the first class stays
        let (kept, _) = select_from_duplicates(vec![c("a.B", "p", 0), c("a.B", "q", 1)]);
        assert_eq!(kept[0].origin, Origin::Dex(0));
    }
}
