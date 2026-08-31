//! Integration test for ADR-013 Phase 1.
//!
//! Verifies that passing a directory to the analyzer (via `DefaultFileDiscoverer`)
//! finds and analyzes multiple source files, with the flat-merge registries
//! still seeing each component/hook by its plain name.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use reactant::{
    engine::{ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program},
    lowering::{lower_custom_hooks, lower_program},
    resolver::{DefaultFileDiscoverer, FileDiscoverer},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-multifile-{}-{}-{}",
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

fn parse_file(path: &Path) -> (Vec<reactant::ir::ComponentIR>, Vec<reactant::ir::HookIR>) {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser as OxcParser};

    let source = fs::read_to_string(path).expect("read source");
    let alloc = Allocator::default();
    let source_type = reactant::resolver::source_type_for(path);
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
fn directory_input_discovers_and_analyzes_multiple_files() {
    let tmp = Tmp::new("page-plus-hook");

    // File 1 : a component that uses a custom hook.
    tmp.write(
        "Page.tsx",
        r#"
        function Page() {
            const data = useData();
            return <div>{data}</div>;
        }
        "#,
    );

    // File 2 : a custom hook in a subdirectory.
    tmp.write(
        "hooks/useData.ts",
        r#"
        function useData() {
            const [value, setValue] = useState(0);
            return value;
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

    // Parse + lower both files, flat-merge into a single registry pair.
    let mut all_components = Vec::new();
    let mut all_hook_irs = Vec::new();
    for path in &files {
        let (components, hooks) = parse_file(path);
        all_components.extend(components);
        all_hook_irs.extend(hooks);
    }

    assert!(
        all_components.iter().any(|c| c.name == "Page"),
        "expected component Page, got: {:?}",
        all_components.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    assert!(
        all_hook_irs.iter().any(|h| h.name == "useData"),
        "expected hook useData, got: {:?}",
        all_hook_irs.iter().map(|h| &h.name).collect::<Vec<_>>()
    );

    // Phase 2: registries + analysis (smoke check analysis must not panic
    // when the component's custom hook lives in a different source file).
    let reg = ComponentRegistry::from_components(all_components);
    let hook_reg = HookRegistry::from_hooks(all_hook_irs);
    let result = analyze_program(
        reg,
        hook_reg,
        RootStrategy::AllComponents,
        &Config::default(),
    );

    assert!(
        result.components.contains_key("Page"),
        "expected Page in analysis results, got: {:?}",
        result.components.keys().collect::<Vec<_>>()
    );
}

#[test]
fn directory_input_ignores_node_modules_and_test_files() {
    let tmp = Tmp::new("noise");
    tmp.write("Page.tsx", "function Page() { return <div/>; }");
    tmp.write("Page.test.tsx", "function PageTest() { return <div/>; }");
    tmp.write(
        "node_modules/lib/Other.tsx",
        "function Other() { return <div/>; }",
    );

    let files = DefaultFileDiscoverer::default().discover(tmp.path());
    assert_eq!(files.len(), 1, "expected only Page.tsx, got: {:?}", files);
    assert!(files[0].ends_with("Page.tsx"));
}
