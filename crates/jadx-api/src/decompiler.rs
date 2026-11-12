//! The decompiler facade (`JadxDecompiler`): loads the inputs, lists classes, packages and
//! resources, generates class code in parallel and saves the output tree.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jadx_core::model::node::Root;
use jadx_core::passes::pre_decompile::{run_pre_decompile, PreDecompileOptions};
use jadx_dex_input::{load_path, DexFile, LoadOptions};

use crate::args::{JadxArgs, OutputFormat, DEFAULT_OUT_DIR, DEFAULT_RES_DIR, DEFAULT_SRC_DIR};
use crate::code::{ICodeInfo, SimpleCodeInfo};
use crate::error::{ErrorsCounter, JadxError, Result};
use crate::generator::{
    generate_declarations, ClassCodeGenerator, ClassDiagnostics, DeclarationGenerator, GenContext,
};
use crate::java::{JavaClass, JavaMethod, JavaPackage};
use crate::log::{Level, LogSink, NullLog};
use crate::parallel::run_indexed;
use crate::paths::{class_alias_file_path, safe_join, save_text};
use crate::resources::{
    load_resources, save_resources, AndroidResourceDecoder, ResourceDecoder, ResourceFile,
    SaveResources,
};

/// Progress callback: `(done, total)`.
pub type ProgressFn<'a> = &'a (dyn Fn(usize, usize) + Sync);

/// Outcome of the code generation of one class.
#[derive(Debug, Clone)]
pub struct ClassResult {
    pub code: SimpleCodeInfo,
    pub diag: ClassDiagnostics,
}

#[derive(Debug)]
pub(crate) struct PackageData {
    pub full_name: String,
    pub parent: Option<usize>,
    pub sub_packages: Vec<usize>,
    pub classes: Vec<usize>,
}

struct Loaded {
    dexes: Vec<DexFile>,
    class_dex: Vec<usize>,
    root: Root,
    /// Member classes of every class in output order.
    inner: Vec<Vec<usize>>,
    /// Top level classes to save, in output order.
    top_classes: Vec<usize>,
    packages: Vec<PackageData>,
    /// Package of every top level class.
    class_package: HashMap<usize, usize>,
    resources: Vec<ResourceFile>,
}

/// Decompiler facade. Create with [`JadxArgs`], call [`load`](JadxDecompiler::load), then list
/// classes or [`save`](JadxDecompiler::save) the output.
pub struct JadxDecompiler {
    args: JadxArgs,
    loaded: Option<Loaded>,
    errors: ErrorsCounter,
    generator: Arc<dyn ClassCodeGenerator>,
    resource_decoder: Option<Arc<dyn ResourceDecoder>>,
    log: Arc<dyn LogSink>,
    code_cache: Mutex<HashMap<usize, Arc<ClassResult>>>,
}

