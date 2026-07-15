//! Static documentation for every diagnostic name the analyzer can emit.
//!
//! Keyed by **diagnostic name** (`Diagnostic::rule`), not by `Rule::name()`:
//! some rules emit several diagnostic names (`InfiniteLoop` also emits
//! `cross-component-infinite-loop`, `SetterInRender` also emits
//! `cross-setter-in-render`). Used by the CLI's `rules` / `explain`
//! subcommands and to validate `--rule` / `--ignore-rule` filters.

/// Documentation entry for one diagnostic name.
pub struct RuleDoc {
    /// Matches `Diagnostic::rule` exactly.
    pub name: &'static str,
    /// One line, shown by `reactant rules`.
    pub summary: &'static str,
    /// What the rule detects and why it matters, shown by `reactant explain`.
    pub explanation: &'static str,
    /// Minimal buggy snippet.
    pub example: &'static str,
    /// How to fix it.
    pub fix: &'static str,
}

/// All diagnostic names, sorted alphabetically.
pub const RULE_DOCS: &[RuleDoc] = &[
    RuleDoc {
        name: "always-unstable-deps",
        summary: "a dep is a fresh reference every render — the deps array never matches",
        explanation: "React compares deps with `Object.is`. An object, array, or function \
                      literal allocated during render has a new identity every render, so a \
                      single such dep defeats the whole array: the hook re-runs every render \
                      no matter how stable the other deps are. For `useEffect` this means the \
                      effect always fires; for `useMemo`/`useCallback` the memoization is dead \
                      weight.",
        example: "useEffect(() => sync(opts), [{ mode }, id]);",
        fix: "Hoist the literal out of render, memoize it (`useMemo`), or depend on its \
              primitive parts (`[mode, id]`) instead of a fresh wrapper object.",
    },
    RuleDoc {
        name: "analysis-limit",
        summary: "the analyzer deliberately truncated analysis here (potential false negatives)",
        explanation: "Emitted (with --info) wherever soundness required giving up precision: \
                      recursive components (recursion-cutoff), children not found in the \
                      registry (unknown-component), callback inlining past the depth cap \
                      (callback-depth-cap), or custom hooks with no source and no summary \
                      (unknown-hook). Each site is a place where a real bug could hide — a \
                      clean report does NOT cover these regions.",
        example: "const data = useVendorHook(); // hook body not in the analyzed files",
        fix: "Not a code bug. Include the hook's source in the analyzed paths, or register a \
              HookSummary for it via the plugin API.",
    },
    RuleDoc {
        name: "conditional-hook",
        summary: "hook called inside a conditional branch",
        explanation: "Hooks must be called unconditionally, in the same order, on every \
                      render — React matches hook calls to state slots by call order. A hook \
                      whose block does not dominate every render exit can be skipped on some \
                      path, shifting every later hook onto the wrong slot.",
        example: "if (visible) { const [n, setN] = useState(0); }",
        fix: "Call the hook unconditionally and branch on its result instead: \
              `const [n, setN] = useState(0); if (!visible) return null;`",
    },
    RuleDoc {
        name: "cross-component-infinite-loop",
        summary: "child effect sets parent state — parent re-renders child, effect refires",
        explanation: "An effect in a child component calls a setter received as a prop, and \
                      the abstract value of the parent's state slot keeps growing across \
                      fixpoint iterations. Parent re-renders → child re-renders → effect \
                      fires again: an infinite render loop spanning two components, invisible \
                      to any single-file linter.",
        example: "function Child({ onCount }) {\n  useEffect(() => onCount(c => c + 1));\n}",
        fix: "Gate the effect with a deps array that converges, or lift the update to an \
              event handler instead of an effect.",
    },
    RuleDoc {
        name: "cross-setter-in-render",
        summary: "parent's setter (received as prop) called during render",
        explanation: "Calling a setter passed down as a prop during the render body schedules \
                      a parent re-render while the child is rendering. React errors with \
                      \"Cannot update a component while rendering a different component\", or \
                      loops.",
        example: "function Child({ setTotal }) { setTotal(42); return <div/>; }",
        fix: "Move the call into a `useEffect` or an event handler.",
    },
    RuleDoc {
        name: "derived-state",
        summary: "effect only mirrors another state — should be computed during render",
        explanation: "A `useEffect` that unconditionally sets state B to a call-free function \
                      of state A stores derived data in state. It costs an extra render on \
                      every change of A (render with stale B, effect, render again) and can \
                      go stale.",
        example: "useEffect(() => setFull(first + ' ' + last), [first, last]);",
        fix: "Compute it during render — `const full = first + ' ' + last;` — or `useMemo` if \
              it's expensive. Delete the state slot.",
    },
    RuleDoc {
        name: "infinite-loop",
        summary: "effect sets state that re-triggers the effect — state diverges",
        explanation: "The render → effect → setState → render cycle never stabilizes: the \
                      abstract value of the state slot required widening and the effect's \
                      write stays unbounded, the structural signature of an infinite render \
                      loop. Detected through the fixpoint (guards that bound the state, e.g. \
                      `if (n < 5) setN(n+1)`, converge and are not flagged), including \
                      through `.then(...)` / `setTimeout(...)` callbacks and inlined \
                      cross-file custom hooks.",
        example: "useEffect(() => { setCount(count + 1); }, [count]);",
        fix: "Add a converging condition, use a deps array that doesn't include the state \
              being set, or move the update out of the effect.",
    },
    RuleDoc {
        name: "lazy-init",
        summary: "useState initializer calls a function on every render",
        explanation: "`useState(f())` evaluates `f()` on every render but uses the result \
                      only on mount — wasted work on each subsequent render. The lazy form \
                      `useState(() => f())` runs it once.",
        example: "const [data, setData] = useState(expensiveParse(blob));",
        fix: "const [data, setData] = useState(() => expensiveParse(blob));",
    },
    RuleDoc {
        name: "missing-deps",
        summary: "effect body captures a variable not listed in its deps array",
        explanation: "A `useEffect`/`useMemo`/`useCallback` body reads a non-stable free \
                      variable that is missing from the deps array. The closure keeps seeing \
                      the value from the render when deps last matched — the classic stale \
                      closure: the hook silently works on outdated data.",
        example: "useEffect(() => { fetch(url); }, []); // url captured, not declared",
        fix: "Add the captured variable to the deps array, or move it inside the effect if \
              it shouldn't retrigger it.",
    },
    RuleDoc {
        name: "redundant-set-state",
        summary: "setState called with the value the state already holds",
        explanation: "Both the argument and the current abstract state are stable and equal: \
                      the call can never change anything. Usually a leftover or a sign the \
                      update was meant to be conditional. (React bails out on identical \
                      values, but the render to discover it still costs.)",
        example: "const [n, setN] = useState(0); useEffect(() => { setN(0); }, []);",
        fix: "Delete the call, or make it set a genuinely different value.",
    },
    RuleDoc {
        name: "setter-in-render",
        summary: "setState called during the render body",
        explanation: "Calling a setter while rendering schedules another render before this \
                      one commits. Unconditional call → guaranteed infinite loop (Error); \
                      conditional call → loops whenever the condition holds (Warning).",
        example: "function C() { const [n, setN] = useState(0); setN(1); return <div/>; }",
        fix: "Move the call into a `useEffect` or an event handler. To derive a value, \
              compute it during render without state.",
    },
    RuleDoc {
        name: "unnecessary-rerender",
        summary: "mount-only effect immediately overwrites the initial state",
        explanation: "A `deps: []` effect sets a state slot to a stable constant different \
                      from its `useState` init. Every mount renders with the init value, then \
                      immediately re-renders with the effect's value — one wasted render and \
                      a visible flash, when the init could be the final value directly.",
        example: "const [mode, setMode] = useState('none');\nuseEffect(() => setMode('grid'), []);",
        fix: "Put the final value in the initializer: `useState('grid')` — or compute it \
              lazily: `useState(() => readSetting())`.",
    },
    RuleDoc {
        name: "widening-info",
        summary: "state slot required widening to converge (precision lost here)",
        explanation: "Informational (--info): the fixpoint only converged on this state slot \
                      by widening its abstract value (e.g. an interval jumped to +∞). \
                      Downstream checks on this slot are less precise. Often accompanies a \
                      real divergence (see infinite-loop) but is not itself a bug.",
        example: "useEffect(() => setN(n + 1), [n]); // n widens to [0, +∞)",
        fix: "Not a code bug per se — see the companion diagnostic on the same slot if any.",
    },
];

