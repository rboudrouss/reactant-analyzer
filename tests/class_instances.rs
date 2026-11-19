//! A program class's instance is a placed object whose methods and
//! accessors are the class's bodies, `this` bound to it, and `new` runs its
//! constructor with its arguments (#335): a setter the constructor keeps is
//! a member of the instance, and a method the render calls calls it there.

use std::process::{Command, Output};

fn reactant(dir: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .arg(dir)
        .args([
            "--config",
            "tests/fixtures/all-rules.json",
            "--format",
            "json",
            "--fail-on",
            "never",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// The findings (`rule:severity`) on the oracle case `case`, its `// @file`
/// sections written as files of their own.
fn findings(case: &str) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reactant-class-{}-{case}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("oracle/cases/{case}.tsx")),
    )
    .unwrap();
    let mut files: Vec<(String, String)> = vec![("a.tsx".to_string(), String::new())];
    for line in src.lines() {
        match line.strip_prefix("// @file ") {
            Some(name) => files.push((name.trim().to_string(), String::new())),
            None => {
                let text = &mut files.last_mut().unwrap().1;
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    for (name, text) in &files {
        if !text.trim().is_empty() {
            std::fs::write(dir.join(name), text).unwrap();
        }
    }
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    let mut found: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"].as_str() != Some("info"))
        .map(|d| {
            format!(
                "{}:{}",
                d["rule"].as_str().unwrap(),
                d["severity"].as_str().unwrap()
            )
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

#[test]
fn a_method_of_a_kept_instance_calls_the_setter_its_constructor_kept() {
    for case in [
        "class-instance-memo-method-setter-loops",
        "class-instance-extends-program-class-loops",
        "class-instance-extends-unseen-class-loops",
        "class-instance-imported-method-loops",
        "class-field-arrow-this-setter-loops",
        "class-constructor-returns-object-loops",
        "class-getter-render-instance-loops",
    ] {
        assert_eq!(
            findings(case),
            ["setter-in-render:warning"],
            "{case}: the render calls the kept setter"
        );
    }
}

#[test]
fn a_method_that_calls_nothing_writes_nothing() {
    assert_eq!(
        findings("class-instance-memo-method-writes-nothing-converges"),
        Vec::<String>::new()
    );
}

#[test]
fn a_constructor_that_returns_a_kept_instance_makes_no_new_reference() {
    // #311: `new Service()` may be the instance a module binding keeps, so
    // `o` is no new reference at every render, and no loop is certain.
    let found = findings("class-singleton-constructor-dep-converges");
    assert!(
        !found
            .iter()
            .any(|f| f.ends_with(":error") || f.starts_with("always-unstable-deps")),
        "{found:?}"
    );
}
