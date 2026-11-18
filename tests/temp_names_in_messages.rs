//! A message never prints a name lowering minted (#293).
//!
//! Lowering holds the value of `a ?? b`, `c ? x : y`, `a?.f()` or a switch's
//! discriminant in a temp named by its span, and records the span's text: a
//! message names the temp by that text (`this effect depends on
//! `obj?.get()``), as it names a call temp by its call (`get(…)`). Checked
//! over every fixture and every oracle case, so a printer that bypasses
//! [`reactant::ir::SourceNames`] fails here.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, text) of `paths`: the message, then each note.
fn texts(paths: &[&str]) -> Vec<(String, String, String)> {
    rows(paths)
        .into_iter()
        .map(|(component, rule, text, _)| (component, rule, text))
        .collect()
}

/// [`texts`], with the file each row is reported in.
fn rows(paths: &[&str]) -> Vec<(String, String, String, String)> {
    let mut args = paths.to_vec();
    args.extend([
        "--config",
        "tests/fixtures/all-rules.json",
        "--format",
        "json",
        "--fail-on",
        "never",
        "--all-roots",
        "--info",
    ]);
    let out = reactant(&args);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    let mut rows = vec![];
    for d in doc["diagnostics"].as_array().expect("diagnostics") {
        let s = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
        let (component, rule, file) = (s(&d["component"]), s(&d["rule"]), s(&d["file"]));
        let row = |text| (component.clone(), rule.clone(), text, file.clone());
        rows.push(row(s(&d["message"])));
        for n in d["notes"].as_array().into_iter().flatten() {
            rows.push(row(s(&n["message"])));
            rows.push(row(s(&n["desc"])));
        }
    }
    rows
}

/// The identifiers of `text` that are names lowering minted.
fn temps_in(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '#'))
        .filter(|w| reactant::ir::is_lowering_temp(w))
        .collect()
}

#[test]
fn the_shape_is_the_lowering_s() {
    for t in [
        "__t12_30",
        "__sw4_9",
        "__call_3_8",
        "__hook_0#2",
        "__p0",
        "__dflt_17",
    ] {
        assert!(reactant::ir::is_lowering_temp(t), "{t}");
    }
    for s in [
        "__DEV__",
        "__dirname",
        "t1",
        "__t",
        "__proto__",
        "__typename",
    ] {
        assert!(!reactant::ir::is_lowering_temp(s), "{s}");
    }
}

#[test]
fn no_message_of_any_fixture_or_case_names_a_temp() {
    let rows = texts(&["tests/fixtures", "oracle/cases"]);
    assert!(rows.len() > 1000, "{} texts", rows.len());
    for (component, rule, text) in rows {
        assert!(temps_in(&text).is_empty(), "{component} {rule}: {text}");
    }
}

#[test]
fn a_temp_reads_as_the_expression_it_holds() {
    let rows = texts(&["tests/fixtures/temp_names"]);
    for (component, rule, text) in &rows {
        assert!(temps_in(text).is_empty(), "{component} {rule}: {text}");
    }
    let says = |component: &str, rule: &str, part: &str| {
        assert!(
            rows.iter()
                .any(|(c, r, t)| c == component && r == rule && t.contains(part)),
            "{component} {rule}: no text says {part}: {rows:#?}"
        );
    };
    says(
        "OptionalCallDep",
        "infinite-loop",
        "depends on `obj?.get()`",
    );
    says("AndDep", "always-unstable-deps", "depends on `a && { b }`");
    says("NullishDep", "infinite-loop", "depends on `a ?? {}`");
    says("TernaryDep", "infinite-loop", "depends on `a ? [] : {}`");
    says("MemoDep", "always-unstable-deps", "depends on `a && {}`");
    says(
        "ChosenSetter",
        "setter-in-render",
        "setter `a ? setN : setM`",
    );
    says("HandedSetter", "setter-in-render", "setter `a || setN`");
    says("Child", "cross-setter-in-render", "prop `a ? onX : onX`");
    says(
        "MutatedInEffect",
        "state-mutation",
        "`s ?? {}` is mutated in place",
    );
    says(
        "ChosenReceiver",
        "missing-cleanup",
        "`(a ? window : document).addEventListener`",
    );
    says(
        "ChosenReceiver",
        "stale-closure",
        "`(a ?? window).setInterval`",
    );
}

