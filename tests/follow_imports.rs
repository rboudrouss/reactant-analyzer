//! `--follow-imports`: analyse what the named paths import, report only the
//! named paths (#138).
//!
//! Discovery is normally the sole producer of lowered files, so on a narrowed
//! run the hook whose body decides whether the caller loops is resolved and
//! then never opened. The default stays that way on purpose — naming a
//! directory is a cheap way to look at one pattern, and following its imports
//! can approach the whole project — so this is opt-in, and the report scope
//! does not change when it is on.
//!
//! Drives the compiled binary: the flag is only observable end to end.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A hook returning a fresh object every render. A caller that puts it in a
/// dep array re-runs every render — but only a reader of *this* file can know
/// that, which is the whole point.
const HOOK: &str = r#"
import { useState } from "react";
export function useThing() {
  const [n, setN] = useState(0);
  return { n, setN, bag: { n } };
}
"#;

const CALLER: &str = r#"
import { useEffect } from "react";
import { useThing } from "../hooks/useThing";
export function Panel() {
  const { bag, setN } = useThing();
  useEffect(() => { setN(x => x + 1); }, [bag]);
  return <div>{bag.n}</div>;
}
"#;

/// A component defined in the imported file, holding a finding of its own.
const HOOK_WITH_COMPONENT: &str = r#"
import { useState, useEffect } from "react";
export function useThing() {
  const [n, setN] = useState(0);
  return { n, setN, bag: { n } };
}
export function Widget() {
  const [m, setM] = useState(0);
  useEffect(() => { setM(m + 1); });
  return <div>{m}</div>;
}
"#;

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-follow-{}-{label}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create tmp dir");
        Tmp(path).write("package.json", "{}")
    }

    fn write(self, rel: &str, body: &str) -> Self {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parents");
        fs::write(&path, body).expect("write file");
        self
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

struct Run(serde_json::Value);

impl Run {
    fn rules(&self) -> Vec<String> {
        let mut v: Vec<String> = self.0["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .filter_map(|d| d["rule"].as_str())
            .map(str::to_string)
            .collect();
        v.sort();
        v
    }

    fn blind_kinds(&self) -> Vec<String> {
        self.0["blind_spots"]
            .as_array()
            .expect("blind_spots")
            .iter()
            .filter_map(|b| b["kind"].as_str())
            .map(str::to_string)
            .collect()
    }

    fn followed(&self) -> Option<&serde_json::Value> {
        self.0.get("followed").filter(|v| !v.is_null())
    }
}

fn run(paths: &[&Path], extra: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(["--format", "json", "--fail-on", "never"])
        .args(extra)
        .args(paths)
        .env("NO_COLOR", "1")
        .output()
        .expect("run reactant binary");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    Run(serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}):\n{text}")))
}

// ── The default does not follow ───────────────────────────────────────────────

/// Without the flag the imported hook is opaque, and the run says so. This is
/// the control: the flag must be what changes the answer, not the fixture.
#[test]
fn the_default_leaves_the_import_unread_and_says_so() {
    let tmp = Tmp::new("default")
        .write("src/hooks/useThing.ts", HOOK)
        .write("src/features/Panel.tsx", CALLER);
    let r = run(&[&tmp.path().join("src/features")], &[]);
    assert_eq!(r.blind_kinds(), vec!["unread-imports"]);
    assert!(r.followed().is_none(), "{:?}", r.followed());
}

// ── The flag reads the import ─────────────────────────────────────────────────

/// The finding that only a reader of the hook can produce, reported *in the
/// named file*. `always-unstable-deps` needs to know `bag` is a fresh object,
/// which is a fact about `useThing`'s body.
#[test]
fn following_finds_what_only_the_imported_body_knows() {
    let tmp = Tmp::new("finds")
        .write("src/hooks/useThing.ts", HOOK)
        .write("src/features/Panel.tsx", CALLER);
    let named = tmp.path().join("src/features");
    assert!(
        !run(&[&named], &[])
            .rules()
            .contains(&"always-unstable-deps".into())
    );
    assert!(
        run(&[&named], &["--follow-imports"])
            .rules()
            .contains(&"always-unstable-deps".into())
    );
}

/// …and the blind spot it replaces is gone: the edge was followed, so nothing
/// resolved-but-unread is left to report.
#[test]
fn following_clears_the_unread_imports_blind_spot() {
    let tmp = Tmp::new("blind")
        .write("src/hooks/useThing.ts", HOOK)
        .write("src/features/Panel.tsx", CALLER);
    let r = run(&[&tmp.path().join("src/features")], &["--follow-imports"]);
    assert!(r.blind_kinds().is_empty(), "{:?}", r.blind_kinds());
}

/// The closure is reported, because a run that silently widened what it reads
/// would be the same trust failure the blind-spot list exists to prevent.
#[test]
fn the_followed_set_is_reported() {
    let tmp = Tmp::new("report")
        .write("src/hooks/useThing.ts", HOOK)
        .write("src/features/Panel.tsx", CALLER);
    let r = run(&[&tmp.path().join("src/features")], &["--follow-imports"]);
    let f = r.followed().expect("followed present");
    assert_eq!(f["files"], 1);
    assert!(
        f["examples"][0].as_str().unwrap().ends_with("useThing.ts"),
        "{f:?}"
    );
}

// ── The report still covers only what was named ───────────────────────────────

/// A component defined in a followed file is analysed but not reported — and
/// the count of what was left out is stated, with the path to add.
#[test]
fn findings_in_followed_files_are_counted_not_shown() {
    let tmp = Tmp::new("withheld")
        .write("src/hooks/useThing.tsx", HOOK_WITH_COMPONENT)
        .write("src/features/Panel.tsx", CALLER);
    let r = run(&[&tmp.path().join("src/features")], &["--follow-imports"]);
    assert!(
        !r.rules().contains(&"infinite-loop".into()),
        "Widget's finding leaked into the report: {:?}",
        r.rules()
    );
    let f = r.followed().expect("followed present");
    assert_eq!(f["withheld"], 1);
    assert!(
        f["withheld_examples"][0]
            .as_str()
            .unwrap()
            .ends_with("useThing.tsx"),
        "{f:?}"
    );
}

/// …and the advice the message gives is true: naming the path reports it.
/// Without this the withheld line could be pointing at nothing.
#[test]
fn naming_the_path_reports_the_withheld_finding() {
    let tmp = Tmp::new("widen")
        .write("src/hooks/useThing.tsx", HOOK_WITH_COMPONENT)
        .write("src/features/Panel.tsx", CALLER);
    let r = run(
        &[
            &tmp.path().join("src/features"),
            &tmp.path().join("src/hooks"),
        ],
        &[],
    );
    assert!(
        r.rules().contains(&"infinite-loop".into()),
        "{:?}",
        r.rules()
    );
}

/// A whole-project run already contains its own imports, so the flag reads
/// nothing extra and changes nothing. This is what keeps the flag from being
/// a second, subtly different analysis mode.
#[test]
fn on_a_whole_project_run_the_flag_is_a_no_op() {
    let tmp = Tmp::new("whole")
        .write("src/hooks/useThing.ts", HOOK)
        .write("src/features/Panel.tsx", CALLER);
    let plain = run(&[tmp.path()], &[]);
    let followed = run(&[tmp.path()], &["--follow-imports"]);
    assert_eq!(plain.rules(), followed.rules());
    assert_eq!(followed.followed().expect("present")["files"], 0);
    assert_eq!(followed.followed().expect("present")["withheld"], 0);
}
