//! Class level pass driver: runs the method and class passes of the default pipeline over a class
//! tree the way the reference does, one pass after the other over all methods of the top level
//! class and its inner classes, with the dependencies of a class processed before the class.
//!
//! The driver owns the [`Root`] because class passes modify classes, fields and methods. The
//! method code is provided by a [`CodeSource`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use jadx_input_api::TryData;

/// Diagnostic timing (`JADX_RS_TIMING`): a coarse, 3-way split of [`ClassDriver::run_passes`]'s
/// time, to find which group of passes dominates before trying to optimize or parallelize any
/// of them. "early" = debug info through `REPLACE_NEW_ARRAY`; "regions" = region building
/// through `CLEAN_REGIONS`; "late" = `METHOD_THROWS` through `PREPARE_FOR_CODEGEN`.
static EARLY_PASS_NANOS: AtomicU64 = AtomicU64::new(0);
static REGION_PASS_NANOS: AtomicU64 = AtomicU64::new(0);
static LATE_PASS_NANOS: AtomicU64 = AtomicU64::new(0);
static LOAD_METHODS_NANOS: AtomicU64 = AtomicU64::new(0);

/// Prints the accumulated split from the pass-timing statics above if `JADX_RS_TIMING` is set.
pub fn print_pass_timing_report() {
    if std::env::var("JADX_RS_TIMING").is_ok() {
        eprintln!(
            "TIMING load_methods: {:?}, passes early: {:?}, regions: {:?}, late: {:?}",
            std::time::Duration::from_nanos(LOAD_METHODS_NANOS.load(Ordering::Relaxed)),
            std::time::Duration::from_nanos(EARLY_PASS_NANOS.load(Ordering::Relaxed)),
            std::time::Duration::from_nanos(REGION_PASS_NANOS.load(Ordering::Relaxed)),
            std::time::Duration::from_nanos(LATE_PASS_NANOS.load(Ordering::Relaxed)),
        );
    }
}

use crate::decode::DecodedMethod;
use crate::model::cfg::MethodCfg;
use crate::model::node::{MethodId, Root};
use crate::model::types::ArgType;
use crate::passes::constructor::MethodEnv;
use crate::passes::pipeline::{build_ssa, FrameInfo};

/// Instructions, exception table and register count of a method with code.
pub struct LoadedCode {
    pub decoded: DecodedMethod,
    pub tries: Vec<TryData>,
    pub regs_count: usize,
    /// Register of the first argument if the input defines it (class files).
    pub args_start_reg: Option<usize>,
}

/// Provider of method code.
pub trait CodeSource {
    fn load_code(&self, root: &Root, cls: usize, mth: usize) -> Result<LoadedCode, String>;

    /// Debug info of the method's code (`IDebugInfo`): source lines and local variables, `None`
    /// when the input has none (stripped code, fallback mode sources).
    fn debug_info(
        &self,
        _root: &Root,
        _cls: usize,
        _mth: usize,
    ) -> Option<jadx_input_api::DebugInfo> {
        None
    }
}

impl<T: CodeSource + ?Sized> CodeSource for &T {
    fn load_code(&self, root: &Root, cls: usize, mth: usize) -> Result<LoadedCode, String> {
        (**self).load_code(root, cls, mth)
    }

    fn debug_info(&self, root: &Root, cls: usize, mth: usize) -> Option<jadx_input_api::DebugInfo> {
        (**self).debug_info(root, cls, mth)
    }
}

/// Pass pipeline selection. Every pass has a stage number (see [`pass_stage`]); a pass runs when its
/// stage is not above `stage`. Passes keep the order of the reference pass list.
#[derive(Debug, Clone)]
pub struct DriverOptions {
    pub stage: u32,
    pub extract_finally: bool,
    /// `JadxArgs.isInlineMethods`
    pub inline_methods: bool,
    /// `JadxArgs.isRespectBytecodeAccModifiers`
    pub respect_bytecode_acc_modifiers: bool,
}

impl Default for DriverOptions {
    fn default() -> Self {
        DriverOptions {
            stage: u32::MAX,
            extract_finally: true,
            inline_methods: true,
            respect_bytecode_acc_modifiers: false,
        }
    }
}

/// A method pass: the method code, its environment and the state of all methods.
type MethodPassFn<'a> =
    dyn Fn(&mut MethodCfg, &MethodEnv, &HashMap<MethodId, MethodState>) -> Result<(), String> + 'a;

/// A class pass: the root, the state of all methods and the class index.
type ClassPassFn<'a> =
    dyn Fn(&mut Root, &mut HashMap<MethodId, MethodState>, usize) -> Result<(), String> + 'a;

/// State of a method during processing.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct MethodState {
    pub cfg: Option<MethodCfg>,
    /// First error of a pass; the method is skipped by all later passes.
    pub failed: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassPhase {
    NotLoaded,
    Started,
    Complete,
}

/// One element of the traversal of a class tree.
#[derive(Clone, Copy)]
enum Visit {
    Class(usize),
    Method(usize, usize),
}

pub struct ClassDriver<'s> {
    pub root: Root,
    src: Box<dyn CodeSource + Send + Sync + 's>,
    opts: DriverOptions,
    pub methods: HashMap<MethodId, MethodState>,
    /// Results of [`ClassDriver::preload_all_methods`], drained into `methods` one class's
    /// worth at a time by `load_methods` as each class is reached. Kept separate from `methods`
    /// so the per-class cleanup in `core_gen::generate_with_env` (`methods.retain(...)`, run
    /// once per top-level class) keeps scanning only the bounded, current working set instead
    /// of the whole, O(method count) preloaded set every time.
    preloaded: HashMap<MethodId, MethodState>,
    phase: Vec<ClassPhase>,
    children: Vec<Vec<usize>>,
    /// Running total, in estimated bytes, of erroring method states kept past their own
    /// class's cleanup by `DriverLoader::load` and `generate_with_env`'s `methods.retain` (see
    /// `ClassDriver::method_has_error`'s doc comment) so a later class's `throws_visit` can
    /// still reach them - see those call sites' doc comments. Checked by `try_keep_for_throws`
    /// against a fraction of available memory, so a pathological input cannot grow this without
    /// bound; beyond that, `spill_to_disk` takes over. Never decremented - see
    /// `try_keep_for_throws`'s doc comment.
    kept_for_throws_units: AtomicU64,
    /// Overflow for a method state that `try_keep_for_throws` would otherwise have to drop once
    /// its in-memory budget is spent - see `method_disk_cache`'s module doc comment. Lazy: an
    /// app whose cross-class carryover never exceeds the in-memory budget never touches it.
    /// `RefCell`, not a plain field, so `DriverLoader::load` (`core_gen.rs`), which only ever
    /// holds `&ClassDriver`, can still spill/read through it.
    disk_cache: std::cell::RefCell<super::method_disk_cache::MethodDiskCache>,
}

