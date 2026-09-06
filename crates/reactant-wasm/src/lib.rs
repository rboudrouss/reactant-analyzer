//! WASM entry point for the reactant analyzer (ADR-022 §6).
//!
//! The JS host is pure transport: it reads argv, the raw config text, the
//! pack bytes (resolved via `require.resolve`) and a superset file map, and
//! writes back the returned streams and exit code. **The host is never a
//! trust boundary**: the config and every pack are re-parsed and re-validated
//! here, and discovery/project detection/tsconfig chains/alias resolution all
//! run inside the engine over the in-memory filesystem — the exact same
//! `driver::run_check` composition as the native CLI, so behavior cannot
//! fork.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use reactant::config::{self, CheckArgsPartial, FailOnConfig, FormatConfig, ProjectConfig};
use reactant::driver::{self, CheckOptions, EXIT_USAGE};
use reactant::resolver::MemFileSystem;
use reactant::rules::{RuleRegistry, declarative};

/// Discovery constants the host's superset walk needs — served by the core
/// at runtime so wrapper and engine cannot drift.
///
/// `prunedDirs` is deliberately *not* the engine's exclusion list: since
/// #137 that list depends on the tree's `.gitignore`, which only the engine
/// reads. The host loads everything else and the engine re-filters, so the
/// map stays a superset of what discovery walks.
#[wasm_bindgen(js_name = hostConstants)]
pub fn host_constants() -> String {
    serde_json::json!({
        "prunedDirs": reactant::resolver::HOST_PRUNED_DIRS,
        "sourceExtensions": reactant::resolver::SOURCE_EXTENSIONS,
        "configFileName": config::CONFIG_FILE_NAME,
    })
    .to_string()
}

/// The help page, rendered by the core so `npx reactant-analyzer help` and
/// `reactant help` print the same bytes. Standalone rather than a [`run`]
/// command: printing the command listing must not depend on a config file
/// that parses.
#[wasm_bindgen(js_name = helpPage)]
pub fn help_page(color: bool) -> String {
    driver::run_help(color)
}

/// Extract the `packs` list from raw config text — the one config field the
/// host needs *before* calling [`run`] (to resolve pack files). Parsed by
/// the same validator as the real run, so the host never interprets JSONC.
/// Returns `{"ok": [specs…]}` or `{"error": "…"}`.
#[wasm_bindgen(js_name = packSpecs)]
pub fn pack_specs(config_text: &str) -> String {
    match config::parse(config_text, Path::new(config::CONFIG_FILE_NAME)) {
        Ok(cfg) => serde_json::json!({ "ok": cfg.packs }).to_string(),
        Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
    }
}

