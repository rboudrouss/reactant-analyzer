//! The `check` subcommand: discover → lower → analyze → report.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clap::Args;

use reactant::{
    engine::{ComponentRegistry, Config, RootStrategy, SymbolGraph},
    ir::FileTable,
    project::{self, ProjectKind},
    resolver::{DefaultFileDiscoverer, FileDiscoverer, analyze_lowered, lower_files},
    rules::{Diagnostic, SafeCheck, Severity, all_rules, rule_doc},
};

use super::{EXIT_FINDINGS, EXIT_OK, EXIT_USAGE, FailOn, OutputFormat, ProjectMode};

#[derive(Args, Default)]
pub struct CheckArgs {
    /// Files or directories to analyze (default: current directory).
    /// Directories are walked recursively for .ts/.tsx/.js/.jsx files.
    pub paths: Vec<String>,

    /// Show Info diagnostics (analysis limitations) plus, per shown component,
    /// the applicable checks that ran and passed ("verified: …")
    #[arg(long)]
    pub info: bool,

    /// Show components with no findings (hidden by default)
    #[arg(long)]
    pub show_clean: bool,

    /// Show each finding's causal chain (the `→` trace notes)
    #[arg(long)]
    pub trace: bool,

    /// Verbose debug output (symbol graph, fixpoint stats) on stderr
    #[arg(long)]
    pub verbose: bool,

    /// Analyze all components as entry points (props = ⊤)
    #[arg(long)]
    pub all_roots: bool,

    /// Entry point component names (repeatable or comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub entry: Vec<String>,

    /// Output format
    #[arg(long, value_enum, default_value = "human")]
    pub format: Option<OutputFormat>,

    /// Findings severity that makes the exit code non-zero
    #[arg(long, value_enum, default_value = "warning")]
    pub fail_on: Option<FailOn>,

    /// Project kind: auto-detect, force vite conventions, or plain walk
    #[arg(long, value_enum, default_value = "auto")]
    pub project: Option<ProjectMode>,

    /// Only report these diagnostics (repeatable; see `reactant rules`)
    #[arg(long)]
    pub rule: Vec<String>,

    /// Suppress these diagnostics (repeatable)
    #[arg(long)]
    pub ignore_rule: Vec<String>,

    /// Disable ANSI colors (also honored: NO_COLOR env, non-tty stdout)
    #[arg(long)]
    pub no_color: bool,
}

/// One component's report: display name, defining file, hook count, visible
/// diagnostics.
pub struct ComponentReport {
    pub name: String,
    pub file: Option<PathBuf>,
    pub hook_count: usize,
    pub diagnostics: Vec<Diagnostic>,
    /// Applicable checks that ran on this component and found nothing.
    /// Surfaced only under `--info`.
    pub safe_checks: Vec<SafeCheck>,
}

/// Everything the renderers need.
pub struct CheckReport {
    pub components: Vec<ComponentReport>,
    pub files_analyzed: usize,
    pub parse_errors: Vec<(PathBuf, String)>,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub exit_code: i32,
    /// Resolves the `FileId` carried by every diagnostic/note span (ADR-019),
    /// so renderers can name the file a cross-file trace step points into.
    pub file_table: FileTable,
}

