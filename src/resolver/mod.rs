//! File discovery and import resolution extension points.
//!
//! Default implementations cover the common case: recursive `*.ts*` discovery
//! plus relative imports with `.ts`/`.tsx`/index fallbacks.
//! See `docs/plugins.md` for an end-to-end plugin example.
//!
//! All filesystem access goes through the [`FileSystem`] seam (ADR-022 §6):
//! the defaults are `std::fs`-backed via [`OsFileSystem`]; the WASM build
//! runs the same code over a [`MemFileSystem`].

pub mod filesystem;
pub mod gitignore;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    engine::{
        ComponentRegistry, Config, FunctionRegistry, HookRegistry, ProgramAnalysisResult,
        RootStrategy, analyze_program,
    },
    ir::{
        FileTable, ModuleConstInit, ModuleTable, component::ComponentIR, function_ir::FunctionIR,
        hook_ir::HookIR,
    },
    lowering::{
        ResolvedImport, lower_custom_hooks_with_resolver, lower_program_with_resolver,
        scan_context_names, utility_lowerer::lower_utilities_with_resolver,
    },
};

pub use filesystem::{FileSystem, MemFileSystem, OsFileSystem};
pub use gitignore::GitignoreStack;

pub trait FileDiscoverer: Send + Sync {
    fn discover(&self, root: &Path) -> Vec<PathBuf>;
}

pub trait ImportResolver: Send + Sync {
    /// Resolve a relative specifier from `from` to an absolute path.
    /// Returns `None` for package imports (non-relative specifiers) or
    /// unresolvable paths.
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf>;
}

pub struct DefaultFileDiscoverer {
    fs: Arc<dyn FileSystem>,
    /// `--exclude-dir` / `excludeDirs`, matched by bare name at any depth.
    /// Non-empty *replaces* both fallbacks rather than adding to them: the
    /// setting is an answer to "what is not source here", and a list that
    /// silently kept the built-in names would make `dist` unwalkable.
    exclude_dirs: Vec<String>,
}

pub struct DefaultImportResolver {
    fs: Arc<dyn FileSystem>,
}

impl DefaultFileDiscoverer {
    pub fn new(fs: Arc<dyn FileSystem>) -> Self {
        DefaultFileDiscoverer {
            fs,
            exclude_dirs: Vec::new(),
        }
    }

    /// Replace the default exclusions with an explicit list of directory
    /// names. Empty means "no list configured" — the default policy stands.
    pub fn with_exclude_dirs(mut self, names: Vec<String>) -> Self {
        self.exclude_dirs = names;
        self
    }
}

impl Default for DefaultFileDiscoverer {
    fn default() -> Self {
        Self::new(Arc::new(OsFileSystem))
    }
}

impl DefaultImportResolver {
    pub fn new(fs: Arc<dyn FileSystem>) -> Self {
        DefaultImportResolver { fs }
    }
}

impl Default for DefaultImportResolver {
    fn default() -> Self {
        Self::new(Arc::new(OsFileSystem))
    }
}

// ── Resolver combinators ─────────────────────────────────────────────────────

/// Try each resolver in order; the first one to resolve the specifier wins.
///
/// The "wrap another resolver and fall back to it" shape was being hand-written
/// at every site that needed two schemes at once (`TsconfigPathsResolver` keeps
/// its own `fallback` field, and the plugin guide told you to build one). One
/// combinator instead, so a chain is a value rather than a new `impl`.
///
/// An empty chain resolves nothing — it is `None`, not a panic.
pub struct ChainResolver(Vec<Box<dyn ImportResolver>>);

impl ChainResolver {
    pub fn new(resolvers: Vec<Box<dyn ImportResolver>>) -> Self {
        ChainResolver(resolvers)
    }
}

impl ImportResolver for ChainResolver {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        self.0.iter().find_map(|r| r.resolve(from, specifier))
    }
}

/// Per-file resolution: route by the *importing* file's location.
///
/// A run used to have exactly one `ImportResolver`, which is wrong for a
/// monorepo — `packages/ui` and `apps/web` genuinely resolve the same
/// specifier to different files, and the only way to express that was to
/// hand-roll the dispatch inside a custom impl.
///
/// The scope whose root is the **longest** prefix of the importing file wins,
/// so a nested package overrides its parent; a file under no scope goes to
/// `fallback`. Same longest-prefix discipline as tsconfig `paths`.
pub struct ScopedResolver {
    scopes: Vec<(PathBuf, Box<dyn ImportResolver>)>,
    fallback: Box<dyn ImportResolver>,
}

