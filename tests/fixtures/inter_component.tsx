// Fixtures for inter-component analysis tests.
// Each section is a standalone mini-program.

// ── SECTION 1: Setter passed as prop, called by child effect ─────────────────
// Expected: Parent.count state updated via SharedStateStore after Child runs.

export const Section1_Child = ({ onChange }) => {
    React.useEffect(() => {
        onChange(42);
    }, [onChange]);
    return <div>Child</div>;
};

export const Section1_Parent = () => {
    const [count, setCount] = React.useState(0);
    return <Section1_Child onChange={setCount} />;
};

// ── SECTION 2: Stable prop reference ─────────────────────────────────────────
// Setter is always stable — child receives ComponentSetter (Stable).

export const Section2_Display = ({ value }) => {
    return <div>{value}</div>;
};

export const Section2_App = () => {
    const [val, setVal] = React.useState(0);
    return <Section2_Display value={val} />;
};

// ── SECTION 3: Prop with literal (stable primitive) ──────────────────────────

export const Section3_Label = ({ text }) => {
    return <div>{text}</div>;
};

export const Section3_App = () => {
    return <Section3_Label text="hello" />;
};

// ── SECTION 4: Recursive component (should not crash) ────────────────────────

export const Section4_TreeNode = ({ depth }) => {
    if (depth <= 0) return <div>Leaf</div>;
    return <Section4_TreeNode depth={depth - 1} />;
};

// ── SECTION 5: missing-deps — unstable callback prop not in deps ──────────────
// Child receives an inline callback (new fn each render → Reference(Unstable)).
// The effect uses it but doesn't declare it in deps: [].
// Expected rule: MissingDeps fires on Section5_Child for `onUpdate`.

export const Section5_Child = ({ onUpdate }) => {
    React.useEffect(() => {
        onUpdate(1);
    }, []); // onUpdate is unstable — should be in deps
    return <span/>;
};

export const Section5_Parent = () => {
    const [val, setVal] = React.useState(0);
    return <Section5_Child onUpdate={(v) => setVal(v)} />;
};

// ── SECTION 6: missing-deps — stable setter prop, no warning ─────────────────
// setVal is a useState setter → ComponentSetter (always stable).
// Even though onUpdate is not in deps: [], it's stable → no MissingDeps.
// Expected rule: MissingDeps does NOT fire on Section6_Child.

export const Section6_Child = ({ onUpdate }) => {
    React.useEffect(() => {
        onUpdate(1);
    }, []); // onUpdate is stable (ComponentSetter) — no warning
    return <span/>;
};

export const Section6_Parent = () => {
    const [val, setVal] = React.useState(0);
    return <Section6_Child onUpdate={setVal} />;
};

// ── SECTION 7: Prop drilling (grandparent → parent → child) ──────────────────
// setV flows through two levels: Root → Middle → Leaf.
// Leaf's effect calls action(99) → updates SharedStateStore[(Section7_Root, 0)].
// Expected: SharedStateStore[(Section7_Root, 0)] = Number(99).

export const Section7_Leaf = ({ action }) => {
    React.useEffect(() => { action(99); }, [action]);
    return <span/>;
};

export const Section7_Middle = ({ action }) => {
    return <Section7_Leaf action={action} />;
};

export const Section7_Root = () => {
    const [v, setV] = React.useState(0);
    return <Section7_Middle action={setV} />;
};

// ── SECTION 8: Multiple CompApps inside NativeElem ───────────────────────────
// Two children inside a div, both receiving the same setter.
// Both effects fire → SharedStateStore[(Section8_App, 0)] = join(1, 2) = [1,2].
// Tests that CompApp nodes inside NativeElem children are analyzed.

export const Section8_BtnA = ({ onClick }) => {
    React.useEffect(() => { onClick(1); }, []);
    return <span/>;
};

export const Section8_BtnB = ({ onClick }) => {
    React.useEffect(() => { onClick(2); }, []);
    return <span/>;
};

export const Section8_App = () => {
    const [sel, setSel] = React.useState(0);
    return (
        <div>
            <Section8_BtnA onClick={setSel} />
            <Section8_BtnB onClick={setSel} />
        </div>
    );
};

// ── SECTION 9: cross-setter-in-render — unconditional call ───────────────────
// Child receives a ComponentSetter as prop and calls it unconditionally in render.
// cross-setter-in-render must fire on Section9_Child (Error: call dominates all exits).

export const Section9_Child = ({ reset }) => {
    reset(0); // unconditional call in render via ComponentSetter prop
    return <span/>;
};

export const Section9_Parent = () => {
    const [count, setCount] = React.useState(0);
    return <Section9_Child reset={() => setCount(0)} />;
};

// ── SECTION 10: cross-component-infinite-loop — no-deps effect ───────────────
// Child's no-deps effect calls parent's setter every render → infinite loop.
// cross-component-infinite-loop must fire on Section10_InfiniteChild.
// Analysis must terminate (widening ensures convergence).

export const Section10_InfiniteChild = ({ bump }) => {
    React.useEffect(() => {
        bump(1); // called every render (no deps)
    });
    return <span/>;
};

