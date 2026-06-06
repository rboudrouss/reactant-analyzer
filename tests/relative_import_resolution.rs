//! ADR-013 §2 integration test: `import { X } from './foo'` populates
//! `HookEntry::Custom::resolved_file` with the resolved absolute path so the
//! engine can do `(file, name)` lookups instead of name-only collisions.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use reactant::{
    ir::hooks::HookEntry,
    lowering::{compute_line_starts, lower_custom_hooks, lower_program},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-relimport-{}-{}-{}",
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
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn lower_file(path: &Path) -> (Vec<reactant::ir::ComponentIR>, Vec<reactant::ir::HookIR>) {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser as OxcParser};
    use oxc_span::SourceType;

    let source = fs::read_to_string(path).expect("read source");
    let alloc = Allocator::default();
    let ret = OxcParser::new(&alloc, &source, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(&source);
    (
        lower_program(&ret.program, &line_starts, path),
        lower_custom_hooks(&ret.program, &line_starts, path),
    )
}

#[test]
fn relative_hook_import_populates_resolved_file() {
    let tmp = Tmp::new("hook-import");
    let hooks_path = tmp.write(
        "hooks/useData.ts",
        r#"
        function useData() {
            const [v, setV] = useState(0);
            return v;
        }
        "#,
    );
    let page_path = tmp.write(
        "Page.tsx",
        r#"
        import { useData } from './hooks/useData';
        function Page() {
            const data = useData();
            return <div>{data}</div>;
        }
        "#,
    );

    let (components, _) = lower_file(&page_path);
    let page = components
        .iter()
        .find(|c| c.name == "Page")
        .expect("Page component lowered");

    let custom_use_data = page
        .hooks
        .iter()
        .find_map(|h| match h {
            HookEntry::Custom {
                name,
                resolved_file,
                ..
            } if name == "useData" => Some(resolved_file.clone()),
            _ => None,
        })
        .expect("Page should contain a Custom hook entry for useData");

    let resolved = custom_use_data.expect("resolved_file must be Some for ./hooks/useData");
    assert_eq!(
        resolved,
        std::path::PathBuf::from(
            hooks_path
                .canonicalize()
                .unwrap_or_else(|_| hooks_path.clone()),
        ),
        "resolved_file should point at the actual useData source"
    );
}

#[test]
fn package_import_leaves_resolved_file_none() {
    let tmp = Tmp::new("pkg-import");
    let page_path = tmp.write(
        "Page.tsx",
        r#"
        import { useQuery } from '@tanstack/react-query';
        function Page() {
            const q = useQuery();
            return <div>{q}</div>;
        }
        "#,
    );

    let (components, _) = lower_file(&page_path);
    let page = components
        .iter()
        .find(|c| c.name == "Page")
        .expect("Page lowered");

    let entry = page
        .hooks
        .iter()
        .find_map(|h| match h {
            HookEntry::Custom {
                name,
                resolved_file,
                import_source,
                ..
            } if name == "useQuery" => Some((resolved_file.clone(), import_source.clone())),
            _ => None,
        })
        .expect("Page should contain a Custom hook entry for useQuery");

    assert!(
        entry.0.is_none(),
        "package imports must not populate resolved_file"
    );
    assert_eq!(entry.1.as_deref(), Some("@tanstack/react-query"));
}

#[test]
fn unresolvable_relative_import_leaves_resolved_file_none() {
    let tmp = Tmp::new("missing-import");
    // No './missing' file written.
    let page_path = tmp.write(
        "Page.tsx",
        r#"
        import { useMissing } from './missing';
        function Page() {
            const v = useMissing();
            return <div>{v}</div>;
        }
        "#,
    );

    let (components, _) = lower_file(&page_path);
    let page = components.iter().find(|c| c.name == "Page").unwrap();
    let entry = page
        .hooks
        .iter()
        .find_map(|h| match h {
            HookEntry::Custom {
                name,
                resolved_file,
                ..
            } if name == "useMissing" => Some(resolved_file.clone()),
            _ => None,
        })
        .expect("useMissing entry present");
    assert!(
        entry.is_none(),
        "unresolvable specifier → resolved_file None"
    );
}