impl ScopedResolver {
    pub fn new(fallback: Box<dyn ImportResolver>) -> Self {
        ScopedResolver {
            scopes: Vec::new(),
            fallback,
        }
    }

    /// Route imports written in files under `root` through `resolver`.
    pub fn scope(mut self, root: impl Into<PathBuf>, resolver: Box<dyn ImportResolver>) -> Self {
        self.scopes.push((root.into(), resolver));
        self
    }
}

impl ImportResolver for ScopedResolver {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        self.scopes
            .iter()
            .filter(|(root, _)| from.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, r)| r.as_ref())
            .unwrap_or(self.fallback.as_ref())
            .resolve(from, specifier)
    }
}

// ── Plugin-facing high-level entry points ────────────────────────────────────

/// A file the parser or the filesystem complained about.
///
/// The two cases are not equally serious, and the caller cannot tell them apart
/// from the message alone: a recovered syntax error is noise, while a dropped
/// file means every finding it held is a silent false negative — the direction
/// the project forbids. Hence `analyzed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub file: PathBuf,
    pub message: String,
    /// `true` when the parser recovered and the file was still lowered;
    /// `false` when the file was dropped from the run (read error, or a parser
    /// panic that leaves the program empty).
    pub analyzed: bool,
}

/// Source type for a path, by extension.
///
/// `.js` and `.jsx` are JSX-enabled: the React ecosystem routinely puts JSX in
/// `.js` (Babel, CRA, Docusaurus), and JSX in a non-JSX source type makes the
/// parser *panic*, which drops the whole file — three real excalidraw
/// components were lost that way. Module kind stays unambiguous so a `.js` that
/// is a CommonJS script and one that uses `import.meta` both parse.
///
/// TypeScript is the exception that cannot share the flag: `<T>expr` is a type
/// assertion in `.ts` and a JSX element in `.tsx`, so the two must stay split.
///
/// `pub`: this mapping decides whether a file is analysed at all, so no caller
/// gets to keep a private copy of it that can drift.
pub fn source_type_for(path: &Path) -> oxc_span::SourceType {
    use oxc_span::SourceType;
    match path.extension().and_then(|e| e.to_str()) {
        Some("tsx") => SourceType::tsx(),
        Some("ts") | Some("mts") | Some("cts") => SourceType::ts(),
        _ => SourceType::unambiguous().with_jsx(true),
    }
}

/// Output of the parse+lower phase over a set of files, before any analysis.
///
/// Produced by [`lower_files`]; consumed by [`analyze_lowered`]. Splitting the
/// pipeline here lets callers inspect the lowered IR (component names, files,
/// hook counts) before running the fixpoint — the CLI uses this to build its
/// display-name → file map.
pub struct LoweredProgram {
    pub components: Vec<ComponentIR>,
    pub hooks: Vec<HookIR>,
    pub utilities: Vec<FunctionIR>,
    /// Number of files successfully parsed and lowered.
    pub file_count: usize,
    /// Files that hit a read or parse error, with the first message. A read
    /// error or a parser panic means the file was skipped; a *recovered*
    /// syntax error means the file was still lowered from a partial AST, so
    /// this list is a report channel, not a skip list — [`ParseError::analyzed`]
    /// says which of the two happened.
    /// Not printed here — the caller decides how to report them.
    pub parse_errors: Vec<ParseError>,
    /// `FileId ↔ path` interning table shared by every span produced during
    /// this lowering (ADR-019). Moved into the analysis result by
    /// [`analyze_lowered`].
    pub file_table: FileTable,
    /// Per-file directive prologue and resolved import edges (ADR-026 §1).
    /// Moved into the analysis result by [`analyze_lowered`].
    pub module_table: ModuleTable,
    /// Utility import edges (ADR-027 §3): `(importing file, local name) →
    /// (defining file, exported name)` for every resolved import that points
    /// at a lowered utility. Consumed by
    /// [`crate::engine::FunctionRegistry::from_functions_and_imports`].
    pub utility_imports: Vec<((PathBuf, String), (PathBuf, String))>,
}

/// Parse and lower an explicit list of files with the given `ImportResolver`.
/// Reads through `std::fs`; the seam-aware form is [`lower_files_with`].
pub fn lower_files(files: &[PathBuf], resolver: &dyn ImportResolver) -> LoweredProgram {
    lower_files_with(&OsFileSystem, files, resolver)
}

