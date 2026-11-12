//! Instruction level cross reference index.
//!
//! [`UsageIndex::build`] scans the code of every method of a [`Root`] once and stores a posting
//! per cross reference site (invokes, field reads and writes, string and integer literals, type
//! operands) in a bounded [`PostingStore`]. Queries look a fingerprint up and verify every hit
//! against the decoded code, so fingerprint collisions never produce wrong results.
//!
//! Method and field references are canonicalised to their declaration (see
//! [`Resolver`]), so `callers_of(Base.run)` also finds `invoke-virtual Sub.run`.
//!
//! The overall approach (one pass over all methods, fingerprint keyed postings, verification
//! against the original code, bounded scans with cancellation) is adapted from RDX (Apache-2.0,
//! Ch0pin and RDX contributors: `src/usage_index.rs`, `src/invoke_index.rs`,
//! `src/field_references.rs`, `src/integer_references.rs`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

use jadx_core::model::node::{MethodId, Root};
use jadx_input_api::{FieldRefData, MethodRefData};

use crate::error::{IndexError, Result};
use crate::postings::{fingerprint, PostingStore, PostingStoreBuilder, Record, StoreConfig};
use crate::resolve::{field_ref_text, method_ref_text, FieldId, Resolver};
use crate::scope::PackageScope;
use crate::source::{CodeSource, Insn, Operand, SiteKind};

const GROUP_METHOD: u8 = 1;
const GROUP_FIELD: u8 = 2;
const GROUP_STRING: u8 = 3;
const GROUP_INT: u8 = 4;
const GROUP_TYPE: u8 = 5;

/// What a posting is looked up by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Needle {
    /// Canonical method text (`Lcls;->name(args)ret`).
    Method(String),
    /// Canonical field text (`Lcls;->name:type`).
    Field(String),
    Str(String),
    Int(i64),
    /// Type descriptor.
    Type(String),
}

impl Needle {
    fn key(&self) -> u64 {
        match self {
            Needle::Method(t) => fingerprint(GROUP_METHOD, t.as_bytes()),
            Needle::Field(t) => fingerprint(GROUP_FIELD, t.as_bytes()),
            Needle::Str(t) => fingerprint(GROUP_STRING, t.as_bytes()),
            Needle::Int(v) => fingerprint(GROUP_INT, &v.to_le_bytes()),
            Needle::Type(t) => fingerprint(GROUP_TYPE, t.as_bytes()),
        }
    }
}

/// One cross reference site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Site {
    /// Class index in the root of the method containing the site.
    pub class: usize,
    /// Method index inside the class.
    pub method: usize,
    /// Offset of the instruction in code units.
    pub offset: u32,
    pub kind: SiteKind,
}

impl Site {
    pub fn method_id(&self) -> MethodId {
        (self.class, self.method)
    }
}

/// Which accesses of a field to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Any,
}

/// Limits and filters of one query.
#[derive(Debug, Clone)]
pub struct Query {
    /// Maximum number of sites returned.
    pub limit: usize,
    /// Only sites in methods of classes in this scope.
    pub scope: PackageScope,
    /// Decode the method of every hit and confirm the operand (on by default).
    pub verify: bool,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            limit: 1000,
            scope: PackageScope::all(),
            verify: true,
        }
    }
}

/// Result of a query: the sites and whether more existed than `limit` allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sites {
    pub sites: Vec<Site>,
    pub truncated: bool,
}

/// Target of a call: a method of the root or a method outside of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallTarget {
    Method(MethodId),
    External(MethodRefData),
}

/// One invoke instruction of a method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callee {
    pub offset: u32,
    pub kind: SiteKind,
    pub target: CallTarget,
}