impl<'s> ClassDriver<'s> {
    pub fn new<S: CodeSource + Send + Sync + 's>(
        root: Root,
        src: S,
        opts: DriverOptions,
    ) -> ClassDriver<'s> {
        let n = root.classes.len();
        let mut sorted: Vec<usize> = (0..n).collect();
        sorted.sort_by_cached_key(|i| root.classes[*i].info.raw_name());
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &i in &sorted {
            if let Some(p) = root.parent_index_of(&root.classes[i]) {
                if p != i {
                    children[p].push(i);
                }
            }
        }
        ClassDriver {
            root,
            src: Box::new(src),
            opts,
            methods: HashMap::new(),
            preloaded: HashMap::new(),
            phase: vec![ClassPhase::NotLoaded; n],
            children,
            kept_for_throws_units: AtomicU64::new(0),
            disk_cache: std::cell::RefCell::new(
                super::method_disk_cache::MethodDiskCache::default(),
            ),
        }
    }

    /// `ProcessState.isProcessComplete` of a top level class.
    pub fn is_process_complete(&self, top: usize) -> bool {
        self.phase[top] == ClassPhase::Complete
    }

    /// Whether an erroring method's state (worth `units`, in `MethodCfg::units_count`) may be
    /// kept in memory past its own class's cleanup, for a later class's recursive `throws_visit`
    /// lookup to still reach it (see `ClassDriver::method_has_error`'s doc comment). Checked
    /// (and, if allowed, accounted for) by every call site that would otherwise keep such a
    /// state around indefinitely; once the budget is spent, the caller is expected to spill the
    /// state to disk instead (see `ClassDriver::spill_to_disk`) rather than drop it outright, so
    /// bounding this does not cost correctness, only some I/O under heavy load.
    ///
    /// The budget is a fraction of currently available system memory (converted from `units` the
    /// same way `preload_all_methods` converts its own instruction counts, since both estimate
    /// the same thing: the size of the `MethodCfg`/SSA state built from that much bytecode),
    /// re-read on every call (not cached) so it adapts as the run's actual memory pressure
    /// changes. Never decremented when spent (see `kept_for_throws_units`'s doc comment), so
    /// this is intentionally conservative later in a long run rather than track exactly how much
    /// of the earlier "kept" total has since become unreachable garbage.
    pub fn try_keep_for_throws(&self, units: usize) -> bool {
        const BYTES_PER_UNIT_ESTIMATE: u64 = 3_000;
        const BUDGET_NUMERATOR: u64 = 1;
        const BUDGET_DENOMINATOR: u64 = 4;
        let budget_bytes = available_memory_bytes() * BUDGET_NUMERATOR / BUDGET_DENOMINATOR;
        let bytes = (units as u64).saturating_mul(BYTES_PER_UNIT_ESTIMATE);
        let mut spent = self.kept_for_throws_units.load(Ordering::Relaxed);
        loop {
            if spent.saturating_add(bytes) > budget_bytes {
                return false;
            }
            match self.kept_for_throws_units.compare_exchange_weak(
                spent,
                spent + bytes,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => spent = actual,
            }
        }
    }

    /// Spills a method's state to disk for a later class's `throws_visit` to read back via
    /// [`ClassDriver::load_from_disk`] - the fallback once [`ClassDriver::try_keep_for_throws`]'s
    /// in-memory budget is spent, instead of dropping the state (and the cross-class `throws`
    /// information it carries) outright. Returns whether it succeeded; the caller falls back to
    /// dropping the state on failure (a full disk, for instance), same as before this existed.
    pub fn spill_to_disk(&self, id: MethodId, state: &MethodState) -> bool {
        self.disk_cache.borrow_mut().store(id, state)
    }

    /// Whether `id`'s state is currently spilled to disk (see [`ClassDriver::spill_to_disk`]).
    pub fn is_on_disk(&self, id: MethodId) -> bool {
        self.disk_cache.borrow().contains(id)
    }

    /// Reads back and removes a method's state spilled by [`ClassDriver::spill_to_disk`].
    pub fn load_from_disk(&self, id: MethodId) -> Option<MethodState> {
        self.disk_cache.borrow_mut().take(id)
    }

    /// Whether a still-needed cross-class dependency method - one `core_gen`'s `methods.retain`
    /// would otherwise keep resident unconditionally (`top_parent_index(c) != cls`) - should
    /// instead be spilled to disk now to relieve memory pressure.
    ///
    /// Correctness does not depend on the answer: [`ClassDriver::process_with_deps`] and
    /// [`ClassDriver::process_class`] materialize a class's spilled dependencies back into
    /// `methods` (via [`ClassDriver::materialize_subtree_from_disk`]) before its passes and code
    /// generation resolve and read them, so spilling only ever trades RAM for disk I/O, never
    /// output. It is therefore safe to spill as aggressively as memory pressure calls for.
    ///
    /// Returns true only under real memory pressure (available system memory below a floor) or
    /// when `JADX_RS_FORCE_SPILL` forces it (the spill-path stress test). An app whose working
    /// set fits comfortably in memory - the common case, e.g. the oracle input - never crosses
    /// the floor, so nothing is ever spilled and the behavior is byte-for-byte the previous one.
    pub fn should_spill_dependency(&self) -> bool {
        if std::env::var("JADX_RS_FORCE_SPILL").is_ok() {
            return true;
        }
        // 23 GB RAM machines collapse into swap thrashing on the largest apps once essentially
        // the whole decoded program is held resident at once; start spilling well before that,
        // while there is still headroom, rather than when the system is already thrashing.
        const SPILL_WHEN_AVAILABLE_BELOW: u64 = 6_000_000_000;
        available_memory_bytes() < SPILL_WHEN_AVAILABLE_BELOW
    }

    /// Brings every method of `top`'s class subtree (the top-level class and its inner classes)
    /// that is currently spilled to disk back into `self.methods`, so a later read of it - a
    /// dependency's constructor in `enum_visitor`, an inlined callee in `process_variables`, a
    /// method's blocks during code generation - finds it present again, exactly as if it had
    /// never left memory. The counterpart to `core_gen`'s `methods.retain` spilling a
    /// still-needed dependency once memory is tight (see [`ClassDriver::should_spill_dependency`]):
    /// that frees the RAM between uses, this restores it just before the next use. A no-op when
    /// nothing of the subtree is on disk (the common, no-pressure case).
    fn materialize_subtree_from_disk(&mut self, top: usize) {
        if !self.disk_cache.borrow().in_use() {
            return;
        }
        let mut subtree = Vec::new();
        self.collect_subtree(top, &mut subtree);
        for c in subtree {
            for m in 0..self.root.classes[c].methods.len() {
                if !self.methods.contains_key(&(c, m)) && self.is_on_disk((c, m)) {
                    if let Some(state) = self.load_from_disk((c, m)) {
                        self.methods.insert((c, m), state);
                    }
                }
            }
        }
    }

    fn collect_subtree(&self, c: usize, out: &mut Vec<usize>) {
        out.push(c);
        for &inner in &self.children[c] {
            self.collect_subtree(inner, out);
        }
    }

    pub fn into_root(self) -> Root {
        self.root
    }

    /// Process every top level class like the code generation entry point does (without
    /// generating code): dependencies first, then the class.
    pub fn process_all(&mut self) {
        for ci in self.root.sorted_class_order() {
            if !self.root.classes[ci].is_inner() {
                self.process_with_deps(ci);
            }
        }
    }

    /// `ProcessClass.generateCode` without the code generation.
    pub fn process_with_deps(&mut self, top: usize) {
        if self.root.classes[top].dont_generate {
            self.process_class(top);
            return;
        }
        // Restore anything of this class or its dependencies that was spilled to disk under
        // earlier memory pressure before it is read again. `top` itself (its own methods are
        // read by the code generation that follows this call) and every dependency may be
        // `ClassPhase::Complete` already, in which case the `process_class` calls below skip it
        // entirely and would not bring it back; do it here for the whole set up front. Skipped
        // wholesale (no list clones, no walks) when nothing has ever been spilled.
        if self.disk_cache.borrow().in_use() {
            self.materialize_subtree_from_disk(top);
            for dep in self.root.classes[top].dependencies.clone() {
                self.materialize_subtree_from_disk(dep);
            }
            for dep in self.root.classes[top].codegen_deps.clone() {
                self.materialize_subtree_from_disk(dep);
            }
        }
        for dep in self.root.classes[top].dependencies.clone() {
            self.process_class(dep);
        }
        let codegen_deps = self.root.classes[top].codegen_deps.clone();
        if !codegen_deps.is_empty() {
            self.process_class(top);
            for dep in codegen_deps {
                self.process_class(dep);
            }
        }
        self.process_class(top);
    }

    /// `ProcessClass.process(cls, false)` of a top level class.
    pub fn process_class(&mut self, top: usize) {
        let top = self.root.top_parent_index(top);
        if self.phase[top] != ClassPhase::NotLoaded {
            return;
        }
        self.phase[top] = ClassPhase::Started;
        if std::env::var("LOG_ORDER").is_ok() {
            eprintln!("PROC {}", self.root.classes[top].info.raw_name());
        }
        // This class's passes resolve and read the state of the methods it calls, all of which
        // live in its dependencies; restore any that was spilled to disk first (see
        // `materialize_subtree_from_disk`). `process_with_deps` does this too for the top-level
        // generation target, but a dependency reached only here (processed fresh as another
        // class's dependency) needs its own dependencies brought back as well.
        if self.disk_cache.borrow().in_use() {
            for dep in self.root.classes[top].dependencies.clone() {
                self.materialize_subtree_from_disk(dep);
            }
        }
        let mut visits = Vec::new();
        self.collect_visits(top, &mut visits);
        let timing = std::env::var("JADX_RS_TIMING").is_ok();
        let t = std::time::Instant::now();
        self.load_methods(&visits);
        if timing {
            LOAD_METHODS_NANOS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        self.run_passes(&visits);
        self.phase[top] = ClassPhase::Complete;
    }

    fn collect_visits(&self, cls: usize, out: &mut Vec<Visit>) {
        out.push(Visit::Class(cls));
        for &inner in &self.children[cls] {
            self.collect_visits(inner, out);
        }
        for m in 0..self.root.classes[cls].methods.len() {
            out.push(Visit::Method(cls, m));
        }
    }

    /// Method loading and the passes up to the type inference. Drains a method already decoded
    /// by [`ClassDriver::preload_all_methods`] out of `preloaded` into `methods` instead of
    /// redoing the work (see `preloaded`'s doc comment for why it is a separate map).
    fn load_methods(&mut self, visits: &[Visit]) {
        for v in visits {
            let Visit::Method(c, m) = *v else {
                continue;
            };
            if self.root.classes[c].methods[m].code_offset.is_none() {
                continue;
            }
            if self.methods.contains_key(&(c, m)) {
                continue;
            }
            let state = match self.preloaded.remove(&(c, m)) {
                Some(state) => state,
                // a cross-class dependency another class's processing spilled to disk (see
                // `ClassDriver::spill_to_disk`'s doc comment) instead of decoding it again.
                None => match self.load_from_disk((c, m)) {
                    Some(state) => state,
                    None => self.load_method(c, m),
                },
            };
            self.methods.insert((c, m), state);
        }
    }

    fn load_method(&mut self, c: usize, m: usize) -> MethodState {
        decode_method(
            &self.root,
            self.src.as_ref(),
            self.opts.extract_finally,
            c,
            m,
        )
    }

    /// Decodes and builds the SSA state of every method of the whole root up front, across
    /// `threads` OS threads (clamped to the method count). Bytecode decoding and per-method SSA
    /// construction (`decode_method`) only reads the shared `Root`/`CodeSource` and only writes
    /// its own method's result, so unlike the later class passes (which accumulate state across
    /// classes in a way that is NOT safe to split across independent threads - see TODO.md's
    /// "Performance/Parallelitaet" investigation, e.g. override/throws information) this part
    /// has no cross-method dependency and is safe to parallelize. Call once, before any
    /// `process_class`/`process_with_deps`/`process_all` call, so `load_methods` finds every
    /// entry already present and skips redoing the work.
    ///
    /// Holding every method's decoded state at once (instead of only the current class's, like
    /// the non-preloaded path) costs extra peak memory proportional to the method count -
    /// measured at about 65 KB/method on a real app. If that would not comfortably fit the
    /// system's available memory, this silently does nothing (`load_methods` already falls back
    /// to loading on demand when a method is not in `preloaded`), trading the speedup for
    /// safety on very large inputs instead of risking a hang or an OOM kill.
    ///
    /// `done_top_classes` are top-level class indices whose code was already generated (and
    /// whose methods were already retained out of `self.methods` by the per-class cleanup in
    /// `core_gen::generate_with_env`) before this call - the caller defers the preload past the
    /// first class it processes to avoid paying for it on a single-class query. Their methods
    /// are skipped here too, and never reach `self.preloaded` (added to, not replacing, whatever
    /// is already there): they will never be visited again, so nothing would ever drain them,
    /// which would otherwise leak one class's worth of decoded methods for the rest of the run.
    pub fn preload_all_methods(&mut self, threads: usize, done_top_classes: &HashSet<usize>) {
        let mut targets = Vec::new();
        for (c, cls) in self.root.classes.iter().enumerate() {
            if done_top_classes.contains(&self.root.top_parent_index(c)) {
                continue;
            }
            for m in 0..cls.methods.len() {
                if cls.methods[m].code_offset.is_some()
                    && !self.methods.contains_key(&(c, m))
                    && !self.is_on_disk((c, m))
                {
                    targets.push((c, m));
                }
            }
        }
        if targets.len() < 2 {
            // Not worth it for a lone method (e.g. a `--single-class`/single-method query):
            // the preload's own bookkeeping and thread setup would outweigh the saving.
            return;
        }
        // Budget on the methods' real, decoded size (instruction count and register count -
        // both drive the size of the SSA state `build_method_state` constructs), not a flat
        // per-method guess: a method that is unusually large in either dimension - plain code,
        // or an adversarial input trying to defeat a method-count-based cap - costs
        // proportionally more, and this tracks that as it goes rather than assuming an average
        // up front.
        const BYTES_PER_INSN_ESTIMATE: u64 = 3_000;
        const BYTES_PER_INSN_PER_REG_ESTIMATE: u64 = 24;
        const BUDGET_NUMERATOR: u64 = 3;
        const BUDGET_DENOMINATOR: u64 = 5;
        let budget_bytes = available_memory_bytes() * BUDGET_NUMERATOR / BUDGET_DENOMINATOR;
        let threads = threads.clamp(1, targets.len());
        let root = &self.root;
        let src = self.src.as_ref();
        let extract_finally = self.opts.extract_finally;
        let next = AtomicUsize::new(0);
        let spent_bytes = AtomicU64::new(0);
        let results: Mutex<Vec<(MethodId, MethodState)>> =
            Mutex::new(Vec::with_capacity(targets.len()));
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        // Checked before doing any work this iteration (not just via the
                        // per-method budget check below) so that once the budget is blown,
                        // every thread stops picking up new targets instead of continuing to
                        // decode (and immediately discard) the rest of them.
                        if spent_bytes.load(Ordering::Relaxed) > budget_bytes {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= targets.len() {
                            break;
                        }
                        let (c, m) = targets[i];
                        let Ok(loaded) = load_method_code(root, src, c, m) else {
                            continue; // load failure: let `load_methods` retry and report it
                        };
                        let method_bytes = (loaded.decoded.insns.len() as u64)
                            .saturating_mul(BYTES_PER_INSN_ESTIMATE)
                            .saturating_add(
                                (loaded.decoded.insns.len() as u64)
                                    .saturating_mul(loaded.regs_count as u64)
                                    .saturating_mul(BYTES_PER_INSN_PER_REG_ESTIMATE),
                            );
                        let spent =
                            spent_bytes.fetch_add(method_bytes, Ordering::Relaxed) + method_bytes;
                        if spent > budget_bytes {
                            // Over budget: leave this one (and, from the next loop iteration,
                            // every other thread's remaining targets) for the on-demand path
                            // instead of building its (comparatively expensive) SSA state too.
                            break;
                        }
                        let state = build_method_state(root, extract_finally, c, m, loaded);
                        local.push(((c, m), state));
                    }
                    results
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .extend(local);
                });
            }
        });
        self.preloaded
            .extend(results.into_inner().unwrap_or_else(|e| e.into_inner()));
    }

    /// `DebugInfoAttachVisitor` + `DebugInfoApplyVisitor` (local variable names and source
    /// lines), applied directly to the already built SSA state (see module docs of
    /// `passes::debug_info`). Run before [`CodeSource::debug_info`]'s caller mutably borrows
    /// `self.methods`, so this is a dedicated loop rather than [`ClassDriver::method_pass`].
    fn apply_debug_info_pass(&mut self, visits: &[Visit]) {
        for v in visits {
            let Visit::Method(c, m) = *v else {
                continue;
            };
            let Some(debug) = self.src.debug_info(&self.root, c, m) else {
                continue;
            };
            if let Some(st) = self.methods.get_mut(&(c, m)) {
                if st.failed.is_none() {
                    if let Some(cfg) = st.cfg.as_mut() {
                        crate::passes::debug_info::apply_debug_info(cfg, &debug);
                    }
                }
            }
        }
    }

    /// Run `f` for every loaded method of the traversal.
    fn method_pass(&mut self, visits: &[Visit], f: &MethodPassFn) {
        for v in visits {
            if let Visit::Method(c, m) = *v {
                self.method_pass_one(c, m, f);
            }
        }
    }

    /// `DepthTraversal.visit(IDexTreeVisitor, MethodNode)`'s `mth.contains(AType.JADX_ERROR)`
    /// check: true once a method carries a `JadxError` (here, a hard pass failure or a
    /// non-empty `cfg.errors` - a pass recorded a failure but still returned `Ok`, e.g. the
    /// type inference update-count overflow). From that point every later per-method pass is
    /// skipped for the method, not just the one that failed, leaving its state exactly as it
    /// was at the point of failure (no `throws` clause, no later warning comments) before it
    /// falls back to a bytecode dump at codegen time. Used at every per-method pass site that
    /// does not already go through [`ClassDriver::method_pass_one`].
    fn method_has_error(&self, id: MethodId) -> bool {
        let Some(st) = self.methods.get(&id) else {
            return false;
        };
        st.failed.is_some() || st.cfg.as_ref().is_some_and(|c| !c.errors.is_empty())
    }

    /// Run `f` for one method if it is loaded and did not fail.
    fn method_pass_one(&mut self, c: usize, m: usize, f: &MethodPassFn) {
        let Some(st) = self.methods.get_mut(&(c, m)) else {
            return;
        };
        if st.failed.is_some() {
            return;
        }
        let Some(mut cfg) = st.cfg.take() else {
            return;
        };
        if !cfg.errors.is_empty() {
            self.methods.get_mut(&(c, m)).expect("method state").cfg = Some(cfg);
            return;
        }
        let env = MethodEnv {
            root: &self.root,
            cls: c,
            mth: m,
        };
        let methods = &self.methods;
        let r =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut cfg, &env, methods)));
        let st = self.methods.get_mut(&(c, m)).expect("method state");
        st.cfg = Some(cfg);
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => st.failed = Some(e),
            Err(_) => st.failed = Some("panic in pass".to_string()),
        }
    }

    /// Run `f` for every class of the traversal.
    fn class_pass(&mut self, visits: &[Visit], f: &ClassPassFn) {
        for v in visits {
            if let Visit::Class(c) = *v {
                // errors of class passes are reported as class warnings in the reference
                let _ = f(&mut self.root, &mut self.methods, c);
            }
        }
    }

    /// `MethodThrowsVisitor.visit`
    fn throws_visit(&mut self, id: MethodId) {
        use crate::passes::method_throws as mt;
        let (c, m) = id;
        if self.root.classes[c].methods[m].throws_attr.is_none() {
            self.root.classes[c].methods[m].throws_attr = Some(Default::default());
        }
        let visited = self.root.classes[c].methods[m]
            .throws_attr
            .as_ref()
            .is_some_and(|a| a.visited);
        let no_code = self.root.classes[c].methods[m].code_offset.is_none();
        if !self.methods.contains_key(&id) {
            // a cross-class dependency another class's processing spilled to disk instead of
            // dropping (see `ClassDriver::spill_to_disk`'s doc comment) - bring it back so
            // `throws_process` below can read its blocks, same as if it had never left memory.
            if let Some(state) = self.load_from_disk(id) {
                self.methods.insert(id, state);
            }
        }
        let has_blocks = self.methods.get(&id).is_some_and(|s| s.cfg.is_some());
        if !visited && (no_code || has_blocks) {
            if let Some(a) = self.root.classes[c].methods[m].throws_attr.as_mut() {
                a.visited = true;
            }
            self.throws_process(id);
        }
        let declared = self.root.classes[c].methods[m].attrs.throws.clone();
        let mut add = Vec::new();
        for d in &declared {
            let ty = ArgType::object(d);
            if mt::validate_exception(&self.root, &ty) != mt::ExceptionType::NoException {
                if let Some(n) = ty.object_name() {
                    add.push(n);
                }
            }
        }
        let mut attr = self.root.classes[c].methods[m]
            .throws_attr
            .take()
            .unwrap_or_default();
        for n in &add {
            attr.add(n);
        }
        mt::merge_exceptions(&self.root, &mut attr);
        self.root.classes[c].methods[m].throws_attr = Some(attr);
    }

    fn throws_process(&mut self, id: MethodId) {
        use crate::passes::method_throws as mt;
        let plan = match self.methods.get(&id).and_then(|s| s.cfg.as_ref()) {
            Some(cfg) => mt::plan(&self.root, cfg),
            None => return,
        };
        for p in plan {
            match p.step {
                mt::Step::Throw(ty) => self.visit_throws(id, &ty, &p.excluded),
                mt::Step::CallApp(callee) => {
                    self.throws_visit(callee);
                    let list = self.root.classes[callee.0].methods[callee.1]
                        .throws_attr
                        .as_ref()
                        .filter(|a| !a.is_empty())
                        .map(|a| a.java_order())
                        .unwrap_or_default();
                    for s in list {
                        self.visit_throws(id, &ArgType::object(&s), &p.excluded);
                    }
                }
                mt::Step::CallClsp(types) => {
                    for t in types {
                        self.visit_throws(id, &t, &p.excluded);
                    }
                }
            }
        }
    }

    /// `visitThrows`
    fn visit_throws(&mut self, id: MethodId, ty: &ArgType, excluded: &[String]) {
        use crate::passes::method_throws as mt;
        if !ty.is_type_known() {
            return;
        }
        // an unknown type hierarchy is assumed to be an exception
        let required = match mt::validate_exception(&self.root, ty) {
            mt::ExceptionType::UnknownType | mt::ExceptionType::ThrowsRequired => true,
            mt::ExceptionType::Runtime | mt::ExceptionType::NoException => false,
        };
        if !required {
            return;
        }
        let Some(name) = ty.object_name() else {
            return;
        };
        if excluded
            .iter()
            .any(|e| mt::is_base_exception(&self.root, &name, e))
        {
            return;
        }
        if let Some(a) = self.root.classes[id.0].methods[id.1].throws_attr.as_mut() {
            a.add(&name);
        }
    }

    /// `MarkMethodsForInline.process`: set the inline attribute of a method if it has none.
    /// `own` is the code of the method if it is currently taken out of the state. Returns false
    /// if the method can't be analyzed because it is not loaded.
    fn mark_process(&mut self, id: MethodId, own: Option<&MethodCfg>) -> bool {
        if self.root.classes[id.0].methods[id.1].inline.is_some() {
            return true;
        }
        if own.is_none() && !self.methods.contains_key(&id) {
            // `id` may be a method of another, already `ClassPhase::Complete` class that spilled
            // it to disk (see `ClassDriver::spill_to_disk`'s doc comment): that class will not
            // be run through `load_methods` again (`process_class` is a no-op once complete), so
            // without this it would look permanently "not loaded" to every caller below.
            if let Some(state) = self.load_from_disk(id) {
                self.methods.insert(id, state);
            }
        }
        let cfg = own.or_else(|| self.methods.get(&id).and_then(|s| s.cfg.as_ref()));
        let respect = self.opts.respect_bytecode_acc_modifiers;
        match crate::passes::mark_methods_for_inline::process(&self.root, id, cfg, respect) {
            Some(marked) => {
                crate::passes::mark_methods_for_inline::apply_visibility(
                    &mut self.root,
                    &marked.make_public,
                );
                self.root.classes[id.0].methods[id.1].inline = Some(marked.attr);
                true
            }
            None => false,
        }
    }

    /// `InlineMethods`
    fn inline_methods(&mut self, visits: &[Visit]) {
        for v in visits {
            let Visit::Method(c, m) = *v else {
                continue;
            };
            let Some(st) = self.methods.get_mut(&(c, m)) else {
                continue;
            };
            if st.failed.is_some() {
                continue;
            }
            let Some(mut cfg) = st.cfg.take() else {
                continue;
            };
            if !cfg.errors.is_empty() {
                self.methods.get_mut(&(c, m)).expect("method state").cfg = Some(cfg);
                continue;
            }
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.inline_in_method((c, m), &mut cfg)
            }));
            let st = self.methods.get_mut(&(c, m)).expect("method state");
            st.cfg = Some(cfg);
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => st.failed = Some(e),
                Err(_) => st.failed = Some("panic in pass".to_string()),
            }
        }
    }

    fn inline_in_method(&mut self, mth: MethodId, cfg: &mut MethodCfg) -> Result<(), String> {
        use crate::model::insn::InsnType;
        for block in cfg.blocks.clone() {
            for insn in cfg.block(block).insns.clone() {
                if cfg.insn(insn).ty != InsnType::Invoke {
                    continue;
                }
                let Some(details) = cfg.insn(insn).details.clone() else {
                    continue;
                };
                let Some(callee) = self.root.resolve_method(&details) else {
                    continue;
                };
                self.process_invoke_for_inline(mth, cfg, block, insn, callee)?;
            }
        }
        Ok(())
    }

    fn process_invoke_for_inline(
        &mut self,
        mth: MethodId,
        cfg: &mut MethodCfg,
        block: crate::model::insn::BlockId,
        insn: crate::model::insn::InsnId,
        callee: MethodId,
    ) -> Result<(), String> {
        let own = if callee == mth { Some(&*cfg) } else { None };
        let mut loaded = self.mark_process(callee, own);
        if !loaded {
            // method is not yet loaded => force process
            let top = self.root.top_parent_index(callee.0);
            // the code of the method in progress is visible to the processing of other classes
            if let Some(st) = self.methods.get_mut(&mth) {
                st.cfg = Some(cfg.clone());
            }
            self.process_class(top);
            if let Some(st) = self.methods.get_mut(&mth) {
                st.cfg = None;
            }
            let own = if callee == mth { Some(&*cfg) } else { None };
            loaded = self.mark_process(callee, own);
            if !loaded {
                let name = format!(
                    "{}.{}",
                    self.root.classes[callee.0].info.full_name(),
                    self.root.classes[callee.0].methods[callee.1]
                        .info
                        .short_id()
                );
                cfg.warnings.push(format!(
                    "Failed to check method for inline after forced process{name}"
                ));
                return Ok(());
            }
        }
        let Some(attr) = self.root.classes[callee.0].methods[callee.1].inline.clone() else {
            return Ok(());
        };
        if attr.is_not_needed() {
            return Ok(());
        }
        let (done, updates) = crate::passes::inline_methods::inline_method(
            &self.root, cfg, callee, &attr, block, insn,
        );
        if let Some(stored) = self.root.classes[callee.0].methods[callee.1]
            .inline
            .as_mut()
        {
            for (i, k, arg) in updates {
                stored.insns[i].args[k] = arg;
            }
        }
        if done {
            crate::passes::inline_methods::update_usage_info(&mut self.root, mth, callee, &attr);
        }
        Ok(())
    }

    /// `FixSwitchOverEnum`: a pass with a class and a method part, in traversal order.
    fn fix_switch_over_enum(&mut self, visits: &[Visit]) {
        use crate::passes::fix_switch_over_enum as fse;
        for v in visits {
            match *v {
                Visit::Class(c) => fse::init_cls_enum_map(&mut self.root, &self.methods, c),
                Visit::Method(c, m) => {
                    let pending = std::cell::RefCell::new(Vec::new());
                    self.method_pass_one(c, m, &|cfg, env, _| {
                        fse::fix_switch_over_enum(cfg, env, &mut pending.borrow_mut())
                    });
                    for field in pending.into_inner() {
                        fse::hide_enum_map_field(&mut self.root, field);
                    }
                }
            }
        }
    }

    /// Like [`ClassDriver::class_pass`] for passes that process the inner classes of a class
    /// before the class itself.
    fn class_pass_post(&mut self, top: usize, f: &ClassPassFn) {
        fn post(children: &[Vec<usize>], c: usize, out: &mut Vec<usize>) {
            for &i in &children[c] {
                post(children, i, out);
            }
            out.push(c);
        }
        let mut order = Vec::new();
        post(&self.children, top, &mut order);
        for c in order {
            if let Err(e) = f(&mut self.root, &mut self.methods, c) {
                if std::env::var("JADX_RS_ERRORS").is_ok() {
                    eprintln!("CLASS ERROR {}: {e}", self.root.classes[c].info.raw_name());
                }
            }
        }
    }

    fn run_passes(&mut self, visits: &[Visit]) {
        use crate::passes::regions as rg;
        let stage = self.opts.stage;
        let on = |s: u32| stage >= s;
        let timing = std::env::var("JADX_RS_TIMING").is_ok();
        let t_group = std::time::Instant::now();

        if on(pass_stage::MOD_VISITOR) {
            self.apply_debug_info_pass(visits);
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::debug_info::apply_method_parameters(cfg, env.root, env.cls, env.mth);
                Ok(())
            });
        }
        if on(pass_stage::KOTLIN_INTERNALS) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::kotlin_internals::process_kotlin_internals(cfg, env.root)
            });
        }
        if on(pass_stage::INLINE_METHODS) && self.opts.inline_methods {
            self.inline_methods(visits);
        }
        if on(pass_stage::GENERIC_TYPES) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::generic_types::generic_types(cfg, env.root)
            });
        }
        if on(pass_stage::SHADOW_FIELD) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::shadow_field::shadow_field(cfg, env.root)
            });
        }
        if on(pass_stage::DEBOXING) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::deboxing::deboxing(cfg, env)
            });
        }
        if on(pass_stage::ANONYMOUS_CLASS) {
            self.class_pass(visits, &|root, methods, c| {
                crate::passes::anonymous_class::anonymous_class_visitor(root, methods, c)
            });
        }
        if on(pass_stage::MOD_VISITOR) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::mod_visitor::mod_visitor(cfg, env)
            });
        }
        if on(pass_stage::CODE_SHRINK) {
            self.method_pass(visits, &|cfg, _, _| {
                crate::passes::code_shrink::code_shrink(cfg)
            });
        }
        if on(pass_stage::REPLACE_NEW_ARRAY) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::replace_new_array::replace_new_array(cfg, env)
            });
        }
        if timing {
            EARLY_PASS_NANOS.fetch_add(t_group.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        let t_group = std::time::Instant::now();
        if on(pass_stage::REGION_MAKER) {
            self.method_pass(visits, &|cfg, env, _| rg::region_maker_visitor(cfg, env));
        }
        if on(pass_stage::IF_REGION) {
            self.method_pass(visits, &|cfg, env, _| {
                if cfg.blocks.is_empty() {
                    return Ok(());
                }
                rg::if_visitor::if_region_visitor(cfg, env)
            });
        }
        if on(pass_stage::SWITCH_OVER_STRING) {
            self.method_pass(visits, &|cfg, env, _| {
                if cfg.blocks.is_empty() {
                    return Ok(());
                }
                rg::switch_over_string::switch_over_string(cfg, env)
            });
        }
        if on(pass_stage::RETURN) {
            self.method_pass(visits, &|cfg, _, _| rg::if_visitor::return_visitor(cfg));
        }
        if on(pass_stage::CLEAN_REGIONS) {
            self.method_pass(visits, &|cfg, _, _| rg::clean::clean_regions(cfg));
        }
        if timing {
            REGION_PASS_NANOS.fetch_add(t_group.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        let t_group = std::time::Instant::now();
        if on(pass_stage::METHOD_THROWS) {
            for v in visits {
                if let Visit::Method(c, m) = *v {
                    if self.method_has_error((c, m)) {
                        continue;
                    }
                    self.throws_visit((c, m));
                }
            }
        }
        if on(pass_stage::CODE_SHRINK2) {
            self.method_pass(visits, &|cfg, _, _| rg::shrink_method(cfg));
        }
        if on(pass_stage::METHOD_INVOKE) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::method_invoke::method_invoke(cfg, env)
            });
        }
        if on(pass_stage::SIMPLIFY) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::simplify::simplify(cfg, env)
            });
        }
        if on(pass_stage::CHECK_REGIONS) {
            self.method_pass(visits, &|cfg, _, _| rg::check::check_regions(cfg));
        }
        if on(pass_stage::FIX_INSTRUCTIONS) {
            self.method_pass(visits, &|cfg, _, _| {
                crate::passes::fix_instructions::fix_instructions(cfg)
            });
        }
        if on(pass_stage::ENUM_VISITOR) {
            self.class_pass(visits, &|root, methods, c| {
                crate::passes::enum_visitor::enum_visitor(root, methods, c)
            });
        }
        if on(pass_stage::FIX_SWITCH_OVER_ENUM) {
            self.fix_switch_over_enum(visits);
        }
        if on(pass_stage::FIX_ACCESS_MODIFIERS) && !self.opts.respect_bytecode_acc_modifiers {
            for v in visits {
                match *v {
                    Visit::Class(c) => {
                        crate::passes::fix_access_modifiers::fix_class_visibility(&mut self.root, c)
                    }
                    Visit::Method(c, m) => {
                        if self.method_has_error((c, m)) {
                            continue;
                        }
                        crate::passes::fix_access_modifiers::fix_method_visibility(
                            &mut self.root,
                            (c, m),
                        )
                    }
                }
            }
        }
        if on(pass_stage::EXTRACT_FIELD_INIT) {
            if let Some(Visit::Class(top)) = visits.first().copied() {
                self.class_pass_post(top, &|root, methods, c| {
                    crate::passes::extract_field_init::extract_field_init(root, methods, c)
                });
            }
        }
        if on(pass_stage::CLASS_MODIFIER) {
            if let Some(Visit::Class(top)) = visits.first().copied() {
                self.class_pass_post(top, &|root, methods, c| {
                    crate::passes::class_modifier::class_modifier(root, methods, c)
                });
            }
        }
        if on(pass_stage::LOOP_REGION) {
            self.method_pass(visits, &|cfg, env, _| {
                rg::loop_visitor::loop_region_visitor(cfg, env)
            });
        }
        if on(pass_stage::SWITCH_BREAK) {
            self.method_pass(visits, &|cfg, env, _| {
                rg::switch_break::switch_break_visitor(cfg, env)
            });
        }
        if on(pass_stage::MARK_METHODS_FOR_INLINE) && self.opts.inline_methods {
            for v in visits {
                if let Visit::Method(c, m) = *v {
                    if self.method_has_error((c, m)) {
                        continue;
                    }
                    self.mark_process((c, m), None);
                }
            }
        }
        if on(pass_stage::PROCESS_VARIABLES) {
            self.method_pass(visits, &|cfg, env, methods| {
                crate::passes::process_variables::process_variables(cfg, env, methods)
            });
        }
        if on(pass_stage::APPLY_VARIABLE_NAMES) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::codegen::var_names::apply_variable_names(cfg, env.root);
                Ok(())
            });
        }
        if on(pass_stage::PREPARE_FOR_CODEGEN) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::prepare_for_codegen::prepare_for_codegen(cfg, env)
            });
        }
        if timing {
            LATE_PASS_NANOS.fetch_add(t_group.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }
}

