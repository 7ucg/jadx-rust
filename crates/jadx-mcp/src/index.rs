//! Lookup structures over a loaded project: class resolution, subtype index and a bounded
//! source cache.
//!
//! The class resolution rules (exact id, descriptor form, unique short name, ambiguity and
//! "did you mean" candidates) are adapted from the RDX MCP server (Apache-2.0, Copyright 2026
//! Ch0pin and RDX contributors). Modified: the lookup runs over [`ClassEntry`] values and also
//! accepts the dotted inner class form.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::error::ToolError;
use crate::project::{ClassEntry, Project, SourceText};

const SOURCE_CACHE_BYTES: usize = 48 * 1024 * 1024;
const MAX_CANDIDATES: usize = 20;
const MAX_CANDIDATE_BYTES: usize = 16 * 1024;

#[derive(Default)]
struct SourceCache {
    entries: HashMap<String, Arc<SourceText>>,
    bytes: usize,
}

/// A loaded project together with its lookup indexes.
pub struct ProjectHandle {
    pub project: Arc<dyn Project>,
    by_id: HashMap<String, usize>,
    by_full: HashMap<String, usize>,
    by_simple: HashMap<String, Vec<usize>>,
    children: HashMap<String, Vec<usize>>,
    cache: Mutex<SourceCache>,
}

impl ProjectHandle {
    pub fn new(project: Arc<dyn Project>) -> Self {
        let classes = project.classes();
        let mut by_id = HashMap::with_capacity(classes.len());
        let mut by_full = HashMap::new();
        let mut by_simple: HashMap<String, Vec<usize>> = HashMap::new();
        let mut children: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, c) in classes.iter().enumerate() {
            by_id.insert(c.id.clone(), i);
            if c.full_name != c.id {
                by_full.insert(c.full_name.clone(), i);
            }
            by_simple.entry(c.short_name.clone()).or_default().push(i);
            let tail = c.id.rsplit('.').next().unwrap_or(&c.id);
            if tail != c.short_name {
                by_simple.entry(tail.to_string()).or_default().push(i);
            }
            let supers = c.super_class.iter().chain(c.interfaces.iter());
            let mut seen = HashSet::new();
            for s in supers {
                if seen.insert(s.as_str()) {
                    children.entry(s.clone()).or_default().push(i);
                }
            }
        }
        ProjectHandle {
            project,
            by_id,
            by_full,
            by_simple,
            children,
            cache: Mutex::new(SourceCache::default()),
        }
    }

    pub fn class(&self, index: usize) -> &ClassEntry {
        &self.project.classes()[index]
    }

    /// Resolve a class selector to an index into [`Project::classes`].
    pub fn resolve_class(&self, requested: &str) -> Result<usize, ToolError> {
        let name = normalize_class_name(requested);
        let name = name.as_str();
        if let Some(i) = self.by_id.get(name).or_else(|| self.by_full.get(name)) {
            return Ok(*i);
        }
        if !name.contains('.') {
            if let Some(found) = self.by_simple.get(name) {
                let mut unique: Vec<usize> = found.clone();
                unique.sort_unstable();
                unique.dedup();
                if unique.len() == 1 {
                    return Ok(unique[0]);
                }
                let ids = self.candidate_ids(unique.into_iter());
                return Err(ToolError::new(
                    "AMBIGUOUS_CLASS",
                    format!("requested_class_id={name}; use a fully qualified class_id"),
                )
                .with_details(json!({"candidates": ids})));
            }
        }
        Err(self.not_found(name))
    }

    fn candidate_ids(&self, indexes: impl Iterator<Item = usize>) -> Vec<String> {
        let set: BTreeSet<&str> = indexes.map(|i| self.class(i).id.as_str()).collect();
        let mut bytes = 0;
        set.into_iter()
            .take(MAX_CANDIDATES)
            .take_while(|id| {
                bytes += id.len();
                bytes <= MAX_CANDIDATE_BYTES
            })
            .map(str::to_string)
            .collect()
    }

    fn not_found(&self, name: &str) -> ToolError {
        let simple = name.rsplit('.').next().unwrap_or(name);
        let matching: Vec<usize> = self
            .project
            .classes()
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.short_name.eq_ignore_ascii_case(simple)
                    || c.id
                        .rsplit('.')
                        .next()
                        .is_some_and(|t| t.eq_ignore_ascii_case(simple))
            })
            .map(|(i, _)| i)
            .collect();
        let total = matching.len();
        let suggestions = self.candidate_ids(matching.into_iter());
        let truncated = suggestions.len() < total;
        ToolError::new(
            "SYMBOL_NOT_FOUND",
            format!("requested_class_id={name}; no such class in the loaded project"),
        )
        .with_details(json!({
            "candidates": suggestions,
            "candidate_count": total,
            "candidates_truncated": truncated,
            "scope": "loaded classes with the same simple name ignoring case",
            "hint": "Retry with an exact candidate class_id; get_all_classes lists loaded classes",
        }))
    }

    /// Resolve a class that anchors a hierarchy query. Besides project classes, any type named as
    /// super class or interface of a project class qualifies (framework classes).
    pub fn resolve_hierarchy_name(&self, requested: &str) -> Result<String, ToolError> {
        let name = normalize_class_name(requested);
        if let Some(i) = self.by_id.get(&name).or_else(|| self.by_full.get(&name)) {
            return Ok(self.class(*i).id.clone());
        }
        if self.children.contains_key(&name) {
            return Ok(name);
        }
        self.resolve_class(requested)
            .map(|i| self.class(i).id.clone())
    }

    /// Classes that name `id` as super class or interface.
    pub fn direct_subtypes(&self, id: &str) -> &[usize] {
        self.children.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// All classes below `id` in the type hierarchy, breadth first.
    pub fn all_subtypes(&self, id: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen: HashSet<usize> = HashSet::new();
        let mut queue: Vec<String> = vec![id.to_string()];
        while let Some(current) = queue.pop() {
            for &i in self.direct_subtypes(&current) {
                if seen.insert(i) {
                    out.push(i);
                    queue.push(self.class(i).id.clone());
                }
            }
        }
        out
    }

    /// Index of a project class by id.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    /// Source of a class through a bounded cache.
    pub fn class_source(&self, index: usize) -> Result<Arc<SourceText>, ToolError> {
        let id = &self.class(index).id;
        if let Some(found) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(id)
        {
            return Ok(found.clone());
        }
        let source = Arc::new(self.project.class_source(id)?);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.bytes + source.text.len() > SOURCE_CACHE_BYTES {
            cache.entries.clear();
            cache.bytes = 0;
        }
        cache.bytes += source.text.len();
        cache.entries.insert(id.clone(), source.clone());
        Ok(source)
    }
}