/// Validate one pack's JSON without running an analysis — the authoring-time
/// half of `packs build` (ADR-023 §5): the JS→JSON codegen is host-side, but
/// what counts as a valid pack is decided by the same `load_pack` the check
/// run uses, so the codegen cannot bless a pack the core would reject.
/// Returns `{"ok": {"name": …, "rules": [ids…], "warnings": [msgs…]}}` or
/// `{"error": "…"}`.
#[wasm_bindgen(js_name = validatePack)]
pub fn validate_pack(pack_json: &str) -> String {
    match reactant::rules::declarative::load_pack(pack_json, &Default::default()) {
        Ok(load) => serde_json::json!({
            "ok": {
                "name": load.pack_name,
                "rules": load.rules.iter().map(|r| r.rule.name().to_string()).collect::<Vec<_>>(),
                "warnings": load.warnings.iter()
                    .map(|w| format!("{}: {}", w.rule, w.message))
                    .collect::<Vec<_>>(),
            }
        })
        .to_string(),
        Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    /// "check" | "rules" | "explain"
    command: String,
    #[serde(default)]
    explain_rule: Option<String>,
    /// Positional path arguments, replayed identically to a CLI invocation.
    #[serde(default)]
    paths: Vec<String>,
    /// Superset file map (path → text), cwd-relative POSIX paths.
    #[serde(default)]
    files: BTreeMap<String, String>,
    /// Raw `reactant.config.json` text, if the host found one.
    #[serde(default)]
    config: Option<String>,
    /// Resolved packs: bytes in, validation here.
    #[serde(default)]
    packs: Vec<PackInput>,
    options: Options,
}

#[derive(Deserialize)]
struct PackInput {
    /// The config spec that named it (for error messages).
    name: String,
    json: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Options {
    #[serde(default)]
    info: bool,
    #[serde(default)]
    show_clean: bool,
    #[serde(default)]
    trace: bool,
    #[serde(default)]
    verbose: bool,
    #[serde(default)]
    all_roots: bool,
    #[serde(default)]
    entry: Vec<String>,
    #[serde(default)]
    exclude_dir: Vec<String>,
    #[serde(default)]
    follow_imports: bool,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    fail_on: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    rule: Vec<String>,
    #[serde(default)]
    ignore_rule: Vec<String>,
    #[serde(default)]
    color: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Output {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn usage(stderr: String) -> Output {
    Output {
        exit_code: EXIT_USAGE,
        stdout: String::new(),
        stderr,
    }
}

#[wasm_bindgen]
pub fn run(input_json: &str) -> String {
    console_error_panic_hook::set_once();
    let out = run_inner(input_json);
    serde_json::to_string(&out).expect("output envelope serializes")
}

fn run_inner(input_json: &str) -> Output {
    let input: Input = match serde_json::from_str(input_json) {
        Ok(i) => i,
        Err(e) => return usage(format!("[error] invalid input envelope: {e}\n")),
    };

    // ── Config: re-parsed and validated here, never trusted from the host ────
    let cfg = match &input.config {
        Some(text) => match config::parse(text, Path::new(config::CONFIG_FILE_NAME)) {
            Ok(c) => c,
            Err(e) => return usage(format!("[error] {e}\n")),
        },
        None => config::ReactantConfig::default(),
    };

    // ── Registry: natives + re-validated packs (config order, §8) ────────────
    let mut registry = RuleRegistry::natives();
    let options_by_id: BTreeMap<String, serde_json::Map<String, serde_json::Value>> = cfg
        .rules
        .iter()
        .filter(|(_, s)| !s.options.is_empty())
        .map(|(k, s)| (k.clone(), s.options.clone()))
        .collect();
    let mut warnings = String::new();
    for pack in &input.packs {
        let load = match declarative::load_pack(&pack.json, &options_by_id) {
            Ok(l) => l,
            Err(e) => return usage(format!("[error] pack `{}`: {e}\n", pack.name)),
        };
        for w in &load.warnings {
            warnings.push_str(&format!("[warn] rule `{}`: {}\n", w.rule, w.message));
        }
        for rule in load.rules {
            if let Err(e) = registry.register(rule.rule, rule.doc) {
                return usage(format!("[error] pack `{}`: {e}\n", pack.name));
            }
        }
    }
    let overrides =
        config::resolve_overrides(&cfg, &input.options.rule, &input.options.ignore_rule);
    if let Err(e) = registry.set_overrides(overrides) {
        return usage(format!("[error] {e}\n"));
    }

    let color = input.options.color;
    match input.command.as_str() {
        "rules" => Output {
            exit_code: 0,
            stdout: driver::run_rules_list(&registry, color),
            stderr: warnings,
        },
        "explain" => {
            let Some(rule) = &input.explain_rule else {
                return usage("[error] explain: missing rule name\n".to_string());
            };
            let out = driver::run_explain(&registry, rule, color);
            Output {
                exit_code: out.exit_code,
                stdout: out.stdout,
                stderr: warnings + &out.stderr,
            }
        }
        "check" => {
            let opts = match check_options(&input.options, &cfg) {
                Ok(o) => o,
                Err(msg) => return usage(msg),
            };
            let fs = Arc::new(MemFileSystem::from_map(
                input
                    .files
                    .iter()
                    .map(|(p, s)| (PathBuf::from(p), s.clone())),
            ));
            let paths = if input.paths.is_empty() {
                vec![".".to_string()]
            } else {
                input.paths.clone()
            };
            // Identity display: the host already sends cwd-relative paths.
            let out = driver::run_check(fs, &paths, &registry, &opts, &|p: &Path| {
                p.display().to_string()
            });
            Output {
                exit_code: out.exit_code,
                stdout: out.stdout,
                stderr: warnings + &out.stderr,
            }
        }
        other => usage(format!("[error] unknown command `{other}`\n")),
    }
}

/// Map envelope options into the shared partial, merge the config (the same
/// precedence mechanism as the native CLI), and produce driver options.
fn check_options(o: &Options, cfg: &config::ReactantConfig) -> Result<CheckOptions, String> {
    let mut partial = CheckArgsPartial {
        info: o.info,
        show_clean: o.show_clean,
        trace: o.trace,
        verbose: o.verbose,
        all_roots: o.all_roots,
        entry: o.entry.clone(),
        exclude_dirs: o.exclude_dir.clone(),
        follow_imports: o.follow_imports,
        format: match o.format.as_deref() {
            None => None,
            Some("human") => Some(FormatConfig::Human),
            Some("json") => Some(FormatConfig::Json),
            Some(x) => return Err(format!("[error] unknown format `{x}`\n")),
        },
        fail_on: match o.fail_on.as_deref() {
            None => None,
            Some("error") => Some(FailOnConfig::Error),
            Some("warning") => Some(FailOnConfig::Warning),
            Some("never") => Some(FailOnConfig::Never),
            Some(x) => return Err(format!("[error] unknown fail-on `{x}`\n")),
        },
        project: match o.project.as_deref() {
            None => None,
            Some("auto") => Some(ProjectConfig::Auto),
            Some("vite") => Some(ProjectConfig::Vite),
            Some("next") => Some(ProjectConfig::Next),
            Some("plain") => Some(ProjectConfig::Plain),
            Some(x) => return Err(format!("[error] unknown project `{x}`\n")),
        },
    };
    partial.merge(cfg);
    Ok(CheckOptions {
        info: partial.info,
        show_clean: partial.show_clean,
        trace: partial.trace,
        verbose: partial.verbose,
        all_roots: partial.all_roots,
        entry: partial.entry,
        exclude_dirs: partial.exclude_dirs,
        follow_imports: partial.follow_imports,
        format: match partial.format.unwrap_or(FormatConfig::Human) {
            FormatConfig::Human => driver::ReportFormat::Human,
            FormatConfig::Json => driver::ReportFormat::Json,
        },
        fail_on: match partial.fail_on.unwrap_or(FailOnConfig::Warning) {
            FailOnConfig::Error => driver::FailOn::Error,
            FailOnConfig::Warning => driver::FailOn::Warning,
            FailOnConfig::Never => driver::FailOn::Never,
        },
        project: match partial.project.unwrap_or(ProjectConfig::Auto) {
            ProjectConfig::Auto => driver::ProjectOverride::Auto,
            ProjectConfig::Vite => driver::ProjectOverride::Vite,
            ProjectConfig::Next => driver::ProjectOverride::NextJs,
            ProjectConfig::Plain => driver::ProjectOverride::Plain,
        },
        color: o.color,
    })
}
