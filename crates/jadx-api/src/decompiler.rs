//! The decompiler facade (`JadxDecompiler`): loads the inputs, lists classes, packages and
//! resources, generates class code in parallel and saves the output tree.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jadx_core::model::const_storage::FieldRef;
use jadx_core::model::node::Root;
use jadx_core::passes::pre_decompile::{run_pre_decompile, PreDecompileOptions};
use jadx_core::resources::{add_resource_classes, ResourceField, ResourceInput};
use jadx_dex_input::DexFile;
use jadx_input_api::ConstValue;
use jadx_java_input::JavaClass as JavaInputClass;
use jadx_package::{load_inputs, InputSet, LoadOptions};

use crate::args::{JadxArgs, DEFAULT_OUT_DIR, DEFAULT_RES_DIR, DEFAULT_SRC_DIR};
use crate::code::{ICodeInfo, SimpleCodeInfo};
use crate::error::{ErrorsCounter, JadxError, Result};
use crate::generator::{
    generate_declarations, ClassCodeGenerator, ClassDiagnostics, DeclarationGenerator, GenContext,
};
use crate::input_code::{ClassOrigin, CombinedCodeRefs};
use crate::java::{JavaClass, JavaMethod, JavaPackage};
use crate::log::{Level, LogSink, NullLog};
use crate::parallel::run_indexed;
use crate::paths::{class_alias_file_path, safe_join, save_text};
use crate::resources::{
    load_resources, resource_options, save_resources, AndroidResourceDecoder, ResourceContext,
    ResourceDecoder, ResourceFile, ResourceType, SaveResources,
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
    java_classes: Vec<JavaInputClass>,
    class_origin: Vec<ClassOrigin>,
    root: Root,
    /// Member classes of every class in output order.
    inner: Vec<Vec<usize>>,
    /// Top level classes to save, in output order.
    top_classes: Vec<usize>,
    packages: Vec<PackageData>,
    /// Package of every top level class.
    class_package: HashMap<usize, usize>,
    resources: Vec<ResourceFile>,
    resource_context: ResourceContext,
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
        let inputs: Vec<PathBuf> = expand_dirs(&self.args.input_files)
            .iter()
            // plain dex files are named by their absolute path (shown in the `loaded from` comment)
            .map(|f| std::path::absolute(f).unwrap_or_else(|_| f.clone()))
            .collect();
        let opts = LoadOptions {
            verify_checksum: self.args.verify_dex_checksum,
            ..LoadOptions::default()
        };
        let input_set = load_inputs(&inputs, &opts);
        for w in &input_set.warnings {
            if w.starts_with("File open error") {
                self.log(Level::Error, w);
                self.errors.add_error("input file", w);
            } else {
                self.log(Level::Warn, w);
            }
        }
        let InputSet {
            dex_files: dexes,
            java_classes,
            ..
        } = input_set;

        // the duplicate selection of the reference root: of classes with the same name the one of
        // the lowest `classesN.dex` wins, otherwise the first loaded
        let sources = {
            let mut all = Vec::new();
            for (di, dex) in dexes.iter().enumerate() {
                for def in dex.class_defs().flatten() {
                    if let Ok(desc) = dex.type_descriptor(def.class_idx) {
                        all.push(jadx_package::ClassSource {
                            raw_name: raw_class_name(&desc),
                            source: dex.name().to_owned(),
                            origin: jadx_package::Origin::Dex(di),
                        });
                    }
                }
            }
            for (ji, cls) in java_classes.iter().enumerate() {
                if let Ok(desc) = cls.type_desc() {
                    all.push(jadx_package::ClassSource {
                        raw_name: raw_class_name(&desc),
                        source: cls.source().to_owned(),
                        origin: jadx_package::Origin::Java(ji),
                    });
                }
            }
            all
        };
        let (kept, dup_warnings) = jadx_package::select_from_duplicates(sources);
        for w in &dup_warnings {
            self.log(Level::Warn, w);
        }
        let chosen: HashMap<String, jadx_package::Origin> =
            kept.into_iter().map(|c| (c.raw_name, c.origin)).collect();

        let mut class_data = Vec::new();
        let mut class_dex = Vec::new();
        let mut class_origin = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (di, dex) in dexes.iter().enumerate() {
            for (ci, def) in dex.class_defs().enumerate() {
                let loaded = def
                    .map_err(|e| e.to_string())
                    .and_then(|d| dex.class_api(&d).map_err(|e| e.to_string()));
                match loaded {
                    Ok(data) => {
                        let raw = raw_class_name(&data.type_desc);
                        if chosen.get(&raw) != Some(&jadx_package::Origin::Dex(di))
                            || !seen.insert(data.type_desc.clone())
                        {
                            continue;
                        }
                        class_data.push(data);
                        class_dex.push(di);
                        class_origin.push(ClassOrigin::Dex(di));
                    }
                    Err(e) => {
                        let node = format!("class #{ci} in {}", dex.name());
                        self.log(Level::Error, &format!("Failed to load {node}: {e}"));
                        self.errors.add_error(&node, &e);
                    }
                }
            }
        }
        for (ji, cls) in java_classes.iter().enumerate() {
            match cls.class_api() {
                Ok(data) => {
                    let raw = raw_class_name(&data.type_desc);
                    if chosen.get(&raw) != Some(&jadx_package::Origin::Java(ji))
                        || !seen.insert(data.type_desc.clone())
                    {
                        continue;
                    }
                    class_data.push(data);
                    class_dex.push(usize::MAX);
                    class_origin.push(ClassOrigin::Java(ji));
                }
                Err(e) => {
                    let node = format!("class in {}", cls.source());
                    self.log(Level::Error, &format!("Failed to load {node}: {e}"));
                    self.errors.add_error(&node, &e.to_string());
                }
            }
        }

        let (mut resources, res_warnings) = load_resources(&self.args.input_files);
        for w in &res_warnings {
            self.log(Level::Warn, w);
        }
        // the resource table gives the `R` class its fields and the ids their names
        let table_data = resources
            .iter()
            .find(|r| r.resource_type() == ResourceType::Arsc)
            .and_then(|r| r.load_bytes().ok());
        let table = table_data.as_deref().and_then(|data| {
            match jadx_resources::ResTable::parse(data, &resource_options(&self.args)) {
                Ok(t) => Some(t),
                Err(e) => {
                    self.log(
                        Level::Error,
                        &format!("Failed to parse 'resources.pb'/'.arsc' file: {e}"),
                    );
                    None
                }
            }
        });
        let res_plan = table.as_ref().map(|t| {
            let input = ResourceInput {
                app_package: t.storage().app_package().map(str::to_string),
                names: t.resource_names(),
                fields: t
                    .r_fields()
                    .into_iter()
                    .map(|f| ResourceField {
                        id: f.id,
                        type_name: f.type_name,
                        name: f.name,
                    })
                    .collect(),
            };
            add_resource_classes(&mut class_data, &input, self.args.move_inner_classes)
        });
        class_dex.resize(class_data.len(), usize::MAX);
        class_origin.resize(class_data.len(), ClassOrigin::Synthetic);

        let mut root = Root::build(class_data, self.args.move_inner_classes)
            .map_err(|e| JadxError::Load(format!("Failed to build class tree: {e}")))?;
        if let Some(plan) = res_plan {
            root.apply_resource_plan(plan);
        }
        let pre = PreDecompileOptions {
            respect_bytecode_acc_modifiers: self.args.respect_bytecode_acc_modifiers,
            replace_consts: self.args.replace_consts,
            inline_anonymous_classes: self.args.inline_anonymous_classes,
            inline_methods: self.args.inline_methods,
            allow_inline_kotlin_lambda: self.args.allow_inline_kotlin_lambda,
            deobf: self.args.deobfuscation_on.then(|| {
                jadx_core::passes::rename::DeobfOptions::from_whitelist(
                    self.args.deobfuscation_min_length,
                    self.args.deobfuscation_max_length,
                    &self.args.deobfuscation_whitelist,
                )
            }),
            ..PreDecompileOptions::default()
        };
        {
            let refs = CombinedCodeRefs {
                dexes: &dexes,
                java_classes: &java_classes,
                origin: &class_origin,
            };
            run_pre_decompile(&mut root, &refs, &pre);
        }

        let mut resource_context = ResourceContext::default();
        if !self.args.skip_resources {
            if let Some(t) = &table {
                // files that the table names (obfuscated or shrunk resource file names)
                let aliases = jadx_resources::file_alias_map(t);
                for r in &mut resources {
                    if let Some(alias) = aliases.get(r.original_name()) {
                        r.set_alias(alias);
                    }
                }
                resource_context = self.resource_context(&mut root, table_data.as_deref());
            }
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
            java_classes,
            class_origin,
            root,
            inner,
            top_classes,
            packages,
            class_package,
            resources,
            resource_context,
        });
        Ok(())
    }

    /// Tables of the decompiled code for the resource decoder: the names of the constants that
    /// carry resource ids and the renamed classes. Resource ids whose constant gets another name
    /// from the table rename the field (`ResTableBinaryParser.getResName`).
    fn resource_context(&self, root: &mut Root, table_data: Option<&[u8]>) -> ResourceContext {
        let mut ctx = ResourceContext::default();
        let mut fields: HashMap<i32, (usize, usize)> = HashMap::new();
        for (value, fld) in root.const_storage.global_const_fields() {
            if let (ConstValue::Int(id), FieldRef::Node(c, f)) = (value, fld) {
                if !root.classes[root.top_parent_index(*c)].synthetic {
                    ctx.code_names
                        .insert(*id, root.classes[*c].fields[*f].info.name.clone());
                    fields.insert(*id, (*c, *f));
                }
            }
        }
        for c in &root.classes {
            let alias = root.class_full_name(&c.info);
            let raw = c.info.raw_name();
            if alias != raw {
                ctx.class_aliases.insert(raw, alias);
            }
        }
        let mut opts = resource_options(&self.args);
        opts.code_names = ctx.code_names.clone();
        if let Some(data) = table_data {
            if let Ok(t) = jadx_resources::ResTable::parse(data, &opts) {
                for (id, name) in t.field_renames() {
                    if let Some(&(c, f)) = fields.get(id) {
                        root.classes[c].fields[f].alias = Some(name.clone());
                    }
                }
            }
        }
        ctx
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
            java_classes: &l.java_classes,
            class_origin: &l.class_origin,
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
        let (src_dir, res_dir, gradle) = if let Some(export_type) = self.args.export_gradle_type {
            let project_dir = self
                .args
                .out_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT_DIR));
            let effective = crate::export::effective_export_type(Some(export_type), &l.resources);
            self.log(
                Level::Info,
                &format!("Export Gradle project using '{effective}' template"),
            );
            let out_dirs = crate::export::init_out_dirs(effective, &project_dir);
            (out_dirs.src, out_dirs.res, Some((project_dir, effective)))
        } else {
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
            (src_dir, res_dir, None)
        };
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
                    context: &l.resource_context,
                    log: &*self.log,
                    errors: &self.errors,
                    progress: None,
                },
            );
        }
        if sources {
            self.save_sources_to(l, &src_dir, progress);
        }
        if let Some((project_dir, effective)) = gradle {
            self.generate_gradle_project(l, &project_dir, &res_dir, effective);
        }
        Ok(())
    }

    /// Writes the Gradle project files (`build.gradle`, `settings.gradle`, `gradle.properties`)
    /// around the sources/resources already saved below `res_dir`/`src_dir`
    /// (`jadx.core.export.ExportGradle.generateGradleFiles`).
    fn generate_gradle_project(
        &self,
        l: &Loaded,
        project_dir: &Path,
        res_dir: &Path,
        export_type: crate::args::ExportGradleType,
    ) {
        use crate::args::ExportGradleType;
        let want_app = export_type != ExportGradleType::AndroidLibrary;
        let manifest_text = std::fs::read_to_string(res_dir.join("AndroidManifest.xml")).ok();
        let strings_text = find_strings_xml(res_dir);
        if manifest_text.is_none()
            && matches!(
                export_type,
                ExportGradleType::AndroidApp | ExportGradleType::AndroidLibrary
            )
        {
            self.log(
                Level::Warn,
                "AndroidManifest.xml not found, exported files will contain 'UNKNOWN' fields",
            );
        }
        let application_params = crate::export::parse_application_params(
            manifest_text.as_deref(),
            strings_text.as_deref(),
            want_app,
        );
        let input = crate::export::GradleExportInput {
            args: &self.args,
            project_dir,
            resources: &l.resources,
            app_package: l.root.app_package.as_deref(),
            gradle_info: crate::export::GradleInfoStorage::default(),
            application_params,
        };
        if let Err(e) = crate::export::generate_gradle_files(&input) {
            self.log(Level::Error, &format!("Gradle export failed: {e}"));
        }
    }

    fn save_sources_to(&self, l: &Loaded, src_dir: &Path, progress: Option<ProgressFn<'_>>) {
        let list = crate::scheduler::schedule(&l.root, &self.classes_to_save(l));
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

/// Dotted raw class name of a type descriptor (`Lp/A$B;` to `p.A$B`).
fn raw_class_name(desc: &str) -> String {
    desc.strip_prefix('L')
        .and_then(|d| d.strip_suffix(';'))
        .unwrap_or(desc)
        .replace('/', ".")
}

/// Finds a `strings.xml` file below `res_dir` (depth-bounded breadth-first search), preferring
/// `res/values/strings.xml` to match the reference's `ApplicationParams` string lookup.
fn find_strings_xml(res_dir: &Path) -> Option<String> {
    let preferred = res_dir.join("res/values/strings.xml");
    if let Ok(text) = std::fs::read_to_string(&preferred) {
        return Some(text);
    }
    let mut dirs = vec![res_dir.to_path_buf()];
    for _ in 0..6 {
        let mut next = Vec::new();
        for dir in &dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    next.push(path);
                } else if path.file_name().and_then(|n| n.to_str()) == Some("strings.xml") {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        return Some(text);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        dirs = next;
    }
    None
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
