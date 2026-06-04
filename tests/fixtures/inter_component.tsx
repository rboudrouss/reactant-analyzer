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

// ── SECTION 9: Setter-in-render via prop (limitation test) ───────────────────
// Child receives a ComponentSetter as prop and calls it unconditionally in render.
// Current limitation: setter_in_render rule uses setter_bindings (not ComponentSetter
// stabs) → rule does NOT fire cross-component. Documents known gap.
// Expected: no crash, analysis completes normally.

export const Section9_Child = ({ reset }) => {
    reset(0); // unconditional call in render via ComponentSetter prop
    return <span/>;
};

export const Section9_Parent = () => {
    const [count, setCount] = React.useState(0);
    return <Section9_Child reset={setCount} />;
};

// ── SECTION 10: No-deps effect calling parent setter (potential infinite loop) ─
// Child's no-deps effect calls parent's setter every render.
// Tests that SharedStateStore is updated and parent state re-converges.
// Note: InfiniteLoop rule may or may not fire depending on widening; key test is
// that analysis terminates (widening ensures convergence).

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