/// Parse and lower an explicit list of files, reading sources through `fs`.
///
/// Read/parse failures don't abort the run: they are recorded in
/// [`LoweredProgram::parse_errors`]. The file is only skipped when nothing can
/// be lowered from it (read error, or a parser panic that leaves the program
/// empty); a syntax error the parser recovered from is reported and analysed.
pub fn lower_files_with(
    fs: &dyn FileSystem,
    files: &[PathBuf],
    resolver: &dyn ImportResolver,
) -> LoweredProgram {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser as OxcParser};

    let mut lowered = LoweredProgram {
        components: Vec::new(),
        hooks: Vec::new(),
        utilities: Vec::new(),
        file_count: 0,
        parse_errors: Vec::new(),
        file_table: FileTable::default(),
        module_table: ModuleTable::default(),
        utility_imports: Vec::new(),
    };
    // Per-file context bindings and resolved imports, for the cross-file pass
    // below: a context is only provable once every file has been scanned.
    let mut contexts: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let mut file_imports: Vec<(PathBuf, HashMap<String, ResolvedImport>)> = Vec::new();

    for path in files {
        let source = match fs.read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                lowered.parse_errors.push(ParseError {
                    file: path.clone(),
                    message: e,
                    analyzed: false,
                });
                continue;
            }
        };
        let alloc = Allocator::default();
        let ret = OxcParser::new(&alloc, &source, source_type_for(path))
            .with_options(ParseOptions::default())
            .parse();
        // `panicked` is oxc's only "this AST is unusable" signal (the program
        // is empty). The parser recovers from every other syntax error and
        // still returns a lowerable program, so skipping on a non-empty
        // diagnostic list would drop whole files from the analysis — a
        // forbidden false negative, and a growing one: oxc keeps moving TS
        // semantic checks into the parser.
        if ret.panicked || !ret.diagnostics.is_empty() {
            lowered.parse_errors.push(ParseError {
                file: path.clone(),
                message: ret
                    .diagnostics
                    .first()
                    .map(|d| d.message.to_string())
                    .unwrap_or_else(|| "the parser produced no usable program".to_string()),
                analyzed: !ret.panicked,
            });
        }
        if ret.panicked {
            continue;
        }
        lowered.components.extend(lower_program_with_resolver(
            &ret.program,
            &source,
            path,
            &mut lowered.file_table,
            resolver,
        ));
        lowered.hooks.extend(lower_custom_hooks_with_resolver(
            &ret.program,
            &source,
            path,
            &mut lowered.file_table,
            resolver,
        ));
        lowered.utilities.extend(lower_utilities_with_resolver(
            &ret.program,
            &source,
            path,
            &mut lowered.file_table,
            resolver,
        ));
        lowered.module_table.insert(
            path.clone(),
            crate::lowering::collect_module_facts(&ret.program, path, resolver),
        );
        contexts.insert(path.clone(), scan_context_names(&ret.program, path));
        file_imports.push((
            path.clone(),
            crate::lowering::build_resolved_imports(&ret.program, path, resolver),
        ));
        lowered.file_count += 1;
    }

    resolve_imported_contexts(&mut lowered, &contexts, &file_imports);
    resolve_imported_utilities(&mut lowered, &file_imports);
    lowered
}

/// Record the import edges that point at lowered utilities (ADR-027 §3), so
/// `FunctionRegistry::resolve` can answer `(caller file, local name)` for an
/// imported — possibly aliased — utility. One level, like every other
/// cross-file resolution (#49).
fn resolve_imported_utilities(
    lowered: &mut LoweredProgram,
    file_imports: &[(PathBuf, HashMap<String, ResolvedImport>)],
) {
    let defined: HashSet<(&PathBuf, &String)> = lowered
        .utilities
        .iter()
        .map(|f| (&f.file, &f.name))
        .collect();
    for (file, imports) in file_imports {
        for (local, origin) in imports {
            if defined.contains(&(&origin.file, &origin.imported)) {
                lowered.utility_imports.push((
                    (file.clone(), local.clone()),
                    (origin.file.clone(), origin.imported.clone()),
                ));
            }
        }
    }
    // HashMap iteration order is seed-dependent; the registry map is
    // order-insensitive but the stored Vec should not be.
    lowered.utility_imports.sort();
}