impl JadxDecompiler {
    pub fn new(args: JadxArgs) -> Self {
        JadxDecompiler {
            args,
            loaded: None,
            errors: ErrorsCounter::new(),
            generator: Arc::new(DeclarationGenerator),
            resource_decoder: Some(Arc::new(AndroidResourceDecoder)),
            log: Arc::new(NullLog),
            code_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Version of the decompiler.
    pub fn version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub fn args(&self) -> &JadxArgs {
        &self.args
    }

    pub fn args_mut(&mut self) -> &mut JadxArgs {
        &mut self.args
    }

    /// Replace the class code generator.
    pub fn set_class_generator(&mut self, generator: Arc<dyn ClassCodeGenerator>) {
        self.generator = generator;
        self.code_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Replace the resource decoder; `None` copies all resources unchanged.
    pub fn set_resource_decoder(&mut self, decoder: Option<Arc<dyn ResourceDecoder>>) {
        self.resource_decoder = decoder;
    }

    pub fn set_log_sink(&mut self, log: Arc<dyn LogSink>) {
        self.log = log;
    }

    fn log(&self, level: Level, msg: &str) {
        self.log.log(level, msg);
    }

    // ------------------------------------------------------------------ loading

    /// Validate the arguments and load classes and resources of the input files.
    pub fn load(&mut self) -> Result<()> {
        self.loaded = None;
        self.errors.clear();
        self.code_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.validate_args()?;
        self.log(Level::Info, "loading ...");
        let inputs = expand_dirs(&self.args.input_files);
        let opts = LoadOptions {
            verify_checksum: self.args.verify_dex_checksum,
        };
        let mut dexes: Vec<DexFile> = Vec::new();
        for file in &inputs {
            // plain dex files are named by their absolute path (shown in the `loaded from` comment)
            let abs = std::path::absolute(file).unwrap_or_else(|_| file.clone());
            match load_path(&abs, &opts) {
                Ok(res) => {
                    for w in &res.warnings {
                        self.log(Level::Warn, w);
                    }
                    dexes.extend(res.dex_files);
                }
                Err(e) => {
                    let msg = format!("Failed to load {}: {e}", file.display());
                    self.log(Level::Error, &msg);
                    self.errors
                        .add_error(&format!("file {}", file.display()), &msg);
                }
            }
        }

        let mut class_data = Vec::new();
        let mut class_dex = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (di, dex) in dexes.iter().enumerate() {
            for (ci, def) in dex.class_defs().enumerate() {
                let loaded = def
                    .map_err(|e| e.to_string())
                    .and_then(|d| dex.class_api(&d).map_err(|e| e.to_string()));
                match loaded {
                    Ok(data) => {
                        if !seen.insert(data.type_desc.clone()) {
                            self.log(
                                Level::Warn,
                                &format!(
                                    "Duplicate class {} in {}, skipped",
                                    data.type_desc,
                                    dex.name()
                                ),
                            );
                            continue;
                        }
                        class_data.push(data);
                        class_dex.push(di);
                    }
                    Err(e) => {
                        let node = format!("class #{ci} in {}", dex.name());
                        self.log(Level::Error, &format!("Failed to load {node}: {e}"));
                        self.errors.add_error(&node, &e);
                    }
                }
            }
        }

        let mut root = Root::build(class_data, self.args.move_inner_classes)
            .map_err(|e| JadxError::Load(format!("Failed to build class tree: {e}")))?;
        let pre = PreDecompileOptions {
            respect_bytecode_acc_modifiers: self.args.respect_bytecode_acc_modifiers,
            replace_consts: self.args.replace_consts,
            inline_anonymous_classes: self.args.inline_anonymous_classes,
            inline_methods: self.args.inline_methods,
            allow_inline_kotlin_lambda: self.args.allow_inline_kotlin_lambda,
            ..PreDecompileOptions::default()
        };
        {
            let refs = jadx_dex_input::DexCodeRefs {
                dexes: &dexes,
                class_dex: &class_dex,
            };
            run_pre_decompile(&mut root, &refs, &pre);
        }

        let (resources, res_warnings) = load_resources(&self.args.input_files);
        for w in &res_warnings {
            self.log(Level::Warn, w);
        }
        if resources
            .iter()
            .any(|r| r.original_name().ends_with(".class"))
            && dexes.is_empty()
        {
            self.log(
                Level::Warn,
                "Java bytecode (.class) inputs are not supported yet, no classes loaded",
            );
        }

        let sorted = root.sorted_class_order();
        let mut inner: Vec<Vec<usize>> = vec![Vec::new(); root.classes.len()];
        let mut top_classes = Vec::new();
        for &i in &sorted {
            let c = &root.classes[i];
            if c.dont_generate {
                continue;
            }
            match root.parent_index(i) {
                Some(p) if p != i => inner[p].push(i),
                _ => top_classes.push(i),
            }
        }
        let (packages, class_package) = build_packages(&root, &top_classes);
        self.loaded = Some(Loaded {
            dexes,
            class_dex,
            root,
            inner,
            top_classes,
            packages,
            class_package,
            resources,
        });
        Ok(())
    }

    /// `JadxArgsValidator`: input files, output directories.
    fn validate_args(&mut self) -> Result<()> {
        if self.args.input_files.is_empty() {
            return Err(JadxError::Args("Please specify input file".into()));
        }
        for f in &self.args.input_files {
            if !f.exists() {
                let abs = std::fs::canonicalize(f)
                    .or_else(|_| std::env::current_dir().map(|d| d.join(f)))
                    .unwrap_or_else(|_| f.clone());
                return Err(JadxError::Args(format!("File not found {}", abs.display())));
            }
        }
        if self.args.output_format == OutputFormat::Json {
            return Err(JadxError::Args(
                "JSON output format is not implemented yet".into(),
            ));
        }
        if self.args.threads_count == 0 {
            return Err(JadxError::Args(
                "Threads count must be positive, got: 0".into(),
            ));
        }
        let a = &mut self.args;
        if a.out_dir.is_none() {
            a.out_dir = Some(match (&a.out_dir_src, &a.out_dir_res) {
                (Some(src), _) => src.clone(),
                (None, Some(res)) => res.clone(),
                (None, None) => {
                    let dir = make_dir_from_input(&a.input_files);
                    self.log
                        .log(Level::Info, &format!("output directory: {}", dir.display()));
                    dir
                }
            });
        }
        let a = &mut self.args;
        let out = a.out_dir.clone().expect("set above");
        if a.out_dir_src.is_none() {
            a.out_dir_src = Some(out.join(DEFAULT_SRC_DIR));
        }
        if a.out_dir_res.is_none() {
            a.out_dir_res = Some(out.join(DEFAULT_RES_DIR));
        }
        for (dir, desc) in [
            (&a.out_dir, "Output"),
            (&a.out_dir_src, "Source output"),
            (&a.out_dir_res, "Resources output"),
        ] {
            if let Some(d) = dir {
                if d.exists() && !d.is_dir() {
                    return Err(JadxError::Args(format!(
                        "{desc} directory exists as file {}",
                        d.display()
                    )));
                }
            }
        }
        Ok(())
    }

    fn loaded(&self) -> Result<&Loaded> {
        self.loaded
            .as_ref()
            .ok_or_else(|| JadxError::Runtime("No loaded files".into()))
    }

    pub(crate) fn loaded_root(&self) -> &Root {
        &self
            .loaded
            .as_ref()
            .expect("decompiler is loaded: class views only exist after load()")
            .root
    }

    /// The class tree after loading.
    pub fn root(&self) -> Option<&Root> {
        self.loaded.as_ref().map(|l| &l.root)
    }

    pub(crate) fn inner_class_indexes(&self, idx: usize) -> &[usize] {
        &self.loaded.as_ref().expect("loaded").inner[idx]
    }

    pub(crate) fn package_data(&self, idx: usize) -> &PackageData {
        &self.loaded.as_ref().expect("loaded").packages[idx]
    }

    pub(crate) fn package_of_class(&self, cls: usize) -> Option<JavaPackage<'_>> {
        let l = self.loaded.as_ref()?;
        let top = l.root.top_parent_index(cls);
        l.class_package
            .get(&top)
            .map(|&p| JavaPackage::new(self, p))
    }

    pub(crate) fn methods_of(&self, ids: &[(usize, usize)]) -> Vec<JavaMethod<'_>> {
        ids.iter()
            .map(|&(c, m)| JavaClass::new(self, c).methods().remove(m))
            .collect()
    }

    // ------------------------------------------------------------------ listing

    /// Top level classes that are saved, sorted by raw name.
    pub fn classes(&self) -> Vec<JavaClass<'_>> {
        match &self.loaded {
            Some(l) => l
                .top_classes
                .iter()
                .map(|&i| JavaClass::new(self, i))
                .collect(),
            None => Vec::new(),
        }
    }

