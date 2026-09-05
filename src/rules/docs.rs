//! Static documentation for every diagnostic name the analyzer can emit.
//!
//! Keyed by **diagnostic name** (`Diagnostic::rule`), not by `Rule::name()`:
//! some rules emit several diagnostic names (`InfiniteLoop` also emits
//! `cross-component-infinite-loop`, `SetterInRender` also emits
//! `cross-setter-in-render`). Used by the CLI's `rules` / `explain`
//! subcommands and to validate `--rule` / `--ignore-rule` filters.

use std::borrow::Cow;

/// Documentation entry for one diagnostic name.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleDoc {
    /// Matches `Diagnostic::rule` exactly.
    pub name: Cow<'static, str>,
    /// One line, shown by `reactant rules`.
    pub summary: Cow<'static, str>,
    /// What the rule detects and why it matters, shown by `reactant explain`.
    pub explanation: Cow<'static, str>,
    /// Minimal buggy snippet.
    pub example: Cow<'static, str>,
    /// How to fix it.
    pub fix: Cow<'static, str>,
}

impl RuleDoc {
    /// Runtime constructor for dynamically loaded rules (ADR-022): pack docs
    /// own their strings. The static table below uses `doc`.
    pub fn new(
        name: impl Into<Cow<'static, str>>,
        summary: impl Into<Cow<'static, str>>,
        explanation: impl Into<Cow<'static, str>>,
        example: impl Into<Cow<'static, str>>,
        fix: impl Into<Cow<'static, str>>,
    ) -> Self {
        RuleDoc {
            name: name.into(),
            summary: summary.into(),
            explanation: explanation.into(),
            example: example.into(),
            fix: fix.into(),
        }
    }
}

/// Const constructor for the static table (fields in declaration order:
/// name, summary, explanation, example, fix).
const fn doc(
    name: &'static str,
    summary: &'static str,
    explanation: &'static str,
    example: &'static str,
    fix: &'static str,
) -> RuleDoc {
    RuleDoc {
        name: Cow::Borrowed(name),
        summary: Cow::Borrowed(summary),
        explanation: Cow::Borrowed(explanation),
        example: Cow::Borrowed(example),
        fix: Cow::Borrowed(fix),
    }
}