/// Mark a context imported from another analyzed file as a context *here*.
///
/// `collect_module_consts` sees one file, so `import { Ctx } from "./ctx"` left
/// `Ctx` unproven and every `<Ctx.Provider>` unreachable to the provider
/// relation. Resolution has to happen once all files are lowered, which is
/// here; the enriched map replaces the per-file `Arc` on every component of
/// that file, so the role stays in the single table its consumers already read.
///
/// One level only: if the origin file re-exports the context from a third file,
/// the chain is not followed (the re-export limit, #49).
fn resolve_imported_contexts(
    lowered: &mut LoweredProgram,
    contexts: &HashMap<PathBuf, HashSet<String>>,
    file_imports: &[(PathBuf, HashMap<String, ResolvedImport>)],
) {
    // `local name → the origin cell it names`. The origin is exactly what the
    // match above already established, and dropping it was #109's defect.
    let mut extra: HashMap<PathBuf, Vec<(String, crate::ir::ContextId)>> = HashMap::new();
    for (file, imports) in file_imports {
        for (local, origin) in imports {
            if contexts
                .get(&origin.file)
                .is_some_and(|names| names.contains(&origin.imported))
            {
                extra.entry(file.clone()).or_default().push((
                    local.clone(),
                    crate::ir::ContextId {
                        origin_file: origin.file.clone(),
                        origin_name: origin.imported.clone(),
                    },
                ));
            }
        }
    }
    if extra.is_empty() {
        return;
    }
    // One rebuilt map per file, shared by that file's components as before.
    let mut rebuilt: HashMap<PathBuf, Arc<HashMap<String, ModuleConstInit>>> = HashMap::new();
    for comp in &mut lowered.components {
        let Some(names) = extra.get(&comp.file) else {
            continue;
        };
        let map = rebuilt.entry(comp.file.clone()).or_insert_with(|| {
            let mut m = (*comp.module_consts).clone();
            for (name, id) in names {
                m.insert(name.clone(), ModuleConstInit::Context(id.clone()));
            }
            Arc::new(m)
        });
        comp.module_consts = Arc::clone(map);
    }
}

/// Run the inter-component analysis on an already-lowered program.
///
/// `config` is consumed: `function_registry` is filled from
/// [`LoweredProgram::utilities`] (any previously set value is overwritten).
/// The caller's `widen_threshold`, `summary_registry`, and `max_inline_depth`
/// are preserved.
pub fn analyze_lowered(
    lowered: LoweredProgram,
    strategy: RootStrategy,
    mut config: Config,
) -> ProgramAnalysisResult {
    config.function_registry =
        FunctionRegistry::from_functions_and_imports(lowered.utilities, lowered.utility_imports);
    let registry = ComponentRegistry::from_components(lowered.components);
    let hook_registry = HookRegistry::from_hooks(lowered.hooks);
    let mut result = analyze_program(registry, hook_registry, strategy, &config);
    result.file_table = lowered.file_table;
    result.module_table = lowered.module_table;
    result
}

/// Parse, lower, and analyze an explicit list of files.
///
/// Parse errors are reported on stderr; the file is skipped only when the
/// parser could not recover (same contract as [`analyze_with_resolvers`]).
/// Returns the analysis result and the number of files actually analysed.
pub fn analyze_files(
    files: &[PathBuf],
    resolver: &dyn ImportResolver,
    strategy: RootStrategy,
    config: Config,
) -> (ProgramAnalysisResult, usize) {
    let lowered = lower_files(files, resolver);
    for e in &lowered.parse_errors {
        if e.analyzed {
            eprintln!("[parse error] {}: {}", e.file.display(), e.message);
        } else {
            eprintln!(
                "[skipped] {}: {} — the file was not analyzed",
                e.file.display(),
                e.message
            );
        }
    }
    let file_count = lowered.file_count;
    (analyze_lowered(lowered, strategy, config), file_count)
}

/// Run the full reactant pipeline (discover → parse → lower → analyse) with
/// caller-provided `FileDiscoverer` and `ImportResolver` implementations.
///
/// Use this when integrating reactant programmatically e.g. a Next.js or
/// monorepo plugin that needs custom discovery (`app/**/page.tsx` only) or
/// custom import resolution (`tsconfig` path aliases).
///
/// `config` is consumed: the function fills in `function_registry` from the
/// utilities it lowers during this run (any previously set value is
/// overwritten). The caller's `widen_threshold`, `summary_registry`, and
/// `max_inline_depth` are preserved.
///
/// Returns the analysis result and the number of files actually analysed
/// (parse errors are reported on stderr and the file is skipped).
pub fn analyze_with_resolvers(
    root: &Path,
    discoverer: &dyn FileDiscoverer,
    resolver: &dyn ImportResolver,
    strategy: RootStrategy,
    config: Config,
) -> (ProgramAnalysisResult, usize) {
    let files = discoverer.discover(root);
    analyze_files(&files, resolver, strategy, config)
}

/// Source extensions of the default discovery. `pub`: the WASM host's
/// superset walk reads them at runtime (`host_constants`), so wrapper and
/// core cannot drift (ADR-022 §6).
pub const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx"];

/// Never walked, under every policy: `node_modules` nests by design and holds
/// no project source, and `.git` holds no source at all. Not overridable —
/// `--exclude-dir` narrows what else is skipped, it does not widen the walk
/// into a package tree.
pub const ALWAYS_EXCLUDED_DIRS: &[&str] = &["node_modules", ".git"];