/// Decodes one method's bytecode and builds its SSA state (`MethodNode.load` + SSA
/// construction). Only reads `root`/`src`; independent of any other method, so it is safe to
/// call for different `(c, m)` from multiple threads at once as long as they share the same
/// `root`/`src` (see [`ClassDriver::preload_all_methods`]).
/// Bytes of system memory currently free to use, as a budget for [`ClassDriver::preload_all_methods`]:
/// `MemAvailable` from `/proc/meminfo` on Linux, a fixed, deliberately low default elsewhere
/// (understating the budget is the safe direction - it only turns off the preload, never risks
/// memory pressure).
fn available_memory_bytes() -> u64 {
    const DEFAULT_BUDGET: u64 = 2_000_000_000;
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("MemAvailable:") {
                    if let Some(kb) = rest
                        .trim()
                        .strip_suffix("kB")
                        .and_then(|s| s.trim().parse::<u64>().ok())
                    {
                        return kb * 1024;
                    }
                }
            }
        }
    }
    DEFAULT_BUDGET
}

fn decode_method(
    root: &Root,
    src: &dyn CodeSource,
    extract_finally: bool,
    c: usize,
    m: usize,
) -> MethodState {
    match load_method_code(root, src, c, m) {
        Ok(loaded) => build_method_state(root, extract_finally, c, m, loaded),
        Err(e) => MethodState {
            cfg: None,
            failed: Some(e),
        },
    }
}