export const Section10_Parent = () => {
    const [n, setN] = React.useState(0);
    return <Section10_InfiniteChild bump={setN} />;
};

// ── SECTION 20: cross-setter-in-render — no fire (setter only in event handler)
// Child receives a ComponentSetter as prop but only uses it in a JSX onClick
// callback, never calls it directly in the render body.
// cross-setter-in-render must NOT fire.

export const Section20_SafeChild = ({ onSubmit }) => {
    return <button onClick={() => onSubmit(1)}>Submit</button>;
};

export const Section20_Parent = () => {
    const [submitted, setSubmitted] = React.useState(false);
    return <Section20_SafeChild onSubmit={setSubmitted} />;
};

// ── SECTION 21: cross-setter-in-render — conditional call (Warning) ───────────
// Child calls the ComponentSetter prop only on a conditional path.
// cross-setter-in-render fires with Warning (not Error): call is conditional.

export const Section21_Child = ({ reset, active }) => {
    if (active) {
        reset(0); // conditional call → Warning
    }
    return <span/>;
};

export const Section21_Parent = () => {
    const [count, setCount] = React.useState(0);
    return <Section21_Child reset={setCount} active={count > 0} />;
};

// ── SECTION 22: cross-component-infinite-loop — no fire (mount-only effect) ──
// Child's effect has deps: [] → fires once on mount, not on every render.
// cross-component-infinite-loop must NOT fire.

export const Section22_Child = ({ onMount }) => {
    React.useEffect(() => {
        onMount(1); // mount-only: deps []
    }, []);
    return <span/>;
};

export const Section22_Parent = () => {
    const [n, setN] = React.useState(0);
    return <Section22_Child onMount={setN} />;
};

// ── SECTION 23: cross-component-infinite-loop — fires (all deps unstable) ─────
// Child writes n+1 to parent's state via onChange.
// Parent's n widens → [0,+inf] (unbounded in SharedStateStore).
// [value] is entirely unstable (Number widens) → effect runs every render.
// cross-component-infinite-loop must fire.

export const Section23_Child = ({ onChange, value }) => {
    React.useEffect(() => {
        onChange(value + 1);
    }, [value]);
    return <span/>;
};

export const Section23_Parent = () => {
    const [n, setN] = React.useState(0);
    return <Section23_Child onChange={setN} value={n} />;
};

// ── SECTION 28: cross-component-infinite-loop — fires (no deps, unbounded write)
// Child no-deps effect writes `n+1` to parent's state every render.
// SharedStateStore grows without bound → proven infinite loop.

export const Section28_Child = ({ bump, n }) => {
    React.useEffect(() => {
        bump(n + 1); // unbounded increment via no-deps effect
    });
    return <span/>;
};

export const Section28_Parent = () => {
    const [n, setN] = React.useState(0);
    return <Section28_Child bump={setN} n={n} />;
};

// ── SECTION 24: setter-in-render via local wrapper function ──────────────────
// Component defines a local helper that calls its own setter, then calls
// that helper unconditionally in render.
// setter-in-render must fire (Error: unconditional, block_id propagated from B6).

export const Section24_Counter = () => {
    const [count, setCount] = React.useState(0);
    const doReset = () => setCount(0); // local wrapper
    doReset(); // ❌ calls setter indirectly in render
    return <span>{count}</span>;
};

// ── SECTION 25: cross-setter-in-render via local wrapper in child ─────────────
// Child wraps the ComponentSetter prop in a local function, then calls it in render.
// cross-setter-in-render must fire (Error: unconditional + outer block_id propagated).

export const Section25_Child = ({ reset }) => {
    const handleReset = () => reset(0); // wraps parent's setter
    handleReset(); // ❌ calls ComponentSetter indirectly in render
    return <span/>;
};

export const Section25_Parent = () => {
    const [count, setCount] = React.useState(0);
    return <Section25_Child reset={setCount} />;
};

// ── SECTION 26: setter-in-render via two-level wrapper (depth=2) ─────────────
// Two levels of indirection: render calls wrapper1 → wrapper2 → setter.
// setter-in-render must fire (depth=2 catches it).

export const Section26_Counter = () => {
    const [n, setN] = React.useState(0);
    const inner = () => setN(0);
    const outer = () => inner(); // two-level wrapper
    outer(); // ❌ setter reached via two B6 steps
    return <span/>;
};

// ── SECTION 27: no fire — local wrapper only called in event handler ──────────
// Local wrapper calls setter, but only from a JSX onClick, not in render body.
// setter-in-render must NOT fire.

export const Section27_Safe = () => {
    const [n, setN] = React.useState(0);
    const handleClick = () => setN(n + 1); // wrapper — only used in onClick
    return <button onClick={handleClick}>{n}</button>;
};

// ── SECTION 11: conditional-hook — hook inside condition based on prop ────────
// Child calls useState inside an if-block gated on prop.
// ConditionalHook must fire on Section11_Child.

export const Section11_Child = ({ show }) => {
    if (show) {
        const [x, setX] = React.useState(0);
    }
    return <span/>;
};