/// Settings of [`UsageIndex::build`].
#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub store: StoreConfig,
    /// Worker threads (0: number of CPUs).
    pub threads: usize,
    /// Set to stop a running build; the build then fails with [`IndexError::Cancelled`].
    pub cancel: Option<Arc<AtomicBool>>,
    /// Only index methods of classes in this scope (callers outside the scope are not found).
    pub scope: PackageScope,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            store: StoreConfig::default(),
            threads: 0,
            cancel: None,
            scope: PackageScope::all(),
        }
    }
}

/// Counters of a finished build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildStats {
    pub methods_scanned: usize,
    /// Methods whose code could not be decoded (their sites are missing from the index).
    pub methods_undecodable: usize,
    pub sites: u64,
    pub on_disk: bool,
}

/// Cross reference index of a root.
pub struct UsageIndex<'a> {
    resolver: Resolver<'a>,
    source: &'a dyn CodeSource,
    store: PostingStore,
    stats: BuildStats,
}

fn needle_of(resolver: &Resolver<'_>, insn: &Insn, kind: SiteKind) -> Option<Needle> {
    Some(match (&insn.operand, kind) {
        (Operand::Method(m), k) if k.is_invoke() => {
            Needle::Method(resolver.canonical_method_text(m))
        }
        (Operand::Field(f), SiteKind::FieldRead | SiteKind::FieldWrite) => {
            Needle::Field(resolver.canonical_field_text(f))
        }
        (Operand::Str(s), SiteKind::StringLiteral) => Needle::Str(s.clone()),
        (Operand::None, SiteKind::IntLiteral) => Needle::Int(insn.literal),
        (Operand::Type(t), k) if k.is_type_use() => Needle::Type(t.clone()),
        _ => return None,
    })
}

/// Worker: scan the classes handed out by `next` and send their records.
struct Scanner<'s, 'r> {
    resolver: &'s Resolver<'r>,
    source: &'s dyn CodeSource,
    scope: &'s PackageScope,
    cancel: Option<&'s AtomicBool>,
    next: &'s AtomicUsize,
    scanned: &'s AtomicUsize,
    undecodable: &'s AtomicUsize,
}

impl Scanner<'_, '_> {
    fn run(&self, tx: &mpsc::SyncSender<Vec<Record>>) {
        let root = self.resolver.root();
        // reference text -> canonical key; sites of one reference are usually repeated
        let mut memo: HashMap<String, u64> = HashMap::new();
        loop {
            if self.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return;
            }
            let ci = self.next.fetch_add(1, Ordering::Relaxed);
            let Some(cls) = root.classes.get(ci) else {
                return;
            };
            if !self.scope.is_unrestricted() && !self.scope.matches(&cls.info.raw_name()) {
                continue;
            }
            let mut out: Vec<Record> = Vec::new();
            for (mi, m) in cls.methods.iter().enumerate() {
                let Some(code_offset) = m.code_offset else {
                    continue;
                };
                self.scanned.fetch_add(1, Ordering::Relaxed);
                let res = self.source.visit_method(ci, code_offset, &mut |insn| {
                    let Some(kind) = SiteKind::of(insn.opcode) else {
                        return;
                    };
                    let key = match (&insn.operand, kind) {
                        (Operand::Method(m), k) if k.is_invoke() => {
                            let raw = method_ref_text(m);
                            *memo.entry(raw).or_insert_with(|| {
                                Needle::Method(self.resolver.canonical_method_text(m)).key()
                            })
                        }
                        (Operand::Field(f), SiteKind::FieldRead | SiteKind::FieldWrite) => {
                            let raw = field_ref_text(f);
                            *memo.entry(raw).or_insert_with(|| {
                                Needle::Field(self.resolver.canonical_field_text(f)).key()
                            })
                        }
                        _ => match needle_of(self.resolver, insn, kind) {
                            Some(n) => n.key(),
                            None => return,
                        },
                    };
                    out.push(Record::new(
                        key,
                        ci as u32,
                        mi as u32,
                        kind as u8,
                        insn.offset,
                    ));
                });
                if res.is_err() {
                    self.undecodable.fetch_add(1, Ordering::Relaxed);
                }
            }
            if !out.is_empty() && tx.send(out).is_err() {
                return;
            }
        }
    }
}