// ── A setter is a setter (D2) ─────────────────────────────────────────────────
//
// A row keyed by its slot names the slot's setter; when the write went
// through a value that is not the setter — an object handed to a callee, a
// closure a child calls — the message says through what, and never calls
// that value a setter.

/// The names `text` calls a setter: `` setter `X` `` and `` `X` (a state
/// setter ``, `` `X`, a state setter ``, `` `X` is a state setter ``.
fn called_setters(text: &str) -> Vec<&str> {
    let mut out: Vec<&str> = text
        .match_indices("setter `")
        .filter_map(|(i, m)| text[i + m.len()..].split('`').next())
        .collect();
    for suffix in [
        "` (a state setter",
        "`, a state setter",
        "` is a state setter",
    ] {
        for (i, _) in text.match_indices(suffix) {
            if let Some(open) = text[..i].rfind('`') {
                out.push(&text[open + 1..i]);
            }
        }
    }
    out
}

/// The names `src` binds to a value that is no setter: a function, an
/// object or an array literal, a hook's result that is one of those.
fn bound_to_no_setter(src: &str) -> std::collections::HashSet<String> {
    let ident = |s: &str| -> Option<(String, usize)> {
        let n = s
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .unwrap_or(s.len());
        (n > 0).then(|| (s[..n].to_string(), n))
    };
    let mut out = std::collections::HashSet::new();
    for line in src.lines() {
        let line = line.trim_start().trim_start_matches("export ");
        if let Some(rest) = line.strip_prefix("function ")
            && let Some((name, _)) = ident(rest)
        {
            out.insert(name);
        }
        for kw in ["const ", "let ", "var "] {
            let Some(rest) = line.strip_prefix(kw) else {
                continue;
            };
            let Some((name, n)) = ident(rest) else {
                continue;
            };
            let Some(value) = rest[n..]
                .trim_start()
                .split_once('=')
                .filter(|(ty, _)| ty.is_empty() || ty.starts_with(':'))
                .map(|(_, v)| v.trim_start())
            else {
                continue;
            };
            let arrow = |v: &str| {
                ident(v).is_some_and(|(_, n)| v[n..].trim_start().starts_with("=>"))
                    || (v.starts_with('(') && line.contains("=>"))
            };
            if value.starts_with('{')
                || value.starts_with('[')
                || value.starts_with("function")
                || value.starts_with("async")
                || ["useCallback(", "useMemo(", "useRef("]
                    .iter()
                    .any(|h| value.starts_with(h))
                || arrow(value)
            {
                out.insert(name);
            }
        }
    }
    out
}

#[test]
fn no_message_calls_a_value_that_is_no_setter_a_setter() {
    let rows = rows(&["tests/fixtures", "oracle/cases"]);
    let mut sources: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    let mut checked = 0;
    for (component, rule, text, file) in &rows {
        if !rule.ends_with("setter-in-render") && rule != "cross-component-infinite-loop" {
            continue;
        }
        let no_setter = sources.entry(file.clone()).or_insert_with(|| {
            bound_to_no_setter(&std::fs::read_to_string(file).unwrap_or_default())
        });
        for name in called_setters(text) {
            checked += 1;
            assert!(
                !no_setter.contains(name),
                "{component} {rule} ({file}): `{name}` is no setter: {text}"
            );
        }
    }
    assert!(checked > 100, "{checked} names checked");
}

#[test]
fn a_row_names_the_slots_setter_and_what_it_went_through() {
    let rows = texts(&["tests/fixtures/setter_wording"]);
    let says = |component: &str, rule: &str, part: &str| {
        assert!(
            rows.iter()
                .any(|(c, r, t)| c == component && r == rule && t.contains(part)),
            "{component} {rule}: no text says {part}: {rows:#?}"
        );
    };
    says(
        "HandedObject",
        "setter-in-render",
        "setter `setSubmitting` is handed to a callee through `payload`,",
    );
    says(
        "HandedSetter",
        "setter-in-render",
        "setter `setN` is handed to a callee with",
    );
    says(
        "Child",
        "cross-setter-in-render",
        "setter `setN` of parent `ClosureProp` called during render of `Child` through `onX`",
    );
    says(
        "SetterChild",
        "cross-setter-in-render",
        "prop `onY` (a state setter of parent `SetterProp`)",
    );
    for (component, rule, text) in &rows {
        for name in called_setters(text) {
            assert!(
                !["payload", "onX", "bump"].contains(&name),
                "{component} {rule}: {text}"
            );
        }
    }
}
