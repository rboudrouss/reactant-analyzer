//! The build-tool marker is where the tsconfig search *starts*, not where it
//! stops (#139).
//!
//! Since `56ff872` the marker is found by walking upward from the given path.
//! The tsconfig was then loaded from the marker's directory and no further,
//! which loses the aliases of every monorepo that keeps `vite.config.*` in a
//! sub-app and the `paths` map at the root. excalidraw is exactly that shape:
//! `reactant test-repo/excalidraw/excalidraw-app` analysed 37 files, found 18
//! components and reported **nothing**, because every `@excalidraw/*` import
//! was opaque and the map that would resolve them was one `..` away.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use reactant::project::{ProjectKind, build_context};
use reactant::resolver::OsFileSystem;

mod tmp {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    pub struct Tmp(pub PathBuf);

    impl Tmp {
        pub fn new(label: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "reactant-tsconfig-{}-{label}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create tmp dir");
            Tmp(path)
        }

        pub fn write(self, rel: &str, body: &str) -> Self {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("create parents");
            fs::write(&path, body).expect("write file");
            self
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

use tmp::Tmp;

fn paths_json(pattern: &str, target: &str) -> String {
    format!(
        r#"{{ "compilerOptions": {{ "baseUrl": ".", "paths": {{ "{pattern}": ["{target}"] }} }} }}"#
    )
}

fn resolve(from: &Path, root: &Path, specifier: &str) -> Option<PathBuf> {
    let ctx = build_context(root, None, Arc::new(OsFileSystem));
    assert_eq!(ctx.kind, ProjectKind::Vite);
    ctx.resolver.resolve(from, specifier)
}

/// excalidraw's shape: the marker is in the sub-app, the `paths` map is at the
/// monorepo root, and nothing but an upward walk connects them.
#[test]
fn a_marker_in_a_sub_app_finds_the_paths_map_at_the_root() {
    let tmp = Tmp::new("subapp")
        .write("tsconfig.json", &paths_json("@lib/*", "./packages/lib/*"))
        .write("packages/lib/util.ts", "export const x = 1;")
        .write("app/vite.config.mts", "export default {}")
        .write("app/src/App.tsx", "");
    let app = tmp.path().join("app");
    assert_eq!(
        resolve(&app.join("src/App.tsx"), &app, "@lib/util"),
        Some(tmp.path().join("packages/lib/util.ts")),
    );
}

/// …and with the aliases loaded, nothing warns about them any more. The
/// `unresolved-aliases` blind spot (#9) is what excalidraw reported instead of
/// findings, so its absence is the visible half of this fix.
#[test]
fn the_sub_app_no_longer_reports_unresolved_aliases() {
    let tmp = Tmp::new("subapp-warn")
        .write("tsconfig.json", &paths_json("@lib/*", "./packages/lib/*"))
        .write("app/vite.config.mts", "export default {}")
        .write("app/src/App.tsx", "");
    let ctx = build_context(&tmp.path().join("app"), None, Arc::new(OsFileSystem));
    assert!(ctx.alias_warning.is_none(), "{:?}", ctx.alias_warning);
}

/// The nearest ancestor with `paths` wins: a monorepo root does not get to
/// override the package that declared its own aliases.
#[test]
fn the_nearest_ancestor_with_paths_wins() {
    let tmp = Tmp::new("nearest")
        .write("tsconfig.json", &paths_json("@lib/*", "./far/*"))
        .write("far/util.ts", "")
        .write("app/tsconfig.json", &paths_json("@lib/*", "./near/*"))
        .write("app/near/util.ts", "")
        .write("app/vite.config.ts", "export default {}")
        .write("app/src/App.tsx", "");
    let app = tmp.path().join("app");
    assert_eq!(
        resolve(&app.join("src/App.tsx"), &app, "@lib/util"),
        Some(app.join("near/util.ts")),
    );
}

/// A tsconfig declaring only `baseUrl` is a usable resolver but not an alias
/// map, so it is held back in favour of a further ancestor that has real
/// `paths` — the same discipline `load_tsconfig_paths` already applies to its
/// `references` hop.
#[test]
fn a_bare_base_url_does_not_stop_the_walk() {
    let tmp = Tmp::new("baseurl")
        .write("tsconfig.json", &paths_json("@lib/*", "./packages/lib/*"))
        .write("packages/lib/util.ts", "")
        .write(
            "app/tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": "." } }"#,
        )
        .write("app/vite.config.ts", "export default {}")
        .write("app/src/App.tsx", "");
    let app = tmp.path().join("app");
    assert_eq!(
        resolve(&app.join("src/App.tsx"), &app, "@lib/util"),
        Some(tmp.path().join("packages/lib/util.ts")),
    );
}

/// …but it is still the answer when no ancestor has anything better: a bare
/// `baseUrl` resolves non-relative specifiers, and losing it would be a
/// regression rather than a fix.
#[test]
fn a_bare_base_url_is_kept_when_no_ancestor_has_paths() {
    let tmp = Tmp::new("baseurl-only")
        .write(
            "app/tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": "." } }"#,
        )
        .write("app/lib/util.ts", "")
        .write("app/vite.config.ts", "export default {}")
        .write("app/src/App.tsx", "");
    let app = tmp.path().join("app");
    assert_eq!(
        resolve(&app.join("src/App.tsx"), &app, "lib/util"),
        Some(app.join("lib/util.ts")),
    );
}

/// The unaffected case: a project whose tsconfig sits at its marker resolves
/// exactly as before, with no ancestor consulted.
#[test]
fn a_tsconfig_at_the_marker_is_unaffected() {
    let tmp = Tmp::new("flat")
        .write("tsconfig.json", &paths_json("@/*", "./src/*"))
        .write("vite.config.ts", "export default {}")
        .write("src/hooks/useThing.ts", "")
        .write("src/App.tsx", "");
    assert_eq!(
        resolve(
            &tmp.path().join("src/App.tsx"),
            tmp.path(),
            "@/hooks/useThing"
        ),
        Some(tmp.path().join("src/hooks/useThing.ts")),
    );
}