export const Section11_Parent = () => {
    const [visible, setVisible] = React.useState(true);
    return <Section11_Child show={visible} />;
};

// ── SECTION 12: setter-in-render — child's own setter called unconditionally ──
// SetterInRender must fire on Section12_Child (setVal in render body).
// Parent provides the value via prop; child misuses it as a render-time setter arg.

export const Section12_Child = ({ initialValue }) => {
    const [val, setVal] = React.useState(0);
    setVal(initialValue); // unconditional setter call in render
    return <span/>;
};

export const Section12_Parent = () => {
    const [m, setM] = React.useState(1);
    return <Section12_Child initialValue={m} />;
};

// ── SECTION 13: redundant-set-state — INTER-SPECIFIC ────────────────────────
// Parent passes label="hello" (string literal → StrConst stable).
// Child's init is also "hello". Effect always writes the same label → redundant.
//
// INTER-SPECIFIC:
//   intra:  label = Top → setVal(Top) → state = StrConst("hello") ⊔ Top = Top
//           Top.is_stable() = false → does NOT fire
//   inter:  label = StrConst("hello") → setVal(StrConst("hello"))
//           state = StrConst("hello") ⊔ StrConst("hello") = StrConst("hello") → stable
//           both arg and state stable → fires

export const Section13_Child = ({ label }) => {
    const [val, setVal] = React.useState("hello"); // init same as prop
    React.useEffect(() => {
        setVal(label); // parent always passes "hello" → writing same value → redundant
    }, [label]);
    return <span/>;
};

export const Section13_Parent = () => {
    return <Section13_Child label="hello" />;
};

// ── SECTION 14: infinite-loop — child effect increments own state unconditionally
// Child: setCount(count + 1) in effect with deps [count] → count grows without bound.
// InfiniteLoop must fire on Section14_Child (count widens via self-incrementing effect).
// Parent prop `step` is used as increment to make it a true cross-component scenario.

export const Section14_Child = ({ step }) => {
    const [count, setCount] = React.useState(0);
    React.useEffect(() => {
        setCount(count + step);
    }, [count]);
    return <span/>;
};

export const Section14_Parent = () => {
    const [s, setS] = React.useState(1);
    return <Section14_Child step={s} />;
};

// ── SECTION 15: derived-state — child mirrors parent state into local state ────
// Child stores `total * 2` in local state via effect.
// DerivedState should fire on Section15_Child: doubled = f(total) suggests useMemo.

export const Section15_Child = ({ total }) => {
    const [doubled, setDoubled] = React.useState(0);
    React.useEffect(() => {
        setDoubled(total * 2);
    }, [total]);
    return <span/>;
};

export const Section15_Parent = () => {
    const [n, setN] = React.useState(5);
    return <Section15_Child total={n} />;
};

// ── SECTION 16: missing-deps on useCallback capturing unstable prop ───────────
// Child's useCallback reads `data.x` but has empty deps. `data` is unstable
// (Parent passes a useState({...}) → Reference(Unstable) reference).
// MissingDeps must fire on Section16_Child for `data`.

export const Section16_Child = ({ data }) => {
    const cb = React.useCallback(() => data.x, []); // ❌ missing-deps: data
    return <button onClick={cb}>x</button>;
};

export const Section16_Parent = () => {
    const [data, setData] = React.useState({ x: 1 });
    return <Section16_Child data={data} />;
};

// ── SECTION 17: missing-deps on useMemo — INTER-SPECIFIC FP suppression ───────
// Child's useMemo reads `label.length` but doesn't declare `label`.
// Parent passes a string literal → StrConst("hello"), Stability::Stable.
// Intra analysis: label = Top → unstable-ish (Unknown via to_stability) → may fire.
// Inter analysis: label = StrConst("hello") (stable) → MissingDeps suppressed.
// Tests that the missing-deps extension to useMemo benefits from inter precision.

export const Section17_Child = ({ label }) => {
    const v = React.useMemo(() => label.length, []); // stable prop ⇒ no warning inter
    return <p>{v}</p>;
};

export const Section17_Parent = () => {
    return <Section17_Child label="hello" />;
};

// ── SECTION 18: always-unstable-deps — child's useEffect with unstable prop ──
// Parent passes an inline object literal as `config` prop, child uses it as
// the only dep. Inline ObjectLit → Reference(Unstable) propagated via inter.
// AlwaysUnstableDeps must fire on Section18_Child: [config] is entirely unstable.

export const Section18_Child = ({ config }) => {
    React.useEffect(() => {
        console.log(config);
    }, [config]); // ❌ always-unstable-deps: config is Reference(Unstable)
    return <div/>;
};

export const Section18_Parent = () => {
    return <Section18_Child config={{ x: 1 }} />;
};

// ── SECTION 19: lazy-init in a child component ───────────────────────────────
// Child uses `useState(expensive(seed))` — structural rule, fires regardless
// of inter vs intra. Verifies the rule works on lowered children.

export const Section19_Child = ({ seed }) => {
    const [v, setV] = React.useState(expensive(seed)); // ❌ lazy-init
    return <p>{v}</p>;
};

export const Section19_Parent = () => {
    return <Section19_Child seed={42} />;
};
