//! Integration test for ADR-013 §1: two components sharing a name in
//! different files must coexist in the analyzer's output the Next.js
//! `app/<route>/page.tsx` pattern is no longer silently dropped.

use reactant::rules::RuleCtx;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use reactant::{
    engine::{ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program},
    lowering::{lower_custom_hooks, lower_program},
    resolver::{DefaultFileDiscoverer, FileDiscoverer},
    rules::{Severity, all_rules},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-page-{}-{}-{}",
            std::process::id(),
            label,
            id
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

fn parse_file(path: &Path) -> (Vec<reactant::ir::ComponentIR>, Vec<reactant::ir::HookIR>) {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser as OxcParser};
    use oxc_span::SourceType;

    let source = fs::read_to_string(path).expect("read source");
    let alloc = Allocator::default();
    let source_type = match path.extension().and_then(|e| e.to_str()) {
        Some("tsx") => SourceType::tsx(),
        Some("ts") => SourceType::ts(),
        Some("jsx") => SourceType::jsx(),
        _ => SourceType::cjs(),
    };
    let ret = OxcParser::new(&alloc, &source, source_type)
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    let components = lower_program(&ret.program, &source, path, &mut Default::default());
    let hooks = lower_custom_hooks(&ret.program, &source, path, &mut Default::default());
    (components, hooks)
}

#[test]
fn two_pages_in_different_files_coexist_and_only_buggy_one_warns() {
    let tmp = Tmp::new("page-collision");
    tmp.write(
        "app/users/page.tsx",
        r#"
        function Page() {
            const [c, setC] = useState(0);
            useEffect(() => { setC(c + 1); }, [c]);
            return <div>{c}</div>;
        }
        "#,
    );
    tmp.write(
        "app/posts/page.tsx",
        r#"
        function Page() {
            return <div>posts</div>;
        }
        "#,
    );

    // Phase 1: discovery.
    let files = DefaultFileDiscoverer::default().discover(tmp.path());
    assert_eq!(
        files.len(),
        2,
        "expected 2 discovered files, got: {:?}",
        files
    );

    let mut all_components = Vec::new();
    let mut all_hooks = Vec::new();
    for path in &files {
        let (cs, hs) = parse_file(path);
        all_components.extend(cs);
        all_hooks.extend(hs);
    }

    // Registry must keep both Page components.
    let registry = ComponentRegistry::from_components(all_components);
    let pages = registry.find_all_by_name(&"Page".to_string());
    assert_eq!(
        pages.len(),
        2,
        "both Page components must coexist in the registry"
    );

    // Run analysis.
    let hook_registry = HookRegistry::from_hooks(all_hooks);
    let result = analyze_program(
        registry,
        hook_registry,
        RootStrategy::AllComponents,
        &Config::default(),
    );

    // Both should appear, with disambiguating display names on collision.
    let keys: Vec<&String> = result.components.keys().collect();
    let page_like: Vec<&&String> = keys.iter().filter(|k| k.starts_with("Page")).collect();
    assert_eq!(
        page_like.len(),
        2,
        "expected two Page-like results, got keys: {:?}",
        keys
    );

    // The buggy Page (users/page.tsx) should have an infinite-loop diagnostic;
    // the clean Page (posts/page.tsx) should not.
    let rules = all_rules();
    let mut errors_per_key: Vec<(String, usize)> = Vec::new();
    for key in &page_like {
        let warns = rules
            .iter()
            .flat_map(|r| r.check(&RuleCtx::new(&result, key)))
            .filter(|d| d.severity() == Severity::Warning || d.severity() == Severity::Error)
            .count();
        errors_per_key.push(((**key).clone(), warns));
    }
    errors_per_key.sort();
    let with_warns: Vec<&(String, usize)> = errors_per_key.iter().filter(|(_, n)| *n > 0).collect();
    assert_eq!(
        with_warns.len(),
        1,
        "exactly one Page should have warnings (the buggy one); got: {:?}",
        errors_per_key
    );
    let (buggy_key, _) = with_warns[0];
    assert!(
        buggy_key.contains("users"),
        "the warning Page should be in /users/, got: {}",
        buggy_key
    );
}
