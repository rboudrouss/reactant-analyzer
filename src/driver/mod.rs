//! The shared frontend driver (ADR-022 §6): the whole `check` composition —
//! project context, discovery, lowering, analysis, rule pass, counting,
//! exit policy, rendering — behind one function used verbatim by the native
//! CLI and the WASM entry point, so their behavior cannot fork.
//!
//! The host owns everything OS-flavored: argv parsing, config discovery and
//! override resolution (done *before* calling in, on the registry),
//! tty/`NO_COLOR` sniffing (resolved into [`CheckOptions::color`]), stream
//! writing and the process exit. The driver only reads through the
//! [`FileSystem`] seam and returns buffered streams.

mod blind_spots;
mod human;
mod json;
mod locations;
pub mod palette;
mod report;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::{ComponentRegistry, Config, RootStrategy, SymbolGraph};
use crate::project::{self, ProjectKind};
use crate::resolver::{
    DefaultFileDiscoverer, FileDiscoverer, FileSystem, analyze_lowered, lower_files_with,
};
use crate::rules::{Diagnostic, ProgramCache, RuleRegistry, SafeCheck, Severity};

pub use blind_spots::BlindSpot;
pub use palette::Palette;
pub use report::{CheckReport, ComponentReport};

pub const EXIT_OK: i32 = 0;
pub const EXIT_FINDINGS: i32 = 1;
pub const EXIT_USAGE: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOn {
    Error,
    Warning,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectOverride {
    Auto,
    Vite,
    NextJs,
    Plain,
}

/// Resolved `check` options — CLI flags and config values already merged by
/// the host (flags win, ADR-022 §5).
pub struct CheckOptions {
    pub info: bool,
    pub show_clean: bool,
    pub trace: bool,
    pub verbose: bool,
    pub all_roots: bool,
    pub entry: Vec<String>,
    pub format: ReportFormat,
    pub fail_on: FailOn,
    pub project: ProjectOverride,
    /// Resolved by the host (tty + NO_COLOR + --no-color); the driver never
    /// sniffs the environment.
    pub color: bool,
}

/// Buffered run result: the host writes the streams and exits.
pub struct CheckOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl CheckOutput {
    fn usage(stderr: String) -> Self {
        CheckOutput {
            stdout: String::new(),
            stderr,
            exit_code: EXIT_USAGE,
        }
    }
}

/// The full check pipeline. `registry` arrives ready (natives + packs +
/// overrides installed); `paths` are the positional arguments, replayed
/// identically by every host; `display` renders paths for output (the CLI
/// passes a cwd-relativizer, WASM the identity).
pub fn run_check(
    fs: Arc<dyn FileSystem>,
    paths: &[String],
    registry: &RuleRegistry,
    opts: &CheckOptions,
    display: &dyn Fn(&Path) -> String,
) -> CheckOutput {
    let mut err = String::new();

    // ── Project context ───────────────────────────────────────────────────────
    // The first directory argument drives project detection; other paths are
    // discovered as-is with the same resolver.
    let project_root = paths
        .iter()
        .map(Path::new)
        .find(|p| fs.is_dir(p))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let forced = match opts.project {
        ProjectOverride::Auto => None,
        ProjectOverride::Vite => Some(ProjectKind::Vite),
        ProjectOverride::NextJs => Some(ProjectKind::NextJs),
        ProjectOverride::Plain => Some(ProjectKind::Plain),
    };
    // Forcing a build-tool kind whose marker file is absent is honored (the
    // tsconfig aliases are usually still right) but said out loud.
    if let Some(kind @ (ProjectKind::Vite | ProjectKind::NextJs)) = forced
        && project::detect(&project_root, fs.as_ref()) != kind
    {
        let (flag, marker) = match kind {
            ProjectKind::NextJs => ("next", "next.config.*"),
            _ => ("vite", "vite.config.*"),
        };
        let _ = writeln!(
            err,
            "[warn] --project {flag}: no {marker} found in {} — still trying tsconfig paths",
            project_root.display()
        );
    }
    let ctx = project::build_context(&project_root, forced, fs.clone());
    // Everything this run knows it did not read, gathered as it is discovered.
    // A non-empty list is what stops the summary claiming a clean bill.
    let mut blind: Vec<BlindSpot> = Vec::new();
    if let Some(warning) = &ctx.alias_warning {
        let _ = writeln!(err, "[warn] {warning}");
        blind.push(BlindSpot::unresolved_aliases(warning));
    }
    if opts.verbose && ctx.kind != ProjectKind::Plain {
        let _ = writeln!(
            err,
            "[verbose] {} project: discovery root {}, tsconfig aliases {}",
            match ctx.kind {
                ProjectKind::NextJs => "next.js",
                _ => "vite",
            },
            ctx.discovery_root.display(),
            if ctx.alias_warning.is_none() {
                "loaded"
            } else {
                "unavailable"
            }
        );
    }

    // ── Discovery ─────────────────────────────────────────────────────────────
    let discoverer = DefaultFileDiscoverer::new(fs.clone());
    let mut files: Vec<PathBuf> = Vec::new();
    for input in paths {
        let p = Path::new(input);
        if fs.is_dir(p) {
            // The project-root dir may be narrowed (vite/next → <root>/src).
            let walk_root = if *p == *project_root {
                &ctx.discovery_root
            } else {
                p
            };
            let found = discoverer.discover(walk_root);
            if found.is_empty() {
                let _ = writeln!(err, "[error] no .ts/.tsx/.js/.jsx files found in {input}");
                return CheckOutput::usage(err);
            }
            files.extend(found);
        } else if fs.is_file(p) {
            files.push(p.to_path_buf());
        } else {
            let _ = writeln!(err, "[error] no such file or directory: {input}");
            return CheckOutput::usage(err);
        }
    }

    // ── Lower ─────────────────────────────────────────────────────────────────
    let mut lowered = lower_files_with(fs.as_ref(), &files, ctx.resolver.as_ref());
    // A file the parser recovered from is noise, and stays on the human
    // channel. A *dropped* file is not: everything it held is a silent false
    // negative, so it is reported whatever the format — stderr is a separate
    // stream, so stdout stays exactly one JSON document.
    let mut dropped = 0usize;
    for e in &lowered.parse_errors {
        if e.analyzed {
            if opts.format == ReportFormat::Human {
                let _ = writeln!(err, "[parse error] {}: {}", e.file.display(), e.message);
            }
        } else {
            dropped += 1;
            let _ = writeln!(
                err,
                "[skipped] {}: {} — the file was not analyzed",
                e.file.display(),
                e.message
            );
        }
    }
    if dropped > 0 {
        blind.push(BlindSpot::unparsed_files(dropped));
    }

    // An import the resolver mapped to a real file that discovery never
    // reached: the pipeline knew where the code was and did not read it (#9).
    // Read off the resolved edges rather than re-resolving, so this cannot
    // disagree with what lowering actually did.
    let analysed: std::collections::HashSet<PathBuf> = files
        .iter()
        .map(|p| crate::resolver::normalize(p))
        .collect();
    let unread: std::collections::BTreeSet<&PathBuf> = lowered
        .module_table
        .paths()
        .filter_map(|p| lowered.module_table.facts(p))
        .flat_map(|f| f.imports.iter())
        .filter(|dep| !analysed.contains(*dep))
        .collect();
    if !unread.is_empty() {
        let examples: Vec<String> = unread.iter().take(3).map(|p| display(p)).collect();
        blind.push(BlindSpot::unread_imports(&examples, unread.len()));
    }

    let strategy = if !opts.entry.is_empty() {
        RootStrategy::Explicit(opts.entry.iter().map(|s| s.trim().to_string()).collect())
    } else if opts.all_roots {
        RootStrategy::AllComponents
    } else {
        RootStrategy::Heuristic
    };

    // Display-name → (file, hook count) map, built before analysis consumes
    // the components. Keyed by display name to disambiguate same-named
    // components across files.
    let temp_registry = ComponentRegistry::from_components(lowered.components.clone());
    let mut component_meta: HashMap<String, (PathBuf, usize)> = HashMap::new();
    for c in &lowered.components {
        let key = (c.file.clone(), c.name.clone());
        component_meta.insert(
            temp_registry.display_name(&key),
            (c.file.clone(), c.hooks.len()),
        );
    }

    // An `--entry` name that matches nothing selects no root, which silently
    // collapses the run to intra-component analysis — every cross-component
    // finding lost to a typo, with an otherwise clean report. Fail instead.
    let unmatched = strategy.unmatched(&temp_registry);
    if !unmatched.is_empty() {
        for name in &unmatched {
            let _ = writeln!(err, "[error] --entry: no component named `{name}`");
        }
        let _ = writeln!(
            err,
            "[error] name a component defined in the analysed files, `Name@path` \
             to pick one of several with the same name"
        );
        return CheckOutput::usage(err);
    }
    drop(temp_registry);

    if opts.verbose {
        let symbol_graph = SymbolGraph::build(&lowered.components, &lowered.hooks);
        let topo = symbol_graph.topo_sort();
        let _ = writeln!(
            err,
            "[verbose] symbol graph: {} nodes, topo order = [{}]",
            topo.len(),
            topo.iter()
                .map(|n| format!("{}@{}", n.name, n.file.display()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if lowered.components.is_empty() {
        let report = CheckReport {
            components: vec![],
            files_analyzed: lowered.file_count,
            parse_errors: lowered.parse_errors,
            errors: 0,
            warnings: 0,
            infos: 0,
            exit_code: EXIT_OK,
            file_table: lowered.file_table,
            blind_spots: blind,
        };
        return CheckOutput {
            stdout: render(&report, opts, display),
            stderr: err,
            exit_code: report.exit_code,
        };
    }

    // ── Analyze ───────────────────────────────────────────────────────────────
    let file_count = lowered.file_count;
    let parse_errors = std::mem::take(&mut lowered.parse_errors);
    // Ship with the common library-hook summaries (TanStack Query, React Router)
    // enabled: they resolve these hooks to ⊤ as *known* hooks, so a real
    // `useQuery`/`useNavigate` no longer emits `analysis-limit/unknown-hook`
    // noise. `Config::default()` stays empty so unit tests keep their baselines.
    let config = Config {
        summary_registry: crate::registry::SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    let program_result = analyze_lowered(lowered, strategy, config);

    if opts.verbose {
        let _ = writeln!(
            err,
            "[verbose] {} components analyzed",
            program_result.stats.components_analyzed
        );
        let _ = writeln!(
            err,
            "[verbose] cache hits: {}, misses: {}",
            program_result.stats.cache_hits, program_result.stats.cache_misses
        );
    }

    // ── Rules + filtering ─────────────────────────────────────────────────────
    let mut names: Vec<&String> = program_result.components.keys().collect();
    names.sort();

    let mut components = Vec::new();
    let (mut errors, mut warnings, mut infos) = (0usize, 0usize, 0usize);

    // One cache for the whole run: rules needing whole-program structure (the
    // churn graph of `infinite-loop`) build it once here instead of once per
    // component, which used to make the rules phase quadratic (issue #86).
    let rule_cache = ProgramCache::new(&program_result);

    for name in names {
        if opts.verbose {
            let result = &program_result.components[name];
            let mut labels: Vec<_> = result.widen_trace.keys().copied().collect();
            labels.sort_unstable();
            let _ = writeln!(
                err,
                "  [verbose] {name}: {} iteration(s), widened: {labels:?}",
                result.iterations
            );
        }

        // The whole rule pass (check + safe_check fallback + severity clamp +
        // off/allow filters + deterministic sort) lives in the registry —
        // shared with every other frontend. Only the `--info` visibility
        // filter is a display concern kept here.
        let findings = registry.check_component(&rule_cache, name);
        let mut diags: Vec<Diagnostic> = findings.diagnostics;
        let safe_checks: Vec<SafeCheck> = findings.safe_checks;
        let suspended_safe_checks = findings.suspended_safe_checks;
        diags.retain(|d| d.severity() != Severity::Info || opts.info);

        errors += diags
            .iter()
            .filter(|d| d.severity() == Severity::Error)
            .count();
        warnings += diags
            .iter()
            .filter(|d| d.severity() == Severity::Warning)
            .count();
        infos += diags
            .iter()
            .filter(|d| d.severity() == Severity::Info)
            .count();

        let (file, hook_count) = component_meta
            .get(name)
            .map(|(f, h)| (Some(f.clone()), *h))
            .unwrap_or((None, 0));

        components.push(ComponentReport {
            name: name.clone(),
            file,
            hook_count,
            diagnostics: diags,
            safe_checks,
            suspended_safe_checks,
        });
    }

    let exit_code = match opts.fail_on {
        FailOn::Error if errors > 0 => EXIT_FINDINGS,
        FailOn::Warning if errors + warnings > 0 => EXIT_FINDINGS,
        _ => EXIT_OK,
    };

    let report = CheckReport {
        components,
        files_analyzed: file_count,
        parse_errors,
        errors,
        warnings,
        infos,
        exit_code,
        file_table: program_result.file_table,
        blind_spots: blind,
    };
    CheckOutput {
        stdout: render(&report, opts, display),
        stderr: err,
        exit_code: report.exit_code,
    }
}

fn render(report: &CheckReport, opts: &CheckOptions, display: &dyn Fn(&Path) -> String) -> String {
    match opts.format {
        ReportFormat::Human => human::render(
            report,
            opts.color,
            opts.show_clean,
            opts.info,
            opts.trace,
            display,
        ),
        ReportFormat::Json => json::render(report, display),
    }
}

/// The `rules` listing: every diagnostic name with its summary.
pub fn run_rules_list(registry: &RuleRegistry, color: bool) -> String {
    let p = Palette::pick(color);
    let mut out = String::new();
    let width = registry.docs().map(|d| d.name.len()).max().unwrap_or(0);
    for doc in registry.docs() {
        let _ = writeln!(
            out,
            "  {}{:width$}{}  {}",
            p.bold, doc.name, p.reset, doc.summary
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Run `reactant explain <rule>` for details, example, and fix."
    );
    out
}

/// The `explain <rule>` page, or a did-you-mean usage error.
pub fn run_explain(registry: &RuleRegistry, rule: &str, color: bool) -> CheckOutput {
    let p = Palette::pick(color);
    match registry.doc(rule) {
        Some(doc) => {
            let mut out = String::new();
            let _ = writeln!(out, "{}{}{}", p.bold, doc.name, p.reset);
            let _ = writeln!(out, "  {}", doc.summary);
            let _ = writeln!(out);
            let _ = writeln!(out, "{}", doc.explanation);
            let _ = writeln!(out);
            if !doc.example.is_empty() {
                let _ = writeln!(out, "{}Example:{}", p.bold, p.reset);
                for line in doc.example.lines() {
                    let _ = writeln!(out, "  {line}");
                }
                let _ = writeln!(out);
            }
            let _ = writeln!(out, "{}Fix:{}", p.bold, p.reset);
            let _ = writeln!(out, "  {}", doc.fix);
            CheckOutput {
                stdout: out,
                stderr: String::new(),
                exit_code: EXIT_OK,
            }
        }
        None => {
            let mut err = String::new();
            let _ = writeln!(err, "[error] unknown rule `{rule}`");
            let suggestions: Vec<&str> = registry
                .docs()
                .map(|d| d.name.as_ref())
                .filter(|n: &&str| {
                    n.contains(rule)
                        || rule.contains(n)
                        || n.split('-').any(|part| rule.contains(part))
                })
                .collect();
            if !suggestions.is_empty() {
                let _ = writeln!(err, "did you mean: {}?", suggestions.join(", "));
            } else {
                let _ = writeln!(err, "run `reactant rules` for the list of valid names");
            }
            CheckOutput::usage(err)
        }
    }
}