/// Just the bytecode-reading half of [`decode_method`]: cheap compared to
/// [`build_method_state`], and `loaded.decoded.insns.len()` is a real (not guessed) proxy for
/// how much the SSA build below will cost, used by
/// [`ClassDriver::preload_all_methods`] to bound its memory use on actual size rather than a
/// flat per-method estimate (which a method with unusually many instructions could exceed).
fn load_method_code(
    root: &Root,
    src: &dyn CodeSource,
    c: usize,
    m: usize,
) -> Result<LoadedCode, String> {
    src.load_code(root, c, m)
}

fn build_method_state(
    root: &Root,
    extract_finally: bool,
    c: usize,
    m: usize,
    loaded: LoadedCode,
) -> MethodState {
    let mut frame = FrameInfo::new(root, c, m, loaded.regs_count);
    frame.args_start_reg = loaded.args_start_reg;
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build_ssa(
            loaded.decoded,
            &loaded.tries,
            root,
            c,
            m,
            &frame,
            9,
            extract_finally,
        )
    }));
    match built {
        Ok(Ok(cfg)) => MethodState {
            cfg: Some(cfg),
            failed: None,
        },
        Ok(Err(e)) => MethodState {
            cfg: None,
            failed: Some(e),
        },
        Err(_) => MethodState {
            cfg: None,
            failed: Some("panic while loading".to_string()),
        },
    }
}