/// The fallback exclusions, matched by bare name at any depth — used only for
/// a tree no `.gitignore` governs and no `--exclude-dir` describes.
///
/// Matching a bare name at any depth is what made this a soundness bug
/// (#137): it also drops build *tooling source* (`scripts/build/`) and any
/// feature directory that happens to be called `dist`. A repository states
/// which of its directories are generated, in the file git reads, so
/// [`gitignore`] is consulted first and these names are only the answer when
/// there is nothing to read.
pub const EXCLUDED_DIRS: &[&str] = &["node_modules", "dist", "build", ".next"];

/// What the WASM host's superset walk may prune without ever hiding a file
/// the engine would have read: [`ALWAYS_EXCLUDED_DIRS`] plus `.next`, a build
/// cache every Next.js `.gitignore` declares and whose contents would cost
/// the host hundreds of megabytes to load for nothing. Everything else the
/// host loads and the engine re-filters (ADR-022 §6).
pub const HOST_PRUNED_DIRS: &[&str] = &["node_modules", ".git", ".next"];

fn is_source_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    // *.d.ts (declaration files)
    if name.ends_with(".d.ts") {
        return false;
    }

    // *.test.* / *.spec.*
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    if let Some((_, suffix)) = stem.rsplit_once('.')
        && (suffix == "test" || suffix == "spec")
    {
        return false;
    }

    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => SOURCE_EXTENSIONS.contains(&ext),
        None => false,
    }
}

/// Whether the walk descends into `path`, whose bare name is `name`.
///
/// One precedence order, three sources (#137): the list the user configured,
/// else the repository's own `.gitignore`s, else the built-in names — with
/// [`ALWAYS_EXCLUDED_DIRS`] short-circuiting all three.
fn skip_dir(name: &str, path: &Path, configured: &[String], ignores: &GitignoreStack) -> bool {
    if ALWAYS_EXCLUDED_DIRS.contains(&name) {
        return true;
    }
    if !configured.is_empty() {
        return configured.iter().any(|c| c == name);
    }
    if !ignores.is_empty() {
        return ignores.ignores_dir(path);
    }
    EXCLUDED_DIRS.contains(&name)
}

fn walk(
    fs: &dyn FileSystem,
    dir: &Path,
    configured: &[String],
    ignores: &mut GitignoreStack,
    out: &mut Vec<PathBuf>,
) {
    let pushed = ignores.push_for(fs, dir);
    for path in fs.read_dir(dir) {
        if fs.is_dir(&path) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if skip_dir(name, &path, configured, ignores) {
                continue;
            }
            walk(fs, &path, configured, ignores, out);
        } else if fs.is_file(&path) && is_source_file(&path) {
            out.push(path);
        }
    }
    if pushed {
        ignores.pop();
    }
}

impl FileDiscoverer for DefaultFileDiscoverer {
    fn discover(&self, root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if self.fs.is_file(root) {
            if is_source_file(root) {
                files.push(root.to_path_buf());
            }
            return files;
        }
        // The `.gitignore`s above `root` govern it too — `reactant check
        // src/features` is still inside the repository that wrote them.
        let mut ignores = GitignoreStack::seed(self.fs.as_ref(), root);
        walk(
            self.fs.as_ref(),
            root,
            &self.exclude_dirs,
            &mut ignores,
            &mut files,
        );
        files.sort();
        files
    }
}