impl<'a> UsageIndex<'a> {
    /// Scan all methods of `root` and build the index.
    pub fn build(
        root: &'a Root,
        source: &'a dyn CodeSource,
        opts: &BuildOptions,
    ) -> Result<UsageIndex<'a>> {
        let resolver = Resolver::new(root);
        let threads = if opts.threads == 0 {
            std::thread::available_parallelism().map_or(4, |n| n.get())
        } else {
            opts.threads
        };
        let next = AtomicUsize::new(0);
        let scanned = AtomicUsize::new(0);
        let undecodable = AtomicUsize::new(0);
        let mut builder = PostingStoreBuilder::new(opts.store.clone());
        let mut failure: Option<IndexError> = None;
        let (tx, rx) = mpsc::sync_channel::<Vec<Record>>(threads * 4);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let tx = tx.clone();
                let scanner = Scanner {
                    resolver: &resolver,
                    source,
                    scope: &opts.scope,
                    cancel: opts.cancel.as_deref(),
                    next: &next,
                    scanned: &scanned,
                    undecodable: &undecodable,
                };
                scope.spawn(move || scanner.run(&tx));
            }
            drop(tx);
            for batch in rx.iter() {
                if failure.is_some() {
                    continue; // drain so the workers can finish
                }
                for rec in batch {
                    if let Err(e) = builder.push(rec) {
                        failure = Some(e);
                        // stop the workers
                        next.store(usize::MAX / 2, Ordering::Relaxed);
                        break;
                    }
                }
            }
        });
        if let Some(e) = failure {
            return Err(e);
        }
        if opts
            .cancel
            .as_deref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
        {
            return Err(IndexError::Cancelled);
        }
        let sites = builder.len();
        let store = builder.finish()?;
        let stats = BuildStats {
            methods_scanned: scanned.load(Ordering::Relaxed),
            methods_undecodable: undecodable.load(Ordering::Relaxed),
            sites,
            on_disk: store.is_on_disk(),
        };
        Ok(UsageIndex {
            resolver,
            source,
            store,
            stats,
        })
    }

    pub fn stats(&self) -> &BuildStats {
        &self.stats
    }

    pub fn root(&self) -> &'a Root {
        self.resolver.root()
    }

    pub fn resolver(&self) -> &Resolver<'a> {
        &self.resolver
    }

    pub fn source(&self) -> &'a dyn CodeSource {
        self.source
    }

    /// Look the needle up, filter by access/scope, verify against the code.
    fn lookup(
        &self,
        needle: &Needle,
        q: &Query,
        accept: impl Fn(SiteKind) -> bool,
    ) -> Result<Sites> {
        let key = needle.key();
        let root = self.root();
        let mut out = Sites::default();
        // fetch more than needed: scope and verification drop hits
        let cap = q.limit.saturating_mul(16).max(4096);
        let mut raw = self.store.lookup(key, cap + 1)?;
        if raw.len() > cap {
            raw.truncate(cap);
            out.truncated = true;
        }
        let mut candidates: Vec<Site> = Vec::new();
        for r in raw {
            let Some(kind) = SiteKind::from_u8(r.kind()) else {
                continue;
            };
            if !accept(kind) {
                continue;
            }
            let (class, method) = (r.class as usize, r.method as usize);
            if !q.scope.is_unrestricted() && !q.scope.matches(&root.classes[class].info.raw_name())
            {
                continue;
            }
            candidates.push(Site {
                class,
                method,
                offset: r.offset(),
                kind,
            });
        }
        // verify method by method so every method is decoded once
        let mut i = 0;
        while i < candidates.len() {
            let mid = candidates[i].method_id();
            let mut j = i;
            while j < candidates.len() && candidates[j].method_id() == mid {
                j += 1;
            }
            let group = &candidates[i..j];
            i = j;
            let valid: Option<std::collections::HashSet<(u32, SiteKind)>> = if q.verify {
                Some(self.verified_offsets(mid, needle)?)
            } else {
                None
            };
            for s in group {
                if valid
                    .as_ref()
                    .is_some_and(|v| !v.contains(&(s.offset, s.kind)))
                {
                    continue;
                }
                if out.sites.len() >= q.limit {
                    out.truncated = true;
                    return Ok(out);
                }
                out.sites.push(*s);
            }
        }
        Ok(out)
    }

    /// Offsets (and kinds) of the instructions of a method whose operand equals `needle`.
    fn verified_offsets(
        &self,
        mid: MethodId,
        needle: &Needle,
    ) -> Result<std::collections::HashSet<(u32, SiteKind)>> {
        let mut set = std::collections::HashSet::new();
        let Some(code_offset) = self.root().classes[mid.0].methods[mid.1].code_offset else {
            return Ok(set);
        };
        self.source.visit_method(mid.0, code_offset, &mut |insn| {
            if let Some(kind) = SiteKind::of(insn.opcode) {
                if needle_of(&self.resolver, insn, kind).as_ref() == Some(needle) {
                    set.insert((insn.offset, kind));
                }
            }
        })?;
        Ok(set)
    }

    /// Invoke sites that call `target` (also through references naming a subclass).
    pub fn callers_of(&self, target: MethodId, q: &Query) -> Result<Sites> {
        let text = self.resolver.method_text(target);
        self.lookup(&Needle::Method(text), q, SiteKind::is_invoke)
    }

    /// Invoke sites of a method reference, for methods outside the root as well.
    pub fn callers_of_ref(&self, r: &MethodRefData, q: &Query) -> Result<Sites> {
        let text = self.resolver.canonical_method_text(r);
        self.lookup(&Needle::Method(text), q, SiteKind::is_invoke)
    }

    /// Accesses of a field of the root.
    pub fn field_sites(&self, field: FieldId, access: Access, q: &Query) -> Result<Sites> {
        let text = self.resolver.field_text(field);
        self.lookup(&Needle::Field(text), q, access_filter(access))
    }

    /// Accesses of a field reference, also for fields outside the root.
    pub fn field_sites_of_ref(&self, r: &FieldRefData, access: Access, q: &Query) -> Result<Sites> {
        let text = self.resolver.canonical_field_text(r);
        self.lookup(&Needle::Field(text), q, access_filter(access))
    }

    /// `const-string` sites with exactly this value.
    pub fn string_sites(&self, value: &str, q: &Query) -> Result<Sites> {
        self.lookup(&Needle::Str(value.to_owned()), q, |k| {
            k == SiteKind::StringLiteral
        })
    }

    /// Integer literal sites (`const`, `const-wide`, arithmetic with literal) with this value;
    /// 32-bit constants are compared sign-extended.
    pub fn int_sites(&self, value: i64, q: &Query) -> Result<Sites> {
        self.lookup(&Needle::Int(value), q, |k| k == SiteKind::IntLiteral)
    }

    /// Sites that use a class as operand (`new-instance`, `const-class`, `check-cast`,
    /// `instance-of`, `new-array`). `desc` is a type descriptor such as `Lpkg/Cls;`.
    pub fn type_sites(&self, desc: &str, q: &Query) -> Result<Sites> {
        self.lookup(&Needle::Type(desc.to_owned()), q, SiteKind::is_type_use)
    }

    /// Invoke instructions of one method with their resolved targets (decodes the method; does
    /// not use the posting store).
    pub fn callees_of(&self, caller: MethodId) -> Result<Vec<Callee>> {
        let mut out = Vec::new();
        let Some(code_offset) = self.root().classes[caller.0].methods[caller.1].code_offset else {
            return Ok(out);
        };
        self.source
            .visit_method(caller.0, code_offset, &mut |insn| {
                let Some(kind) = SiteKind::of(insn.opcode).filter(|k| k.is_invoke()) else {
                    return;
                };
                if let Operand::Method(m) = &insn.operand {
                    let target = match self.resolver.resolve_method(m) {
                        Some(id) => CallTarget::Method(id),
                        None => CallTarget::External(m.clone()),
                    };
                    out.push(Callee {
                        offset: insn.offset,
                        kind,
                        target,
                    });
                }
            })?;
        Ok(out)
    }

    /// Scan the code of all methods (without the posting store) and collect the instructions
    /// accepted by `pred`. Used for searches the exact-match postings cannot answer (substring
    /// of a string literal, literal ranges).
    pub fn scan(
        &self,
        scope: &PackageScope,
        limit: usize,
        cancel: Option<&AtomicBool>,
        pred: &dyn Fn(&Insn) -> bool,
    ) -> Result<ScanResult> {
        let root = self.root();
        let mut res = ScanResult::default();
        for (ci, cls) in root.classes.iter().enumerate() {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(IndexError::Cancelled);
            }
            if !scope.is_unrestricted() && !scope.matches(&cls.info.raw_name()) {
                continue;
            }
            for (mi, m) in cls.methods.iter().enumerate() {
                let Some(code_offset) = m.code_offset else {
                    continue;
                };
                let mut hits: Vec<(Site, Insn)> = Vec::new();
                // undecodable methods are skipped like in the build
                let _ = self.source.visit_method(ci, code_offset, &mut |insn| {
                    if let Some(kind) = SiteKind::of(insn.opcode) {
                        if pred(insn) {
                            hits.push((
                                Site {
                                    class: ci,
                                    method: mi,
                                    offset: insn.offset,
                                    kind,
                                },
                                insn.clone(),
                            ));
                        }
                    }
                });
                for h in hits {
                    if res.hits.len() >= limit {
                        res.truncated = true;
                        return Ok(res);
                    }
                    res.hits.push(h);
                }
            }
        }
        Ok(res)
    }

    /// String literals whose value contains `needle` (case insensitive for ASCII when
    /// `ignore_case`). Returns the sites with the literal value.
    pub fn search_strings(
        &self,
        needle: &str,
        ignore_case: bool,
        scope: &PackageScope,
        limit: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<(Site, String)>> {
        let lowered = needle.to_lowercase();
        let r = self.scan(scope, limit, cancel, &|insn| match &insn.operand {
            Operand::Str(s) => {
                if ignore_case {
                    s.to_lowercase().contains(&lowered)
                } else {
                    s.contains(needle)
                }
            }
            _ => false,
        })?;
        Ok(r.hits
            .into_iter()
            .filter_map(|(s, i)| match i.operand {
                Operand::Str(v) => Some((s, v)),
                _ => None,
            })
            .collect())
    }

    /// Integer literals in `lo..=hi`.
    pub fn search_int_range(
        &self,
        lo: i64,
        hi: i64,
        scope: &PackageScope,
        limit: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<(Site, i64)>> {
        let r = self.scan(scope, limit, cancel, &|insn| {
            SiteKind::of(insn.opcode) == Some(SiteKind::IntLiteral)
                && (lo..=hi).contains(&insn.literal)
        })?;
        Ok(r.hits.into_iter().map(|(s, i)| (s, i.literal)).collect())
    }
}

fn access_filter(access: Access) -> impl Fn(SiteKind) -> bool {
    move |k| match access {
        Access::Read => k == SiteKind::FieldRead,
        Access::Write => k == SiteKind::FieldWrite,
        Access::Any => matches!(k, SiteKind::FieldRead | SiteKind::FieldWrite),
    }
}

/// Result of [`UsageIndex::scan`].
#[derive(Debug, Default)]
pub struct ScanResult {
    pub hits: Vec<(Site, Insn)>,
    pub truncated: bool,
}
