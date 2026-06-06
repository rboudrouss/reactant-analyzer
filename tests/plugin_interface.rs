//! ADR-013 Phase 4 — plugin interface smoke tests for `analyze_with_resolvers`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use reactant::{
    engine::{Config, RootStrategy},
    resolver::{
        DefaultFileDiscoverer, DefaultImportResolver, FileDiscoverer, ImportResolver,
        analyze_with_resolvers,
    },
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-plugin-{}-{}-{}",
            std::process::id(),
            label,
            id
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create tmp dir");
        Tmp(path)
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parents");
        }
        fs::write(&path, body).expect("write file");
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

#[test]
fn default_discoverer_and_resolver_end_to_end() {
    let tmp = Tmp::new("default-pair");
    tmp.write(
        "App.tsx",
        r#"
        function App() {
            const [c, setC] = useState(0);
            return <div>{c}</div>;
        }
        "#,
    );

    let (result, file_count) = analyze_with_resolvers(
        tmp.path(),
        &DefaultFileDiscoverer,
        &DefaultImportResolver,
        RootStrategy::AllComponents,
        Config::default(),
    );

    assert_eq!(file_count, 1);
    assert!(result.components.contains_key("App"));
}

#[test]
fn custom_discoverer_can_filter_to_specific_files() {
    // Plugin behaviour: only pick up files named `page.tsx`, ignoring
    // `layout.tsx`. Verifies the trait is wired into the high-level entry.
    struct OnlyPages;
    impl FileDiscoverer for OnlyPages {
        fn discover(&self, root: &Path) -> Vec<PathBuf> {
            let mut out = Vec::new();
            walk(root, &mut out);
            out
        }
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("page.tsx") {
                out.push(p);
            }
        }
    }

    let tmp = Tmp::new("only-pages");
    tmp.write("app/users/page.tsx", "function Users() { return <div/>; }");
    tmp.write(
        "app/users/layout.tsx",
        "function Layout() { return <div/>; }",
    );

    let (result, file_count) = analyze_with_resolvers(
        tmp.path(),
        &OnlyPages,
        &DefaultImportResolver,
        RootStrategy::AllComponents,
        Config::default(),
    );

    assert_eq!(
        file_count, 1,
        "plugin should only see page.tsx, not layout.tsx"
    );
    assert!(result.components.contains_key("Users"));
    assert!(
        !result.components.contains_key("Layout"),
        "Layout should not be in results since layout.tsx was filtered out"
    );
}

#[test]
fn custom_resolver_is_invoked_for_relative_imports() {
    // Plugin behaviour: resolver that records every (from, specifier) it sees
    // so we can prove `analyze_with_resolvers` does call us back.
    use std::sync::Mutex;

    struct Recorder {
        calls: Mutex<Vec<(PathBuf, String)>>,
    }
    impl ImportResolver for Recorder {
        fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
            self.calls
                .lock()
                .unwrap()
                .push((from.to_path_buf(), specifier.to_string()));
            DefaultImportResolver.resolve(from, specifier)
        }
    }

    let tmp = Tmp::new("custom-resolver");
    tmp.write("hooks/useThing.ts", "function useThing() { return 0; }");
    tmp.write(
        "Page.tsx",
        r#"
        import { useThing } from './hooks/useThing';
        function Page() {
            const v = useThing();
            return <div>{v}</div>;
        }
        "#,
    );

    let recorder = Recorder {
        calls: Mutex::new(Vec::new()),
    };
    let (_result, _file_count) = analyze_with_resolvers(
        tmp.path(),
        &DefaultFileDiscoverer,
        &recorder,
        RootStrategy::AllComponents,
        Config::default(),
    );

    let calls = recorder.calls.lock().unwrap();
    assert!(
        calls.iter().any(|(_, spec)| spec == "./hooks/useThing"),
        "resolver should have been asked about the relative useThing import; got: {:?}",
        *calls
    );
}