/// Collapse `.` and `..` lexically, without touching the filesystem.
/// We don't use `fs::canonicalize` because it resolves symlinks (and on
/// Windows produces UNC paths), which is more than we need for registry keys.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                // Pop the last normal segment if any; otherwise keep `..`
                // (relative paths like `../foo` are valid).
                let popped = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if popped {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl ImportResolver for DefaultImportResolver {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        if !specifier.starts_with('.') {
            return None;
        }

        let parent = from.parent()?;
        let base = parent.join(specifier);

        // Try <base>.<ext> for each source extension.
        for ext in SOURCE_EXTENSIONS {
            let candidate = base.with_extension(ext);
            if self.fs.is_file(&candidate) {
                return Some(normalize(&candidate));
            }
        }

        // Try <base>/index.<ext>.
        for ext in SOURCE_EXTENSIONS {
            let candidate = base.join(format!("index.{ext}"));
            if self.fs.is_file(&candidate) {
                return Some(normalize(&candidate));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Lightweight scratch directory under the system temp dir.
    /// Cleans up on drop; unique per test via a process-local counter.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(label: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "reactant-resolver-{}-{}-{}",
                std::process::id(),
                label,
                id,
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create tmp dir");
            Tmp(path)
        }

        fn write(&self, rel: &str, body: &str) -> PathBuf {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parents");
            }
            fs::write(&path, body).expect("write file");
            path
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn rel(root: &Path, files: &[PathBuf]) -> Vec<String> {
        let mut names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn discover_finds_tsx_files() {
        let tmp = Tmp::new("finds-tsx");
        tmp.write("Page.tsx", "");
        tmp.write("helper.ts", "");
        tmp.write("button.jsx", "");
        tmp.write("legacy.js", "");
        tmp.write("README.md", "");

        let files = DefaultFileDiscoverer::default().discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(
            names,
            vec!["Page.tsx", "button.jsx", "helper.ts", "legacy.js"]
        );
    }

    #[test]
    fn discover_recurses_subdirectories() {
        let tmp = Tmp::new("recurse");
        tmp.write("app/page.tsx", "");
        tmp.write("app/components/Button.tsx", "");
        tmp.write("lib/utils/format.ts", "");

        let files = DefaultFileDiscoverer::default().discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(
            names,
            vec![
                "app/components/Button.tsx",
                "app/page.tsx",
                "lib/utils/format.ts",
            ]
        );
    }

    #[test]
    fn discover_excludes_node_modules() {
        let tmp = Tmp::new("node-modules");
        tmp.write("Page.tsx", "");
        tmp.write("node_modules/react/index.tsx", "");
        tmp.write("node_modules/nested/lib/foo.ts", "");

        let files = DefaultFileDiscoverer::default().discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(names, vec!["Page.tsx"]);
    }

    #[test]
    fn discover_excludes_build_dirs() {
        let tmp = Tmp::new("build-dirs");
        tmp.write("src/Page.tsx", "");
        tmp.write("dist/Page.tsx", "");
        tmp.write("build/Page.tsx", "");
        tmp.write(".next/Page.tsx", "");

        let files = DefaultFileDiscoverer::default().discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(names, vec!["src/Page.tsx"]);
    }

    #[test]
    fn discover_excludes_test_and_declaration_files() {
        let tmp = Tmp::new("tests");
        tmp.write("Page.tsx", "");
        tmp.write("Page.test.tsx", "");
        tmp.write("Page.spec.ts", "");
        tmp.write("types.d.ts", "");

        let files = DefaultFileDiscoverer::default().discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(names, vec!["Page.tsx"]);
    }

    #[test]
    fn discover_accepts_single_file() {
        let tmp = Tmp::new("single-file");
        let file = tmp.write("Page.tsx", "");

        let files = DefaultFileDiscoverer::default().discover(&file);
        assert_eq!(files, vec![file]);
    }

    #[test]
    fn discover_returns_empty_for_unknown_root() {
        let tmp = Tmp::new("missing");
        let missing = tmp.path().join("does-not-exist");

        let files = DefaultFileDiscoverer::default().discover(&missing);
        assert!(files.is_empty());
    }

    #[test]
    fn resolve_relative_ts() {
        let tmp = Tmp::new("resolve-ts");
        let from = tmp.write("a/b.tsx", "");
        let utils = tmp.write("a/utils.ts", "");

        let resolved = DefaultImportResolver::default().resolve(&from, "./utils");
        assert_eq!(resolved, Some(utils));
    }

    #[test]
    fn resolve_prefers_ts_over_js() {
        let tmp = Tmp::new("resolve-precedence");
        let from = tmp.write("a/b.tsx", "");
        let ts = tmp.write("a/utils.ts", "");
        tmp.write("a/utils.js", "");

        let resolved = DefaultImportResolver::default().resolve(&from, "./utils");
        assert_eq!(resolved, Some(ts));
    }

    #[test]
    fn resolve_index_fallback() {
        let tmp = Tmp::new("resolve-index");
        let from = tmp.write("a/b.tsx", "");
        let index = tmp.write("a/utils/index.ts", "");

        let resolved = DefaultImportResolver::default().resolve(&from, "./utils");
        assert_eq!(resolved, Some(index));
    }

    #[test]
    fn resolve_parent_directory() {
        let tmp = Tmp::new("resolve-parent");
        let from = tmp.write("a/b/c.tsx", "");
        let sibling = tmp.write("a/sibling.tsx", "");

        let resolved = DefaultImportResolver::default().resolve(&from, "../sibling");
        assert_eq!(resolved, Some(sibling));
    }

    #[test]
    fn resolve_package_returns_none() {
        let tmp = Tmp::new("resolve-package");
        let from = tmp.write("a/b.tsx", "");

        assert!(
            DefaultImportResolver::default()
                .resolve(&from, "@tanstack/react-query")
                .is_none()
        );
        assert!(
            DefaultImportResolver::default()
                .resolve(&from, "react")
                .is_none()
        );
    }

    /// A syntax error the parser recovered from must not cost us the file.
    /// oxc keeps moving TS semantic checks into the parser, so keying the
    /// skip on "any diagnostic" silently drops more and more real files —
    /// a forbidden false negative. Only `panicked` means "nothing to lower".
    #[test]
    fn recovered_syntax_error_still_lowers_the_file() {
        let tmp = Tmp::new("recovered-parse");
        let file = tmp.write(
            "App.tsx",
            r#"
import { useState, useEffect } from "react";

const z = a ?? b || c;

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => { setN(n + 1); });
  return <div>{n}</div>;
}
"#,
        );

        let lowered = lower_files(
            std::slice::from_ref(&file),
            &DefaultImportResolver::default(),
        );

        assert_eq!(lowered.file_count, 1, "the file must still be analysed");
        assert!(
            lowered.components.iter().any(|c| c.name == "App"),
            "the component must be lowered from the recovered AST"
        );
        assert_eq!(
            lowered.parse_errors.len(),
            1,
            "the diagnostic is still reported, it just no longer skips"
        );
        assert_eq!(lowered.parse_errors[0].file, file);
        assert!(
            lowered.parse_errors[0].analyzed,
            "a recovered error must not read as a dropped file"
        );
    }

    /// The other side of the contract: when the parser panics the program is
    /// empty, so the file is skipped and recorded.
    #[test]
    fn unrecoverable_parse_error_skips_the_file() {
        let tmp = Tmp::new("panicked-parse");
        let file = tmp.write(
            "Broken.tsx",
            "const = ;\nexport function App() { return <div />; }\n",
        );

        let lowered = lower_files(
            std::slice::from_ref(&file),
            &DefaultImportResolver::default(),
        );

        assert_eq!(lowered.file_count, 0);
        assert!(lowered.components.is_empty());
        assert_eq!(lowered.parse_errors.len(), 1);
        assert_eq!(lowered.parse_errors[0].file, file);
        assert!(
            !lowered.parse_errors[0].analyzed,
            "a dropped file must say so — its findings are missing, not absent"
        );
    }

    /// JSX in a `.js` file is the Babel/CRA/Docusaurus convention. Parsing
    /// `.js` as a CommonJS script made the parser panic on the first JSX
    /// element, dropping the whole file — three real excalidraw components were
    /// lost that way.
    #[test]
    fn jsx_in_a_dot_js_file_is_analyzed() {
        let tmp = Tmp::new("jsx-in-js");
        let file = tmp.write(
            "Loop.js",
            "import { useState } from 'react';\n\
             export function Loop() {\n\
               const [n, setN] = useState(0);\n\
               setN(n + 1);\n\
               return <div>{n}</div>;\n\
             }\n",
        );

        let lowered = lower_files(
            std::slice::from_ref(&file),
            &DefaultImportResolver::default(),
        );

        assert!(
            lowered.parse_errors.is_empty(),
            "JSX in `.js` must parse cleanly: {:?}",
            lowered.parse_errors
        );
        assert_eq!(lowered.file_count, 1);
        assert!(
            lowered.components.iter().any(|c| c.name == "Loop"),
            "the component must be lowered, got {:?}",
            lowered
                .components
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    /// `import.meta` is legal in a `.js` module and appears in real config
    /// files (three chakra-ui `eslint.config.js`). Script mode rejected it.
    #[test]
    fn import_meta_in_a_dot_js_file_parses() {
        let tmp = Tmp::new("import-meta-js");
        let file = tmp.write(
            "eslint.config.js",
            "const dir = import.meta.url;\nexport default [dir];\n",
        );

        let lowered = lower_files(
            std::slice::from_ref(&file),
            &DefaultImportResolver::default(),
        );

        assert!(
            lowered.parse_errors.is_empty(),
            "`import.meta` in `.js` must parse cleanly: {:?}",
            lowered.parse_errors
        );
    }

    /// The other half of `.js`: a CommonJS script must keep parsing. The module
    /// kind is unambiguous, so `require`/`module.exports` and `import.meta` are
    /// both fine in the same extension.
    #[test]
    fn commonjs_in_a_dot_js_file_still_parses() {
        let tmp = Tmp::new("cjs-in-js");
        let file = tmp.write(
            "util.js",
            "const path = require('path');\nmodule.exports = { path };\n",
        );

        let lowered = lower_files(
            std::slice::from_ref(&file),
            &DefaultImportResolver::default(),
        );

        assert!(
            lowered.parse_errors.is_empty(),
            "a CommonJS `.js` must parse cleanly: {:?}",
            lowered.parse_errors
        );
    }

    /// `.ts` is the extension that cannot share the JSX flag: `<T>expr` is a
    /// type assertion there, and a JSX element in `.tsx`.
    #[test]
    fn ts_keeps_angle_bracket_type_assertions() {
        let tmp = Tmp::new("ts-assertion");
        let file = tmp.write("cast.ts", "const n = <number>maybe;\nexport default n;\n");

        let lowered = lower_files(
            std::slice::from_ref(&file),
            &DefaultImportResolver::default(),
        );

        assert!(
            lowered.parse_errors.is_empty(),
            "`<number>x` must stay a type assertion in `.ts`: {:?}",
            lowered.parse_errors
        );
    }

    #[test]
    fn resolve_missing_returns_none() {
        let tmp = Tmp::new("resolve-missing");
        let from = tmp.write("a/b.tsx", "");

        assert!(
            DefaultImportResolver::default()
                .resolve(&from, "./nope")
                .is_none()
        );
    }

    // ── Combinators ───────────────────────────────────────────────────────────

    /// Answers `specifier` with a fixed path, and nothing else.
    struct Fixed(&'static str, PathBuf);

    impl ImportResolver for Fixed {
        fn resolve(&self, _from: &Path, specifier: &str) -> Option<PathBuf> {
            (specifier == self.0).then(|| self.1.clone())
        }
    }

    #[test]
    fn chain_takes_the_first_resolver_that_answers() {
        let chain = ChainResolver::new(vec![
            Box::new(Fixed("@ui", PathBuf::from("/first.tsx"))),
            Box::new(Fixed("@ui", PathBuf::from("/second.tsx"))),
            Box::new(Fixed("@app", PathBuf::from("/app.tsx"))),
        ]);
        let from = Path::new("/x/y.tsx");

        assert_eq!(
            chain.resolve(from, "@ui"),
            Some(PathBuf::from("/first.tsx")),
            "earlier resolvers win"
        );
        assert_eq!(
            chain.resolve(from, "@app"),
            Some(PathBuf::from("/app.tsx")),
            "later resolvers still get their turn"
        );
        assert_eq!(chain.resolve(from, "@nope"), None);
    }

    #[test]
    fn empty_chain_resolves_nothing() {
        assert!(
            ChainResolver::new(vec![])
                .resolve(Path::new("/x/y.tsx"), "@ui")
                .is_none()
        );
    }

    /// The point of #59: two files in the same run resolving one specifier to
    /// two different places, which a single per-run resolver cannot express.
    #[test]
    fn scoped_routes_by_the_importing_file() {
        let resolver = ScopedResolver::new(Box::new(Fixed("@x", PathBuf::from("/root.tsx"))))
            .scope(
                "/repo/packages/ui",
                Box::new(Fixed("@x", PathBuf::from("/ui.tsx"))),
            )
            .scope(
                "/repo/apps/web",
                Box::new(Fixed("@x", PathBuf::from("/web.tsx"))),
            );

        assert_eq!(
            resolver.resolve(Path::new("/repo/packages/ui/Button.tsx"), "@x"),
            Some(PathBuf::from("/ui.tsx"))
        );
        assert_eq!(
            resolver.resolve(Path::new("/repo/apps/web/Page.tsx"), "@x"),
            Some(PathBuf::from("/web.tsx"))
        );
        assert_eq!(
            resolver.resolve(Path::new("/elsewhere/Other.tsx"), "@x"),
            Some(PathBuf::from("/root.tsx")),
            "a file under no scope falls back"
        );
    }

    /// A nested scope overrides the one that contains it — longest prefix wins,
    /// the same rule tsconfig `paths` follows.
    #[test]
    fn the_innermost_scope_wins() {
        let resolver = ScopedResolver::new(Box::new(Fixed("@x", PathBuf::from("/root.tsx"))))
            .scope("/repo", Box::new(Fixed("@x", PathBuf::from("/outer.tsx"))))
            .scope(
                "/repo/packages/ui",
                Box::new(Fixed("@x", PathBuf::from("/inner.tsx"))),
            );

        assert_eq!(
            resolver.resolve(Path::new("/repo/packages/ui/Button.tsx"), "@x"),
            Some(PathBuf::from("/inner.tsx"))
        );
        assert_eq!(
            resolver.resolve(Path::new("/repo/other.tsx"), "@x"),
            Some(PathBuf::from("/outer.tsx"))
        );
    }
}