    /// All classes including member classes and classes without own code.
    pub fn classes_with_inners(&self) -> Vec<JavaClass<'_>> {
        match &self.loaded {
            Some(l) => (0..l.root.classes.len())
                .map(|i| JavaClass::new(self, i))
                .collect(),
            None => Vec::new(),
        }
    }

    /// All packages (including empty parent packages), sorted by full name.
    pub fn packages(&self) -> Vec<JavaPackage<'_>> {
        match &self.loaded {
            Some(l) => (0..l.packages.len())
                .map(|i| JavaPackage::new(self, i))
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn resources(&self) -> &[ResourceFile] {
        match &self.loaded {
            Some(l) => &l.resources,
            None => &[],
        }
    }

    /// Find a class by full name (`a.b.Outer.Inner`) or raw name (`a.b.Outer$Inner`).
    pub fn search_class(&self, name: &str) -> Option<JavaClass<'_>> {
        let l = self.loaded.as_ref()?;
        let idx = l.root.class_index(name).or_else(|| {
            l.root
                .classes
                .iter()
                .position(|c| c.info.full_name() == name || c.info.raw_full_name() == name)
        })?;
        Some(JavaClass::new(self, idx))
    }

    pub fn errors_count(&self) -> usize {
        self.errors.error_count()
    }

    pub fn warns_count(&self) -> usize {
        self.errors.warns_count()
    }

    pub fn errors(&self) -> &ErrorsCounter {
        &self.errors
    }

    /// Print the errors and warnings summary to the log sink.
    pub fn print_errors_report(&self) {
        self.errors.print_report(&*self.log);
    }

    // ------------------------------------------------------------------ generation

    fn generate_class(&self, idx: usize) -> ClassResult {
        let l = self.loaded.as_ref().expect("loaded");
        let ctx = GenContext {
            root: &l.root,
            dexes: &l.dexes,
            class_dex: &l.class_dex,
            args: &self.args,
            inner_classes: &l.inner,
        };
        let mut diag = ClassDiagnostics::default();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.generator.generate(&ctx, idx, &mut diag)
        }));
        let failure = match outcome {
            Ok(Ok(code)) => {
                self.record(idx, &diag);
                return ClassResult { code, diag };
            }
            Ok(Err(msg)) => msg,
            Err(p) => {
                let detail = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                format!("Code generation failed: {detail}")
            }
        };
        diag.error(failure.clone());
        self.record(idx, &diag);
        let decl = generate_declarations(&ctx, idx);
        let code = SimpleCodeInfo::new(format!(
            "/* JADX ERROR: {} */\n{}",
            failure.replace("*/", "* /"),
            decl.code_str()
        ));
        ClassResult { code, diag }
    }

    fn record(&self, idx: usize, diag: &ClassDiagnostics) {
        let node = format!("class {}", self.loaded_root().classes[idx].info.full_name());
        for e in &diag.errors {
            self.errors.add_error(&node, e);
        }
        for w in &diag.warnings {
            self.errors.add_warn(&node, w);
        }
    }

    /// Generate the code of a class and keep it for later calls.
    pub(crate) fn decompile_class(&self, idx: usize) -> Arc<ClassResult> {
        if let Some(r) = self
            .code_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&idx)
        {
            return r.clone();
        }
        let r = Arc::new(self.generate_class(idx));
        self.code_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(idx, r.clone());
        r
    }

    pub(crate) fn unload_class(&self, idx: usize) {
        self.code_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&idx);
    }

    // ------------------------------------------------------------------ saving

    /// Classes selected for saving (class filter applied).
    fn classes_to_save(&self, l: &Loaded) -> Vec<usize> {
        let Some(filter) = &self.args.class_filter else {
            return l.top_classes.clone();
        };
        let mut keep: HashSet<usize> = HashSet::new();
        for &i in &l.top_classes {
            if (filter.0)(l.root.classes[i].info.full_name()) {
                keep.insert(i);
                if self.args.include_dependencies {
                    keep.extend(l.root.classes[i].dependencies.iter().copied());
                }
            }
        }
        l.top_classes
            .iter()
            .copied()
            .filter(|i| keep.contains(i))
            .collect()
    }

    /// Save sources and resources as configured by the arguments.
    pub fn save(&self) -> Result<()> {
        self.save_with_progress(None)
    }

    /// Like [`save`](Self::save), reporting `(done, total)` classes while sources are written.
    pub fn save_with_progress(&self, progress: Option<ProgressFn<'_>>) -> Result<()> {
        self.save_parts(!self.args.skip_sources, !self.args.skip_resources, progress)
    }

    pub fn save_sources(&self) -> Result<()> {
        self.save_parts(true, false, None)
    }

    pub fn save_resources(&self) -> Result<()> {
        self.save_parts(false, true, None)
    }

    fn save_parts(
        &self,
        sources: bool,
        resources: bool,
        progress: Option<ProgressFn<'_>>,
    ) -> Result<()> {
        let l = self.loaded()?;
        if self.args.export_gradle_type.is_some() {
            self.log(
                Level::Warn,
                "Gradle export is not implemented yet, saving sources and resources only",
            );
        }
        let src_dir = self
            .args
            .out_dir_src
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT_DIR).join(DEFAULT_SRC_DIR));
        let res_dir = self
            .args
            .out_dir_res
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT_DIR).join(DEFAULT_RES_DIR));
        if !self.args.skip_files_save {
            std::fs::create_dir_all(&src_dir)?;
            std::fs::create_dir_all(&res_dir)?;
        }
        // resources first: decompilation can stop or fail
        if resources && !self.args.skip_files_save {
            save_resources(
                &l.resources,
                &SaveResources {
                    args: &self.args,
                    out_dir: &res_dir,
                    decoder: self.resource_decoder.as_deref(),
                    log: &*self.log,
                    errors: &self.errors,
                    progress: None,
                },
            );
        }
        if sources {
            self.save_sources_to(l, &src_dir, progress);
        }
        Ok(())
    }

    fn save_sources_to(&self, l: &Loaded, src_dir: &Path, progress: Option<ProgressFn<'_>>) {
        let list = self.classes_to_save(l);
        let total = list.len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        if let Some(p) = progress {
            p(0, total);
        }
        run_indexed(
            total,
            self.args.threads_count,
            || (),
            |_, i| {
                let idx = list[i];
                self.save_class(l, idx, src_dir);
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if let Some(p) = progress {
                    p(n, total);
                }
            },
        );
    }

    fn save_class(&self, l: &Loaded, idx: usize, src_dir: &Path) {
        let node = &l.root.classes[idx];
        if node.dont_generate {
            return;
        }
        let result = self.generate_class(idx);
        if result.code.is_empty() || self.args.skip_files_save {
            return;
        }
        let rel = format!(
            "{}{}",
            class_alias_file_path(&l.root, &node.info, self.args.is_rename_valid()),
            self.args.output_format.file_extension()
        );
        let Some(path) = safe_join(src_dir, &rel) else {
            let msg = format!("Invalid class file name: {rel:?}");
            self.log(Level::Error, &msg);
            self.errors
                .add_error(&format!("class {}", node.info.full_name()), &msg);
            return;
        };
        if let Err(e) = save_text(&path, result.code.code_str()) {
            let msg = format!("Error saving class: {}: {e}", node.info.full_name());
            self.log(Level::Error, &msg);
            self.errors
                .add_error(&format!("class {}", node.info.full_name()), &msg);
        }
    }

    /// Generate and save one class (a member class saves its top level class) to `out`.
    /// Returns the path written.
    pub fn save_class_to(&self, cls: JavaClass<'_>, out: &Path) -> Result<()> {
        let top = cls.top_parent_class();
        let result = self.generate_class(top.index());
        save_text(out, result.code.code_str())?;
        Ok(())
    }
}