/// Look up the documentation for a diagnostic name.
pub fn rule_doc(name: &str) -> Option<&'static RuleDoc> {
    RULE_DOCS.iter().find(|d| d.name == name)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::all_rules;

    #[test]
    fn every_rule_name_has_a_doc() {
        for rule in all_rules() {
            assert!(
                rule_doc(rule.name()).is_some(),
                "rule `{}` has no RuleDoc entry",
                rule.name()
            );
        }
    }

    #[test]
    fn multi_name_rules_have_docs() {
        // Diagnostic names emitted under a different name than Rule::name().
        assert!(rule_doc("cross-component-infinite-loop").is_some());
        assert!(rule_doc("cross-setter-in-render").is_some());
    }

    #[test]
    fn docs_are_sorted_and_unique() {
        let names: Vec<&str> = RULE_DOCS.iter().map(|d| d.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "RULE_DOCS must be sorted and unique by name");
    }

    #[test]
    fn no_empty_fields() {
        for d in RULE_DOCS {
            assert!(!d.summary.is_empty(), "{}: empty summary", d.name);
            assert!(!d.explanation.is_empty(), "{}: empty explanation", d.name);
            assert!(!d.example.is_empty(), "{}: empty example", d.name);
            assert!(!d.fix.is_empty(), "{}: empty fix", d.name);
        }
    }
}
