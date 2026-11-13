//! Execution of a parsed command line: load, save, report, exit code.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use jadx_api::{class_output_path, JadxArgs, JadxDecompiler, JadxError, Level, LogSink};

use super::options::{self, Cli, LogLevel, ParseError};

/// Exit code: success.
pub const EXIT_OK: i32 = 0;
/// Exit code: invalid arguments or a failure of the run.
pub const EXIT_ERROR: i32 = 1;
/// Exit code: nothing could be loaded.
pub const EXIT_LOAD_FAILED: i32 = 2;
/// Exit code: finished, but classes or resources had errors.
pub const EXIT_FINISHED_WITH_ERRORS: i32 = 3;

/// Prints messages like the reference (`LEVEL - message` on stdout), filtered by the log level.
struct ConsoleLog {
    level: LogLevel,
}

impl ConsoleLog {
    fn enabled(&self, l: Level) -> bool {
        let max = match self.level {
            LogLevel::Quiet => return false,
            LogLevel::Progress | LogLevel::Info => Level::Info,
            LogLevel::Error => Level::Error,
            LogLevel::Warn => Level::Warn,
            LogLevel::Debug => Level::Debug,
        };
        l <= max
    }
}

impl LogSink for ConsoleLog {
    fn log(&self, level: Level, msg: &str) {
        if self.enabled(level) {
            println!("{:<5} - {msg}", level.as_str());
        }
    }
}

/// Run `jadx-rs` with the given arguments (without the program name); returns the exit code.
pub fn run(argv: &[String]) -> i32 {
    let cli = match options::parse(argv) {
        Ok(c) => c,
        Err(ParseError::Usage(m)) => {
            eprintln!("Arguments parse error: {m}");
            return EXIT_ERROR;
        }
        Err(ParseError::Invalid(m)) => {
            println!("ERROR - Incorrect arguments: {m}");
            return EXIT_ERROR;
        }
    };
    if cli.help {
        print!("{}", options::usage());
        return EXIT_OK;
    }
    if cli.version {
        println!("{}", JadxDecompiler::version());
        return EXIT_OK;
    }
    execute(cli)
}

fn execute(cli: Cli) -> i32 {
    let log = Arc::new(ConsoleLog {
        level: cli.log_level,
    });
    for name in &cli.unimplemented {
        log.log(
            Level::Warn,
            &format!("Option {name} is accepted but not implemented yet, ignored"),
        );
    }
    let mut jadx = JadxDecompiler::new(cli.args.clone());
    jadx.set_log_sink(log.clone());
    jadx.set_class_generator(std::sync::Arc::new(jadx_api::CoreClassGenerator::new()));
    let timing = std::env::var("JADX_RS_TIMING").is_ok();
    let t0 = std::time::Instant::now();
    if let Err(e) = jadx.load() {
        return report_failure(&*log, e);
    }
    if timing {
        eprintln!("TIMING load: {:?}", t0.elapsed());
    }
    if check_for_errors(&mut jadx, &*log) {
        return EXIT_LOAD_FAILED;
    }
    let t1 = std::time::Instant::now();
    let saved = match single_class(&jadx, &cli, &*log) {
        Ok(true) => Ok(()),
        Ok(false) => save(&jadx, cli.log_level, &*log),
        Err(e) => Err(e),
    };
    if timing {
        eprintln!("TIMING save: {:?}", t1.elapsed());
        jadx_api::core_gen::print_timing_report();
        jadx_core::passes::driver::print_pass_timing_report();
    }
    if let Err(e) = saved {
        return report_failure(&*log, e);
    }
    let errors = jadx.errors_count();
    if errors != 0 {
        jadx.print_errors_report();
        log.log(
            Level::Error,
            &format!("finished with errors, count: {errors}"),
        );
        return EXIT_FINISHED_WITH_ERRORS;
    }
    log.log(Level::Info, "done");
    EXIT_OK
}

fn report_failure(log: &dyn LogSink, e: JadxError) -> i32 {
    match e {
        JadxError::Args(m) => log.log(Level::Error, &format!("Incorrect arguments: {m}")),
        other => log.log(Level::Error, &format!("Process error: {other}")),
    }
    EXIT_ERROR
}