/// Path of the class file below the sources directory for a class of the loaded input.
pub fn class_output_path(cls: &JavaClass<'_>, args: &JadxArgs) -> String {
    format!(
        "{}{}",
        class_alias_file_path(
            cls.decompiler().loaded_root(),
            &cls.class_node().info,
            args.is_rename_valid()
        ),
        args.output_format.file_extension()
    )
}

/// Output directory name derived from the first input file (`JadxArgsValidator`).
fn make_dir_from_input(inputs: &[PathBuf]) -> PathBuf {
    let Some(first) = inputs.first() else {
        return PathBuf::from(DEFAULT_OUT_DIR);
    };
    let name = first
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(pos) => PathBuf::from(&name[..pos]),
        None => PathBuf::from(format!("{name}-{DEFAULT_OUT_DIR}")),
    }
}

/// Replace directories by the files below them (sorted, recursively).
fn expand_dirs(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out);
        } else {
            out.push(p.clone());
        }
    }
    out
}

fn build_packages(root: &Root, top_classes: &[usize]) -> (Vec<PackageData>, HashMap<usize, usize>) {
    // full name -> classes; all parent packages exist as well
    let mut names: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for &c in top_classes {
        let pkg = root.classes[c].info.package().to_string();
        names.entry(pkg).or_default().push(c);
    }
    let own: Vec<String> = names.keys().cloned().collect();
    for pkg in own {
        let mut cur = pkg.as_str();
        while let Some(dot) = cur.rfind('.') {
            cur = &cur[..dot];
            names.entry(cur.to_string()).or_default();
        }
    }
    let index: HashMap<String, usize> = names
        .keys()
        .enumerate()
        .map(|(i, k)| (k.clone(), i))
        .collect();
    let mut data: Vec<PackageData> = names
        .iter()
        .map(|(k, classes)| PackageData {
            full_name: k.clone(),
            parent: k.rfind('.').map(|d| index[&k[..d]]),
            sub_packages: Vec::new(),
            classes: classes.clone(),
        })
        .collect();
    for i in 0..data.len() {
        if let Some(p) = data[i].parent {
            data[p].sub_packages.push(i);
        }
    }
    let mut class_package = HashMap::new();
    for (i, d) in data.iter().enumerate() {
        for &c in &d.classes {
            class_package.insert(c, i);
        }
    }
    (data, class_package)
}