/// Stage numbers of the passes: 10 to 19 are the passes ported first (see `build_ssa`), later
/// numbers were assigned in the order the passes were ported. The oracle tools
/// (`tests/oracle/java/PipelineDump.java`) use the same numbers.
pub mod pass_stage {
    pub const MOD_VISITOR: u32 = 10;
    pub const CODE_SHRINK: u32 = 11;
    pub const REGION_MAKER: u32 = 12;
    pub const IF_REGION: u32 = 13;
    pub const RETURN: u32 = 14;
    pub const CLEAN_REGIONS: u32 = 15;
    pub const CODE_SHRINK2: u32 = 16;
    pub const CHECK_REGIONS: u32 = 17;
    pub const LOOP_REGION: u32 = 18;
    pub const SWITCH_BREAK: u32 = 19;
    pub const REPLACE_NEW_ARRAY: u32 = 20;
    pub const DEBOXING: u32 = 21;
    pub const SHADOW_FIELD: u32 = 22;
    pub const GENERIC_TYPES: u32 = 23;
    pub const KOTLIN_INTERNALS: u32 = 24;
    pub const SWITCH_OVER_STRING: u32 = 25;
    pub const METHOD_THROWS: u32 = 26;
    pub const METHOD_INVOKE: u32 = 27;
    pub const SIMPLIFY: u32 = 28;
    pub const FIX_INSTRUCTIONS: u32 = 29;
    pub const PROCESS_VARIABLES: u32 = 30;
    pub const APPLY_VARIABLE_NAMES: u32 = 31;
    pub const PREPARE_FOR_CODEGEN: u32 = 32;
    pub const ANONYMOUS_CLASS: u32 = 33;
    pub const EXTRACT_FIELD_INIT: u32 = 34;
    pub const CLASS_MODIFIER: u32 = 35;
    pub const ENUM_VISITOR: u32 = 36;
    pub const FIX_SWITCH_OVER_ENUM: u32 = 37;
    pub const INLINE_METHODS: u32 = 38;
    pub const FIX_ACCESS_MODIFIERS: u32 = 40;
    pub const MARK_METHODS_FOR_INLINE: u32 = 41;
}