/// Accept descriptors (`La/b/C;`) and slash separated names.
pub fn normalize_class_name(requested: &str) -> String {
    let trimmed = requested.trim();
    let inner = trimmed
        .strip_prefix('L')
        .and_then(|s| s.strip_suffix(';'))
        .unwrap_or(trimmed);
    if inner.contains('/') {
        inner.replace('/', ".")
    } else {
        inner.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::*;

    struct Fake(Vec<ClassEntry>, ProjectInfo);

    impl Project for Fake {
        fn info(&self) -> &ProjectInfo {
            &self.1
        }
        fn classes(&self) -> &[ClassEntry] {
            &self.0
        }
        fn methods(&self, _: &str) -> Option<Vec<MethodEntry>> {
            None
        }
        fn fields(&self, _: &str) -> Option<Vec<FieldEntry>> {
            None
        }
        fn class_source(&self, id: &str) -> Result<SourceText, ToolError> {
            Ok(SourceText {
                text: format!("class {id}"),
                kind: SourceKind::Skeleton,
            })
        }
        fn method_source(&self, _: &str, _: &str) -> Result<SourceText, ToolError> {
            Err(ToolError::unsupported("n/a"))
        }
        fn class_smali(&self, _: &str) -> Result<String, ToolError> {
            Err(ToolError::unsupported("n/a"))
        }
        fn manifest(&self) -> Result<Option<String>, ToolError> {
            Ok(None)
        }
        fn resource_files(&self) -> Result<Vec<ResourceFile>, ToolError> {
            Ok(vec![])
        }
        fn resource_text(&self, _: &str) -> Result<Option<String>, ToolError> {
            Ok(None)
        }
        fn strings(&self, _: Option<&str>) -> Result<Arc<Vec<String>>, ToolError> {
            Ok(Arc::new(vec![]))
        }
        fn string_uses(
            &self,
            _: &dyn Fn(&str) -> bool,
            _: usize,
            _: CancelCheck<'_>,
        ) -> Result<Vec<StringUse>, ToolError> {
            Ok(vec![])
        }
        fn usages(&self, _: &Symbol) -> Result<Usages, ToolError> {
            Ok(Usages::default())
        }
        fn callees(&self, _: &str) -> Result<Callees, ToolError> {
            Ok(Callees::default())
        }
    }

    fn entry(id: &str, sup: Option<&str>, ifaces: &[&str]) -> ClassEntry {
        let short = id.rsplit('.').next().unwrap().rsplit('$').next().unwrap();
        ClassEntry {
            id: id.into(),
            full_name: id.replace('$', "."),
            short_name: short.into(),
            package: id.rsplit_once('.').map(|p| p.0).unwrap_or("").into(),
            access: 1,
            super_class: sup.map(str::to_string),
            interfaces: ifaces.iter().map(|s| s.to_string()).collect(),
            is_inner: id.contains('$'),
            method_count: 0,
            field_count: 0,
        }
    }

    fn handle() -> ProjectHandle {
        let classes = vec![
            entry("p.Login", Some("android.app.Activity"), &[]),
            entry("q.Login", Some("p.Login"), &["java.lang.Runnable"]),
            entry("p.Outer$Inner", Some("java.lang.Object"), &[]),
            entry("p.Unique", Some("q.Login"), &[]),
        ];
        let info = ProjectInfo {
            kind: InputKind::Dex,
            path: String::new(),
            size_bytes: 0,
            class_count: 4,
            method_count: 0,
            field_count: 0,
            dex_count: 1,
            has_resources: false,
            has_smali: false,
            has_strings: false,
            source_kind: SourceKind::Skeleton,
            warnings: vec![],
        };
        ProjectHandle::new(Arc::new(Fake(classes, info)))
    }

    #[test]
    fn resolves_exact_descriptor_dotted_inner_and_unique_short_names() {
        let h = handle();
        assert_eq!(h.resolve_class("p.Login").unwrap(), 0);
        assert_eq!(h.resolve_class("Lq/Login;").unwrap(), 1);
        assert_eq!(h.resolve_class("p.Outer.Inner").unwrap(), 2);
        assert_eq!(h.resolve_class("Unique").unwrap(), 3);
        assert_eq!(h.resolve_class("Inner").unwrap(), 2);
    }

    #[test]
    fn ambiguous_and_missing_names_carry_candidates() {
        let h = handle();
        let err = h.resolve_class("Login").unwrap_err();
        assert_eq!(err.code, "AMBIGUOUS_CLASS");
        assert_eq!(
            err.details.unwrap()["candidates"],
            json!(["p.Login", "q.Login"])
        );
        let err = h.resolve_class("x.login").unwrap_err();
        assert_eq!(err.code, "SYMBOL_NOT_FOUND");
        let details = err.details.unwrap();
        assert_eq!(details["candidate_count"], json!(2));
        assert_eq!(details["candidates"], json!(["p.Login", "q.Login"]));
    }

    #[test]
    fn hierarchy_queries_use_declared_supertypes() {
        let h = handle();
        assert_eq!(
            h.resolve_hierarchy_name("android.app.Activity").unwrap(),
            "android.app.Activity"
        );
        assert_eq!(h.direct_subtypes("p.Login"), &[1]);
        let mut all = h.all_subtypes("p.Login");
        all.sort_unstable();
        assert_eq!(all, vec![1, 3]);
        assert!(h.resolve_hierarchy_name("no.Such").is_err());
    }

    #[test]
    fn source_cache_returns_same_allocation() {
        let h = handle();
        let a = h.class_source(0).unwrap();
        let b = h.class_source(0).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
