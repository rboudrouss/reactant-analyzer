//! A module `const` a program function made (`const channel =
//! createChannel()`, #422): when every return of the function is an object
//! literal and the program cannot have changed the object, its methods are
//! the literal's functions, so `channel.emit(x)` runs a program body — no
//! dispatch of any event — and a dispatch inside that body is seen where it
//! is. Anywhere else the method stays one of a module object the analysis
//! does not run.
use std::path::PathBuf;
use std::sync::Arc;

use reactant::engine::{Config, RootStrategy};
use reactant::ir::ModuleConstInit;
use reactant::resolver::{
    DefaultImportResolver, LoweredProgram, MemFileSystem, analyze_lowered, lower_files_with,
};
use reactant::rules::{RuleCtx, Severity, all_rules};

fn lower(files: &[(&str, &str)]) -> LoweredProgram {
    let mem =
        || MemFileSystem::from_map(files.iter().map(|(p, s)| (PathBuf::from(p), s.to_string())));
    let fs = mem();
    let resolver = DefaultImportResolver::new(Arc::new(mem()));
    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(p)).collect();
    lower_files_with(&fs, &paths, &resolver)
}

/// Is `name` a made module object in `file`'s table?
fn made(files: &[(&str, &str)], file: &str, name: &str) -> bool {
    let lowered = lower(files);
    matches!(
        lowered
            .module_consts
            .get(&PathBuf::from(file))
            .and_then(|m| m.get(name)),
        Some(ModuleConstInit::Made(_))
    )
}

/// The Warning or Error rows of the loop rules on `component`.
fn loop_claims(files: &[(&str, &str)], component: &str) -> Vec<String> {
    let r = analyze_lowered(lower(files), RootStrategy::Heuristic, Config::default());
    let ctx = RuleCtx::new(&r, r.component_named(component).unwrap());
    all_rules()
        .iter()
        .filter(|rule| {
            [
                "infinite-loop",
                "cross-component-infinite-loop",
                "setter-in-render",
                "cross-setter-in-render",
            ]
            .contains(&rule.name())
        })
        .flat_map(|rule| rule.check(&ctx))
        .filter(|d| d.severity() != Severity::Info)
        .map(|d| format!("{} {}", d.rule, d.message))
        .collect()
}

const CHANNEL: &str = r#"
export const createChannel = () => {
  const listeners = new Set<(v: string) => void>();
  return {
    emit: (v: string) => {
      listeners.forEach((l) => l(v));
    },
    subscribe: (l: (v: string) => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
};
export const channel = createChannel();
"#;

const APP: &str = r#"
import { useCallback, useEffect, useState } from "react";
import { channel } from "./channel";

function useShortcut(callback: () => void) {
  const onKeyDown = useCallback(() => callback(), [callback]);
  useEffect(() => {
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onKeyDown]);
}

export function App() {
  const [open, setOpen] = useState(false);
  const [saved, setSaved] = useState("");
  useShortcut(() => setOpen((o) => !o));
  useEffect(() => channel.subscribe((v) => setSaved(v)), []);
  useEffect(() => {
    channel.emit("x");
  }, []);
  return <p>{open ? "open" : saved}</p>;
}
"#;

fn program(channel: &str) -> Vec<(&'static str, String)> {
    vec![
        ("/p/channel.ts", channel.to_string()),
        ("/p/App.tsx", APP.to_string()),
    ]
}

fn files<'a>(p: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    p.iter().map(|(f, s)| (*f, s.as_str())).collect()
}

#[test]
fn a_factory_made_object_s_methods_are_its_literal_s_functions() {
    let p = program(CHANNEL);
    assert!(made(&files(&p), "/p/channel.ts", "channel"));
    assert!(made(&files(&p), "/p/App.tsx", "channel"), "its importer");
    // `emit` calls the closures `subscribe` received: no dispatch of any
    // event, so the keydown listener runs on a key press only.
    assert_eq!(loop_claims(&files(&p), "App"), Vec::<String>::new());
}

#[test]
fn a_dispatch_inside_a_method_is_seen_where_the_method_runs() {
    // W6's `n1n`: `emit` fires a keydown on `document`, from an effect that
    // runs after every commit: React loops.
    let channel = r#"
export const createChannel = () => ({
  emit: () => {
    document.dispatchEvent(new KeyboardEvent("keydown"));
  },
});
export const channel = createChannel();
"#;
    let app = APP
        .replace(
            "useEffect(() => {\n    channel.emit(\"x\");\n  }, []);",
            "useEffect(() => {\n    channel.emit();\n  });",
        )
        .replace(
            "useEffect(() => channel.subscribe((v) => setSaved(v)), []);",
            "",
        );
    assert_ne!(app, APP);
    let p = vec![("/p/channel.ts", channel.to_string()), ("/p/App.tsx", app)];
    assert!(made(&files(&p), "/p/channel.ts", "channel"));
    assert!(!loop_claims(&files(&p), "App").is_empty());
}

#[test]
fn an_object_the_program_may_change_stays_a_module_object() {
    let fail = |channel: String, why: &str| {
        let p = program(&channel);
        assert!(!made(&files(&p), "/p/channel.ts", "channel"), "{why}");
        assert!(!made(&files(&p), "/p/App.tsx", "channel"), "{why}");
        // The base's reading: `emit` fires any event.
        assert!(!loop_claims(&files(&p), "App").is_empty(), "{why}");
    };
    fail(
        format!("{CHANNEL}channel.emit = (v: string) => void v;\n"),
        "a member written",
    );
    fail(
        format!("{CHANNEL}export function keep(c: unknown) {{ return c; }}\nkeep(channel);\n"),
        "the object handed on",
    );
    fail(
        format!("{CHANNEL}export const s = channel.toString();\n"),
        "a call of no method of its",
    );
    fail(
        CHANNEL.replace(
            "  return {\n    emit",
            "  if (Math.random()) return other;\n  return {\n    emit",
        ),
        "a return that is no literal",
    );
    fail(
        CHANNEL.replace(
            "emit: (v: string) => {\n      listeners.forEach((l) => l(v));\n    },",
            "emit(v: string) {\n      listeners.forEach(() => (this.n = v));\n    },",
        ),
        "a method reading `this`",
    );
    fail(
        CHANNEL.replace(
            "const listeners = new Set<(v: string) => void>();",
            "const listeners = new Set<(v: string) => void>();\n  const run = () => 0;",
        ),
        "a function the factory makes beside its methods",
    );
    fail(
        CHANNEL
            .replace("listeners.add(l);", "listeners.add(l);\n      last = l;")
            .replace(
                "const listeners",
                "let last: unknown = null;\n  const listeners",
            ),
        "a method writing what it captures",
    );
    fail(
        CHANNEL.replace(
            "emit: (v: string) => {",
            "...base,\n    emit: (v: string) => {",
        ),
        "a spread",
    );
}

#[test]
fn an_importer_s_write_voids_the_object() {
    let app = format!("{APP}\nchannel.emit = () => undefined;\n");
    let p = vec![("/p/channel.ts", CHANNEL.to_string()), ("/p/App.tsx", app)];
    assert!(!made(&files(&p), "/p/channel.ts", "channel"));
}