pub fn run(mut args: CheckArgs) -> i32 {
    if args.paths.is_empty() {
        args.paths.push(".".to_string());
    }
    let format = args.format.unwrap_or(OutputFormat::Human);
    let fail_on = args.fail_on.unwrap_or(FailOn::Warning);

    // Validate rule filters before doing any work.
    for name in args.rule.iter().chain(args.ignore_rule.iter()) {
        if rule_doc(name).is_none() {
            eprintln!(
                "[error] unknown rule `{name}` — run `reactant rules` for the list of valid names"
            );
            return EXIT_USAGE;
        }
    }

    // ── Project context ───────────────────────────────────────────────────────
    // The first directory argument drives project detection; other paths are
    // discovered as-is with the same resolver.
    let project_root = args
        .paths
        .iter()
        .map(Path::new)
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let forced = match args.project.unwrap_or(ProjectMode::Auto) {
        ProjectMode::Auto => None,
        ProjectMode::Vite => Some(ProjectKind::Vite),
        ProjectMode::Plain => Some(ProjectKind::Plain),
    };
    if forced == Some(ProjectKind::Vite) && project::detect(&project_root) != ProjectKind::Vite {
        eprintln!(
            "[warn] --project vite: no vite.config.* found in {} — still trying tsconfig paths",
            project_root.display()
        );
    }
    let ctx = project::build_context(&project_root, forced);
    if let Some(warning) = &ctx.alias_warning {
        eprintln!("[warn] {warning}");
    }
    if args.verbose && ctx.kind == ProjectKind::Vite {
        eprintln!(
            "[verbose] vite project: discovery root {}, tsconfig aliases {}",
            ctx.discovery_root.display(),
            if ctx.alias_warning.is_none() {
                "loaded"
            } else {
                "unavailable"
            }
        );
    }

    // ── Discovery ─────────────────────────────────────────────────────────────
    let discoverer = DefaultFileDiscoverer;
    let mut files: Vec<PathBuf> = Vec::new();
    for input in &args.paths {
        let p = Path::new(input);
        if p.is_dir() {
            // The project-root dir may be narrowed (vite → <root>/src).
            let walk_root = if *p == *project_root {
                &ctx.discovery_root
            } else {
                p
            };
            let found = discoverer.discover(walk_root);
            if found.is_empty() {
                eprintln!("[error] no .ts/.tsx/.js/.jsx files found in {input}");
                return EXIT_USAGE;
            }
            files.extend(found);
        } else if p.is_file() {
            files.push(p.to_path_buf());
        } else {
            eprintln!("[error] no such file or directory: {input}");
            return EXIT_USAGE;
        }
    }

    // ── Lower ─────────────────────────────────────────────────────────────────
    let mut lowered = lower_files(&files, ctx.resolver.as_ref());
    if format == OutputFormat::Human {
        for (path, msg) in &lowered.parse_errors {
            eprintln!("[parse error] {}: {}", path.display(), msg);
        }
    }

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
    drop(temp_registry);

    if args.verbose {
        let symbol_graph = SymbolGraph::build(&lowered.components, &lowered.hooks);
        let topo = symbol_graph.topo_sort();
        eprintln!(
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
        };
        render(&report, &args, format);
        return report.exit_code;
    }

    // ── Analyze ───────────────────────────────────────────────────────────────
    let strategy = if !args.entry.is_empty() {
        RootStrategy::Explicit(args.entry.iter().map(|s| s.trim().to_string()).collect())
    } else if args.all_roots {
        RootStrategy::AllComponents
    } else {
        RootStrategy::Heuristic
    };

    let file_count = lowered.file_count;
    let parse_errors = std::mem::take(&mut lowered.parse_errors);
    // Ship with the common library-hook summaries (TanStack Query, React Router)
    // enabled: they resolve these hooks to ⊤ as *known* hooks, so a real
    // `useQuery`/`useNavigate` no longer emits `analysis-limit/unknown-hook`
    // noise. `Config::default()` stays empty so unit tests keep their baselines.
    let config = Config {
        summary_registry: reactant::registry::SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    let program_result = analyze_lowered(lowered, strategy, config);

    if args.verbose {
        eprintln!(
            "[verbose] {} components analyzed",
            program_result.stats.components_analyzed
        );
        eprintln!(
            "[verbose] cache hits: {}, misses: {}",
            program_result.stats.cache_hits, program_result.stats.cache_misses
        );
    }

    // ── Rules + filtering ─────────────────────────────────────────────────────
    let rules = all_rules();
    let mut names: Vec<&String> = program_result.components.keys().collect();
    names.sort();

    let mut components = Vec::new();
    let (mut errors, mut warnings, mut infos) = (0usize, 0usize, 0usize);

    for name in names {
        if args.verbose {
            let result = &program_result.components[name];
            let mut labels: Vec<_> = result.widen_trace.keys().copied().collect();
            labels.sort_unstable();
            eprintln!(
                "  [verbose] {name}: {} iteration(s), widened: {labels:?}",
                result.iterations
            );
        }

        // Per rule: collect its diagnostics; when it produced none, consult
        // `safe_check` — a rule reports "verified safe" only when it was
        // applicable to this component (see the `SafeCheck` doc).
        let mut diags: Vec<Diagnostic> = Vec::new();
        let mut safe_checks: Vec<SafeCheck> = Vec::new();
        for r in &rules {
            let produced = r.check(&program_result, name);
            if produced.is_empty()
                && let Some(sc) = r.safe_check(&program_result, name)
            {
                safe_checks.push(sc);
            }
            diags.extend(produced);
        }
        // Total order: rules iterate HashMaps internally, so same-key ties
        // (many `analysis-limit` Infos, several notes on one slot) come back
        // in a run-dependent order — tie-break on position, then content, so
        // consecutive runs are byte-identical (CI/bench diffing).
        diags.sort_by(|a, b| {
            let pos = |d: &Diagnostic| d.range.map_or((u32::MAX, u32::MAX), |r| (r.line, r.col));
            (
                a.rule,
                a.severity as u8,
                pos(a),
                &a.message,
                &a.var,
                a.hook_label,
            )
                .cmp(&(
                    b.rule,
                    b.severity as u8,
                    pos(b),
                    &b.message,
                    &b.var,
                    b.hook_label,
                ))
        });
        diags.retain(|d| {
            (args.rule.is_empty() || args.rule.iter().any(|r| r == d.rule))
                && !args.ignore_rule.iter().any(|r| r == d.rule)
                && (d.severity != Severity::Info || args.info)
        });
        // Same allowlist/ignore filtering as diagnostics; deterministic order.
        safe_checks.retain(|s| {
            (args.rule.is_empty() || args.rule.iter().any(|r| r == s.rule))
                && !args.ignore_rule.iter().any(|r| r == s.rule)
        });
        safe_checks.sort_by(|a, b| a.rule.cmp(b.rule));

        errors += diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        warnings += diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        infos += diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
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
        });
    }

    let exit_code = match fail_on {
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
    };
    render(&report, &args, format);
    report.exit_code
}

fn render(report: &CheckReport, args: &CheckArgs, format: OutputFormat) {
    match format {
        OutputFormat::Human => super::output_human::render(
            report,
            args.no_color,
            args.show_clean,
            args.info,
            args.trace,
        ),
        OutputFormat::Json => super::output_json::render(report),
    }
}