/// `JadxCLI.checkForErrors`: true if nothing can be done.
fn check_for_errors(jadx: &mut JadxDecompiler, log: &dyn LogSink) -> bool {
    if jadx.classes().is_empty() {
        if jadx.args().skip_resources {
            log.log(Level::Error, "Load failed! No classes for decompile!");
            return true;
        }
        if !jadx.args().skip_sources {
            log.log(
                Level::Warn,
                "No classes to decompile; decoding resources only",
            );
            jadx.args_mut().skip_sources = true;
        }
    }
    let errors = jadx.errors_count();
    if errors > 0 {
        log.log(
            Level::Error,
            &format!("Loading finished with errors! Count: {errors}"),
        );
    }
    false
}

/// Progress line printer: at most one update per half second, plus start and end.
struct Progress {
    start: Instant,
    last_ms: AtomicU64,
}

impl Progress {
    fn update(&self, done: usize, total: usize) {
        let now = self.start.elapsed().as_millis() as u64;
        let last = self.last_ms.load(Ordering::Relaxed);
        let first = done == 0;
        let finished = done == total;
        if first || finished || now.saturating_sub(last) >= 500 {
            self.last_ms.store(now, Ordering::Relaxed);
            let pct = (done * 100).checked_div(total).unwrap_or(100);
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "INFO  - progress: {done} of {total} ({pct}%)\r");
            if finished {
                let _ = writeln!(out);
            }
            let _ = out.flush();
        }
    }
}

fn save(jadx: &JadxDecompiler, level: LogLevel, log: &dyn LogSink) -> Result<(), JadxError> {
    if level == LogLevel::Quiet {
        return jadx.save();
    }
    log.log(Level::Info, "processing ...");
    if level == LogLevel::Progress {
        let progress = Progress {
            start: Instant::now(),
            last_ms: AtomicU64::new(0),
        };
        jadx.save_with_progress(Some(&|done, total| progress.update(done, total)))
    } else {
        jadx.save()
    }
}

/// `--single-class` / `--single-class-output`: returns true if a single class was saved.
fn single_class(jadx: &JadxDecompiler, cli: &Cli, log: &dyn LogSink) -> Result<bool, JadxError> {
    if cli.single_class.is_none() && cli.single_class_output.is_none() {
        return Ok(false);
    }
    let args: &JadxArgs = jadx.args();
    let cls = match &cli.single_class {
        Some(name) => {
            let cls = jadx
                .search_class(name)
                .ok_or_else(|| JadxError::Args(format!("Input class not found: {name}")))?;
            if cls.is_no_code() {
                return Err(JadxError::Args(
                    "Input class can't be saved by current jadx settings (marked as DONT_GENERATE)"
                        .into(),
                ));
            }
            if cls.is_inner() {
                let top = cls.top_parent_class();
                log.log(
                    Level::Warn,
                    &format!(
                        "Input class is inner, parent class will be saved: {}",
                        top.full_name()
                    ),
                );
                top
            } else {
                cls
            }
        }
        None => {
            let classes = jadx.classes();
            if classes.len() != 1 {
                return Err(JadxError::Args(format!(
                    "Found {} classes, single class output can't be used",
                    classes.len()
                )));
            }
            classes[0]
        }
    };
    let ext = args.output_format.file_extension();
    let out: PathBuf = match &cli.single_class_output {
        None => {
            let src = args.out_dir_src.clone().unwrap_or_default();
            jadx_api::paths::safe_join(&src, &class_output_path(&cls, args))
                .ok_or_else(|| JadxError::Runtime("Invalid class file name".into()))?
        }
        Some(o) if o.ends_with(ext) => PathBuf::from(o),
        Some(o) => Path::new(o).join(format!("{}{ext}", cls.name())),
    };
    log.log(
        Level::Info,
        &format!(
            "Saving class '{}' to file '{}'",
            cls.full_name(),
            out.display()
        ),
    );
    jadx.save_class_to(cls, &out)
        .map_err(|e| JadxError::Runtime(format!("Class decompilation failed: {e}")))?;
    Ok(true)
}