/// All diagnostic names, sorted alphabetically.
pub const RULE_DOCS: &[RuleDoc] = &[
    doc(
        "always-unstable-deps",
        "a dep is a fresh reference every render, so the deps array never matches",
        "React compares deps with `Object.is`. An object, array, or function \
                      literal allocated during render has a new identity every render, so a \
                      single such dep defeats the whole array: the hook re-runs every render \
                      no matter how stable the other deps are. For `useEffect` this means the \
                      effect always fires; for `useMemo`/`useCallback` the memoization is dead \
                      weight.",
        "useEffect(() => sync(opts), [{ mode }, id]);",
        "Hoist the literal out of render, memoize it (`useMemo`), or depend on its \
              primitive parts (`[mode, id]`) instead of a fresh wrapper object.",
    ),
    doc(
        "analysis-limit",
        "the analyzer deliberately truncated analysis here (potential false negatives)",
        "Emitted (with --info) wherever soundness required giving up precision: \
                      recursive components (recursion-cutoff), children not found in the \
                      registry (unknown-component), callback inlining past the depth cap \
                      (callback-depth-cap), or custom hooks with no source and no summary \
                      (unknown-hook). Each site is a place where a real bug could hide. A \
                      clean report does NOT cover these regions.",
        "const data = useVendorHook(); // hook body not in the analyzed files",
        "Not a code bug. Include the hook's source in the analyzed paths, or register a \
              HookSummary for it via the plugin API.",
    ),
    doc(
        "conditional-hook",
        "hook called inside a conditional branch",
        "Hooks must be called unconditionally, in the same order, on every \
                      render. React matches hook calls to state slots by call order. A hook \
                      whose block does not dominate every render exit can be skipped on some \
                      path, shifting every later hook onto the wrong slot.",
        "if (visible) { const [n, setN] = useState(0); }",
        "Call the hook unconditionally and branch on its result instead: \
              `const [n, setN] = useState(0); if (!visible) return null;`",
    ),
    doc(
        "cross-component-infinite-loop",
        "child effect sets parent state, parent re-renders child, effect refires",
        "An effect in a child component calls a setter received as a prop, and \
                      the abstract value of the parent's state slot keeps growing across \
                      fixpoint iterations. Parent re-renders → child re-renders → effect \
                      fires again: an infinite render loop spanning two components, invisible \
                      to any single-file linter.",
        "function Child({ onCount }) {\n  useEffect(() => onCount(c => c + 1));\n}",
        "Gate the effect with a deps array that converges, or lift the update to an \
              event handler instead of an effect.",
    ),
    doc(
        "cross-setter-in-render",
        "parent's setter (received as prop) called during render",
        "Calling a setter passed down as a prop during the render body schedules \
                      a parent re-render while the child is rendering. React errors with \
                      \"Cannot update a component while rendering a different component\", or \
                      loops. Error needs two proofs: the call dominates every exit, and \
                      calling the prop provably writes. A prop that only *carries* a \
                      captured setter into a handler warns instead.",
        "function Child({ setTotal }) { setTotal(42); return <div/>; }",
        "Move the call into a `useEffect` or an event handler.",
    ),
    doc(
        "derived-state",
        "effect only mirrors another state, so compute it during render",
        "A `useEffect` that unconditionally sets state B to a call-free function \
                      of state A stores derived data in state. It costs an extra render on \
                      every change of A (render with stale B, effect, render again) and can \
                      go stale.",
        "useEffect(() => setFull(first + ' ' + last), [first, last]);",
        "Compute it during render with `const full = first + ' ' + last;`, or `useMemo` if \
              it's expensive. Delete the state slot.",
    ),
    doc(
        "frozen-initial-state",
        "useState seeded from a prop that changes, so the state freezes at the first value",
        "`useState` reads its initializer on the first render only; every later \
                      render ignores it. When the initializer reads a prop and no effect keyed \
                      on that prop (and no render-time write) ever syncs the slot, the state \
                      stays frozen at the first prop value while the prop moves on. This is the \
                      classic \"my component doesn't update when props change\". Error when \
                      the prop is proven to be fed by another component's state that is \
                      actually written (cross-component analysis) and a mounted instance can \
                      observe the change; Warning when the prop's motion is uncertain, the \
                      setter escapes the component, or the call sites mount the component under \
                      a condition written by the feeder's own handlers; Info when \
                      intent is declared: every seeding prop is named for seed-once \
                      (`initial*`/`default*`), the slot is never written at all (a \
                      deliberate mount-time snapshot), or every call site re-seeds the \
                      component when the prop moves (`key={seed}`, or a render guarded by \
                      the seed). Silent when the feeding state provably never changes or a \
                      sync path exists (that quality is `derived-state`'s concern).",
        "function Row({ user }) {\n\
                  \x20 const [name, setName] = useState(user.name); // user changes later\n\
                  }",
        "Pick an ownership model: use the prop directly and lift updates up (controlled); \
              remount with `key={...}` at the call site to re-seed; or sync deliberately with \
              `useEffect(() => setName(user.name), [user.name])`. If seed-once is intended, \
              name the prop `initialName`.",
    ),
    doc(
        "infinite-loop",
        "effect sets state that re-triggers the effect, and the state diverges",
        "The render → effect → setState → render cycle never stabilizes: the \
                      abstract value of the state slot required widening and the effect's \
                      write stays unbounded, the structural signature of an infinite render \
                      loop. Detected through the fixpoint (guards that bound the state, e.g. \
                      `if (n < 5) setN(n+1)`, converge and are not flagged), including \
                      through `.then(...)` / `setTimeout(...)` callbacks and inlined \
                      cross-file custom hooks.",
        "useEffect(() => { setCount(count + 1); }, [count]);",
        "Add a converging condition, use a deps array that doesn't include the state \
              being set, or move the update out of the effect.",
    ),
    doc(
        "lazy-init",
        "useState initializer calls a function on every render",
        "`useState(f())` evaluates `f()` on every render but uses the result \
                      only on mount, so every later render wastes the work. The lazy form \
                      `useState(() => f())` runs it once. Severity is graded by what the call \
                      does: a state-setter call is an Error (state write every render); a \
                      side-effecting/async call (`fetch`, `subscribe`, `setTimeout`) is a \
                      Warning (the effect re-fires, not just wasted CPU); a proven-cheap pure \
                      builtin (`Math.*`, `Date.now`) is Info (advisory).",
        "const [data, setData] = useState(expensiveParse(blob));",
        "const [data, setData] = useState(() => expensiveParse(blob));",
    ),
    doc(
        "missing-cleanup",
        "effect starts something long-lived and returns no teardown",
        "An effect that calls `setInterval`, `addEventListener`, `subscribe`, \
                      `on` or `addListener` registers again every time it re-runs: on every \
                      deps change, on every mount, and twice on the first mount under \
                      StrictMode. With no cleanup nothing ever unregisters, so the handlers \
                      accumulate and the ones from earlier renders keep firing against state \
                      the component no longer has. One-shot registrars (`setTimeout`, `.then`) \
                      are out of scope: they fire late rather than repeatedly, which wants an \
                      abort flag, not a teardown. Warning only, because the teardown may live in a \
                      helper this rule cannot see through, so it fires solely when the effect \
                      provably returns nothing at all.",
        "useEffect(() => { window.addEventListener('resize', onResize); }, []);",
        "useEffect(() => {\n  window.addEventListener('resize', onResize);\n  \
                      return () => window.removeEventListener('resize', onResize);\n}, []);",
    ),
    doc(
        "missing-deps",
        "effect body captures a variable not listed in its deps array",
        "A `useEffect`/`useMemo`/`useCallback` body reads a non-stable free \
                      variable that is missing from the deps array. The closure keeps seeing \
                      the value from the render when deps last matched. That is the classic stale \
                      closure: the hook silently works on outdated data.",
        "useEffect(() => { fetch(url); }, []); // url captured, not declared",
        "Add the captured variable to the deps array, or move it inside the effect if \
              it shouldn't retrigger it.",
    ),
    doc(
        "redundant-set-state",
        "setState called with the value the state already holds",
        "Both the argument and the current abstract state are stable and equal: \
                      the call can never change anything. Usually a leftover or a sign the \
                      update was meant to be conditional. (React bails out on identical \
                      values, but the render to discover it still costs.)",
        "const [n, setN] = useState(0); useEffect(() => { setN(0); }, []);",
        "Delete the call, or make it set a genuinely different value.",
    ),
    doc(
        "server-component-hook",
        "a hook is called in a Next.js Server Component",
        "Under the Next.js App Router a module is a Server Component unless \
                      `\"use client\"` opens a client boundary above it. Server Components \
                      render once, on the server: there is no state, no commit phase and no \
                      hook for React to give them, so the render throws. The rule fires on a \
                      module reachable from an App Router entry (`page`, `layout`, \
                      `template`, `default`, `not-found`, `loading`) without crossing a \
                      `\"use client\"` directive. It stays silent in projects that never use \
                      the directive, where the whole notion does not apply.",
        "// app/page.tsx  (no \"use client\")\n\
                  export default function Page() { const [n, setN] = useState(0); }",
        "Add `\"use client\"` as the first line of the file, or keep the page a \
              Server Component and move the stateful part into a child component that \
              declares the directive.",
    ),
    doc(
        "setter-in-render",
        "setState called during the render body",
        "Calling a setter while rendering schedules another render before this \
                      one commits. Unconditional call → guaranteed infinite loop (Error); \
                      conditional call → loops whenever the condition holds (Warning). \
                      A third shape warns without claiming a call at all: the setter is \
                      handed to a callee with no timing summary (`handleSubmit(onSubmit)`), \
                      so the analyzer cannot rule out that it runs during render. A write a \
                      known registrar defers (`setTimeout(() => setN(1))`) is proven not to \
                      be a render write and is not reported here.",
        "function C() { const [n, setN] = useState(0); setN(1); return <div/>; }",
        "Move the call into a `useEffect` or an event handler. To derive a value, \
              compute it during render without state.",
    ),
    doc(
        "stale-closure",
        "long-lived callback keeps a state value frozen at registration time",
        "A callback handed to `setInterval`, `addEventListener`, `subscribe`, \
                      `setTimeout` or a promise `.then` inside an effect closes over the \
                      variable values from the render that last ran the effect. When the \
                      effect's deps array does not cover a captured state value, the callback \
                      keeps reading that old value after the state changes. With `[]` deps, \
                      forever. When the callback also writes the slot it reads \
                      (`setN(n + 1)` in an interval), the state can never advance past its \
                      first update: every firing recomputes from the same frozen capture \
                      (Error).",
        "const [n, setN] = useState(0);\n\
                  useEffect(() => { setInterval(() => setN(n + 1), 1000); }, []);",
        "Use the functional updater (`setN(n => n + 1)`) so the callback never reads \
              the captured value; mirror the latest value into a `useRef` and read \
              `ref.current`; or add the value to the deps array and return a cleanup so \
              the callback is re-registered with a fresh capture.",
    ),
    doc(
        "state-mutation",
        "state or prop object mutated in place, same reference, no re-render",
        "Mutating a state object (`arr.push(x)`, `obj.f = v`) keeps its reference \
                      identity; calling the setter with that same reference makes React bail \
                      out (`Object.is(old, new)` is true) and skip the re-render. The UI \
                      silently freezes (Error). Mutating an object received via props writes \
                      into data owned by the parent (Warning).",
        "const [items, setItems] = useState([]);\n\
                  const add = (x) => { items.push(x); setItems(items); };",
        "Create a new reference: `setItems([...items, x])`, or for objects \
              `setUser({ ...user, name })`. Never write through the current state value.",
    ),
    doc(
        "unnecessary-rerender",
        "mount-only effect immediately overwrites the initial state",
        "A `deps: []` effect sets a state slot to a stable constant different \
                      from its `useState` init. Every mount renders with the init value, then \
                      immediately re-renders with the effect's value. That is one wasted render and \
                      a visible flash, when the init could be the final value directly.",
        "const [mode, setMode] = useState('none');\nuseEffect(() => setMode('grid'), []);",
        "Put the final value in the initializer: `useState('grid')`, or compute it \
              lazily: `useState(() => readSetting())`.",
    ),
    doc(
        "unstable-context-value",
        "context provider hands consumers a new object every render",
        "A `<Ctx.Provider value={…}>` whose value is allocated in the \
                      provider's own render body is a different object on every render. \
                      `useContext` compares with `Object.is`, so every consumer re-renders \
                      whenever the providing component does, however deep the tree and \
                      however little actually changed. Fires only on a *proven* context \
                      (a module-level `createContext` reached through a React import) with \
                      a *proven* fresh value, and stays a Warning: the fresh reference is \
                      certain, what it costs is not.",
        "return <Ctx.Provider value={{ user, logout }}>{children}</Ctx.Provider>;",
        "Memoize the value: \
              `const v = useMemo(() => ({ user, logout }), [user, logout]);` then \
              `<Ctx.Provider value={v}>`.",
    ),
    doc(
        "widening-info",
        "state slot required widening to converge (precision lost here)",
        "Informational (--info): the fixpoint only converged on this state slot \
                      by widening its abstract value (e.g. an interval jumped to +∞). \
                      Downstream checks on this slot are less precise. Often accompanies a \
                      real divergence (see infinite-loop) but is not itself a bug.",
        "useEffect(() => setN(n + 1), [n]); // n widens to [0, +∞)",
        "Not a code bug per se. See the companion diagnostic on the same slot if any.",
    ),
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
        let names: Vec<&str> = RULE_DOCS.iter().map(|d| d.name.as_ref()).collect();
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
