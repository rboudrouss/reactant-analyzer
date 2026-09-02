# Wish-list: rendering, referential identity and performance rules

Domain: re-render cascades, `useMemo` / `useCallback` / `React.memo`, context providers and
consumers, unstable identities crossing component boundaries, expensive render-body work,
key/identity churn, render-phase side effects.

Every scenario below assumes an engine that can answer, at minimum: which phase a piece of code
runs in, which allocation site a value came from, whether two values are the same reference at
runtime, and how props flow from a parent's JSX into a callee's body.

---

### S-RENDER-1: memo-defeated-by-inline-prop-allocation
- **What it flags:** a `React.memo`-wrapped component receiving a prop whose value is freshly allocated at a render-phase allocation site in the parent, so the memo comparison can never succeed.
- **Why it matters:** the team paid for `memo` and gets nothing: typing one character in the filter box re-renders all 500 rows, each re-running its own render body and diffing its own subtree. Input latency goes from 2 ms to 80 ms and the field visibly lags behind the keyboard.
- **Severity intent:** warning
- **Fires on:**
```tsx
import { memo, useState } from "react";

type Item = { id: string; label: string };
function select(id: string) { history.pushState(null, "", `/item/${id}`); }

const Row = memo(function Row({ item, onPick }: { item: Item; onPick: (id: string) => void }) {
  return <li onClick={() => onPick(item.id)}>{item.label}</li>;
});

export function List({ items }: { items: Item[] }) {
  const [q, setQ] = useState("");
  const shown = items.filter((i) => i.label.includes(q));
  return (
    <ul>
      <input value={q} onChange={(e) => setQ(e.target.value)} />
      {shown.map((it) => (
        <Row key={it.id} item={it} onPick={(id) => select(id)} />
      ))}
    </ul>
  );
}
```
- **Silent on:**
```tsx
import { memo, useState } from "react";

type Item = { id: string; label: string };
function select(id: string) { history.pushState(null, "", `/item/${id}`); }

const Row = memo(function Row({ item, onPick }: { item: Item; onPick: (id: string) => void }) {
  return <li onClick={() => onPick(item.id)}>{item.label}</li>; // inline arrow on a host element
});

export function List({ items }: { items: Item[] }) {
  const [q, setQ] = useState("");
  return (
    <ul>
      <input value={q} onChange={(e) => setQ(e.target.value)} /> {/* inline arrow, host element */}
      {items.map((it) => (
        <Row key={it.id} item={it} onPick={select} />
      ))}
    </ul>
  );
}
```
- **Semantic facts required:**
  - Which component identifiers are wrapped by `React.memo` (including through `forwardRef(memo(...))`, a re-export, an HOC chain, or a variable that is assigned once at module scope).
  - For each JSX attribute in the parent: whether the attribute's value expression is an *allocation site* (object literal, array literal, function literal, `.map`/`.filter`/spread result, `new`, template-built object) that is evaluated during the parent's render phase.
  - Whether that allocation site is reachable on every render of the parent, i.e. whether the produced reference is provably distinct from the previous render's reference (a per-render-fresh reference, not a value promoted out of the render body).
  - Whether the JSX element's *type* resolves to a memo-wrapped component or to a host element string — a fresh function on `<li onClick>` costs nothing, a fresh function on `<Row onPick>` costs a subtree.
  - Whether the memo has a custom comparator, and whether that comparator inspects the unstable prop at all (if it ignores it, this rule is silent and S-RENDER-9 owns the case).
  - Rough re-render frequency of the parent: which state slots it owns and whether any is written from a high-frequency source (controlled input, scroll/mouse listener, interval). This turns "theoretically wasted" into "wasted 60×/second" and should drive the ranking.

---

### S-RENDER-2: context-value-reallocated-every-render
- **What it flags:** a `Context.Provider` whose `value` is a reference freshly allocated in the provider's render body, when the provider re-renders for reasons unrelated to the value's contents.
- **Why it matters:** context propagation ignores `React.memo` and ignores `shouldComponentUpdate` — every single `useContext` consumer in the subtree re-renders whenever the provider re-renders, even when `user` is byte-for-byte the same object it was. A provider near the root turns any unrelated parent re-render into a full-application re-render.
- **Severity intent:** warning
- **Fires on:**
```tsx
import { createContext, useState, type ReactNode } from "react";

type User = { id: string; name: string };
const AuthCtx = createContext<{ user: User | null; logout: () => void } | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const value = { user, logout: () => setUser(null) };
  return <AuthCtx.Provider value={value}>{children}</AuthCtx.Provider>;
}
```
- **Silent on:**
```tsx
import { createContext, useEffect, useState, type ReactNode } from "react";

type Settings = { theme: "light" | "dark"; locale: string };
const DEFAULTS: Settings = { theme: "light", locale: "en" };
const SettingsCtx = createContext<Settings>(DEFAULTS);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings>(DEFAULTS);
  useEffect(() => onRemoteSettings((next) => setSettings(next)), []);
  return <SettingsCtx.Provider value={settings}>{children}</SettingsCtx.Provider>;
}
```
- **Semantic facts required:**
  - That the JSX element type is a provider (`X.Provider` where `X` flows from a `createContext` call, or React 19's context-as-provider form), and which context object it provides.
  - Whether the `value` expression evaluates to a per-render-fresh reference or to a reference that survives across renders (state slot, ref-held object, module constant, memo output).
  - Whether the provider component can re-render *without* the value's observable content changing — i.e. does it read props, does it own other state slots, is it re-rendered by its parent? If the provider's only re-render trigger is a write to the very slot the value wraps, a fresh wrapper is harmless and the rule must stay quiet.
  - Whether the value is a primitive (identity irrelevant) or a reference.
  - For ranking: how many `useContext(X)` sites exist and how deep the subtree under the provider is.
  - Distinguish this from the *memoized-but-too-coarse* case (S-RENDER-10) — here the value is not memoized at all.

---

### S-RENDER-3: component-type-created-during-render
- **What it flags:** a function literal created during a component's render phase that is used in *element-type position* in JSX, giving the child a new element type on every render.
- **Why it matters:** React compares element types by identity to decide reconciliation vs. remount. A new type each render means the entire subtree is unmounted and remounted every render: DOM nodes are destroyed, uncontrolled inputs lose the user's typed text, focus jumps to `<body>`, CSS transitions restart, `useState` inside the subtree resets, and every child `useEffect` re-runs its cleanup and setup (re-fetching, re-subscribing).
- **Severity intent:** error
- **Fires on:**
```tsx
import { useState, type ReactNode } from "react";

export function Page() {
  const [open, setOpen] = useState(false);

  function Panel({ children }: { children: ReactNode }) {
    return <section className="panel">{children}</section>;
  }

  return (
    <div>
      <button onClick={() => setOpen((o) => !o)}>toggle</button>
      {open && (
        <Panel>
          <input defaultValue="draft" />
        </Panel>
      )}
    </div>
  );
}
```
- **Silent on:**
```tsx
import { useState } from "react";

type Row = { id: string; label: string };

function List({ rows, renderItem }: { rows: Row[]; renderItem: (r: Row) => JSX.Element }) {
  return <ul>{rows.map((r) => renderItem(r))}</ul>; // called, never used as an element type
}

export function Page({ rows }: { rows: Row[] }) {
  const [q, setQ] = useState("");
  const RenderRow = (r: Row) => <li key={r.id}>{r.label.includes(q) ? <b>{r.label}</b> : r.label}</li>;
  return (
    <>
      <input value={q} onChange={(e) => setQ(e.target.value)} />
      <List rows={rows} renderItem={RenderRow} />
    </>
  );
}
```
- **Semantic facts required:**
  - Which function literals are created in the render phase of a component (as opposed to module scope, or inside a factory/HOC that is called once at module scope).
  - For each such function, the set of *use sites* of its value, following it through local bindings, props, context and arrays.
  - At each use site, whether the value lands in element-type position (`<F/>`, `createElement(F, …)`, `<obj.F/>`, an element type held in a variable) or is merely *called* (`F(props)`, `rows.map(F)`, `props.renderItem(x)`). Only the former remounts. Capitalisation is irrelevant; the use site decides.
  - When the function crosses a component boundary as a prop, how the *callee* uses it — this needs cross-component prop flow, and must be conservative (unknown callee ⇒ assume element-type position ⇒ warn) to stay sound.
  - Whether the created type is stable in practice anyway (e.g. produced by a `useMemo` / `useRef` with an empty dep set) — that suppresses the finding.
  - Whether the subtree under the created type contains state, refs, effects, or uncontrolled DOM — this decides error vs. warning, since a remount of a purely presentational leaf costs performance but loses nothing.

---

### S-RENDER-4: key-value-regenerated-in-render
- **What it flags:** a `key` expression whose value provenance is a nondeterministic or per-render-fresh computation (a generated id, a counter, a clock read, a hash of a freshly allocated object) evaluated during the render phase.
- **Why it matters:** the key looks perfectly idiomatic (`key={t.id}`) but the `id` is minted anew on every render, so React sees a completely different set of children each time. Every row unmounts and remounts on every parent render: text typed into a row's input disappears, the row losing focus mid-typing, checkbox state resets, and enter/exit animations fire continuously.
- **Severity intent:** error
- **Fires on:**
```tsx
import { useState } from "react";

function TodoRow({ todo }: { todo: { id: string; text: string } }) {
  return <li><input defaultValue={todo.text} /></li>;
}

export function Todos({ raw }: { raw: string[] }) {
  const [filter, setFilter] = useState("");
  const todos = raw.map((text) => ({ id: crypto.randomUUID(), text }));
  return (
    <>
      <input value={filter} onChange={(e) => setFilter(e.target.value)} />
      <ul>
        {todos.filter((t) => t.text.includes(filter)).map((t) => (
          <TodoRow key={t.id} todo={t} />
        ))}
      </ul>
    </>
  );
}
```
- **Silent on:**
```tsx
import { useState } from "react";

type Raw = { uuid: string; text: string };

function TodoRow({ todo }: { todo: { id: string; text: string } }) {
  return <li><input defaultValue={todo.text} /></li>;
}

export function Todos({ raw }: { raw: Raw[] }) {
  const [filter, setFilter] = useState("");
  const todos = raw.map((r) => ({ id: r.uuid, text: r.text })); // fresh array, fresh objects…
  return (
    <>
      <input value={filter} onChange={(e) => setFilter(e.target.value)} />
      <ul>
        {todos.filter((t) => t.text.includes(filter)).map((t) => (
          <TodoRow key={t.id} todo={t} /> // …but the key *value* is stable
        ))}
      </ul>
    </>
  );
}
```
- **Semantic facts required:**
  - The abstract value flowing into the `key` attribute, tracked back through array element construction, object property writes, destructuring and spreads — the key is a *string value*, not a reference, so object freshness is irrelevant and only value provenance counts.
  - Whether any node on that provenance chain is a nondeterministic source: `crypto.randomUUID`, `Math.random`, `Date.now`, `new Date()`, `performance.now`, an incrementing module counter, `Symbol()`, `useId` called per item, an object's default `toString`.
  - Whether that source is evaluated in the render phase on every render, or once (module init, lazy `useState` initializer, `useRef` initialisation, `useMemo` with a dep set that is provably stable).
  - Which values are stable across renders for a fixed logical item: the analysis must distinguish "the array is a fresh array of fresh objects" (fine) from "the string in `.id` differs from last render" (fatal).
  - What the keyed subtree would lose on remount: uncontrolled inputs, focus, `useState`, effect setup/cleanup with I/O. That is the payload of the diagnostic message.
  - Reordering-only churn (stable set of key values, different order) is a different, benign situation and must not be conflated.

---

### S-RENDER-5: unmemoized-expensive-render-body
- **What it flags:** a computation in the render body whose cost scales with an unbounded input (parse, sort, deep clone, nested loop, regex over a large string), executed unconditionally on every render, when the component re-renders more often than the computation's inputs change.
- **Why it matters:** on every keystroke in the filter box the component re-parses a 400 KB JSON string and re-sorts 5 000 rows on the main thread. The frame budget is 16 ms; this takes 90 ms. The input drops characters, scrolling stutters, and the browser shows the "page unresponsive" tell on low-end devices.
- **Severity intent:** warning
- **Fires on:**
```tsx
import { useState } from "react";

type Row = { name: string; score: number };

export function Leaderboard({ rowsJson }: { rowsJson: string }) {
  const [filter, setFilter] = useState("");
  const rows = JSON.parse(rowsJson) as Row[];
  rows.sort((a, b) => b.score - a.score);
  const top = rows.filter((r) => r.name.includes(filter)).slice(0, 20);
  return (
    <>
      <input value={filter} onChange={(e) => setFilter(e.target.value)} />
      <ol>{top.map((r) => <li key={r.name}>{r.name}: {r.score}</li>)}</ol>
    </>
  );
}
```
- **Silent on:**
```tsx
import { useMemo, useState } from "react";

type Row = { name: string; score: number };

export function Leaderboard({ rowsJson }: { rowsJson: string }) {
  const [filter, setFilter] = useState("");
  const ranked = useMemo(() => {
    const rows = JSON.parse(rowsJson) as Row[];
    return rows.sort((a, b) => b.score - a.score);
  }, [rowsJson]);
  const top = ranked.filter((r) => r.name.includes(filter)).slice(0, 20); // cheap, correctly left in render
  return (
    <>
      <input value={filter} onChange={(e) => setFilter(e.target.value)} />
      <ol>{top.map((r) => <li key={r.name}>{r.name}: {r.score}</li>)}</ol>
    </>
  );
}
```
- **Semantic facts required:**
  - Phase attribution: the expression is evaluated in the render phase of a component (not inside an event handler, effect body, lazy initializer, or a callback that only runs later).
  - Reachability: the expression is evaluated on *every* path through the render body, not behind a rarely-taken branch.
  - A cost model over the abstract value: does the operand's size have a known small bound (a 3-key literal, a fixed-length tuple) or is it unbounded (a prop array, a string of unknown length, a network payload)? `Object.keys(props.style)` must not be flagged; `props.rows.sort()` must.
  - Whether the same computation is already inside a `useMemo`/`useCallback`/`useRef` cache in this component, or is re-derived from a value the component already computed.
  - Render frequency vs. input change frequency: the component owns a state slot written on every keystroke / scroll / mousemove, while the computation's inputs are props that change rarely. If the inputs change on every render too, memoizing buys nothing and the rule should stay quiet or downgrade.
  - Whether the result feeds only a conditionally-rendered branch (then the fix is to move the computation, not to memoize it).

---

### S-RENDER-6: eager-usestate-initializer
- **What it flags:** a `useState` (or `useReducer`) initial-value argument that is a non-trivial call expression rather than a thunk, so it is evaluated on every render and its result discarded after the first.
- **Why it matters:** `localStorage.getItem` is a synchronous, main-thread, cross-process read; wrapping it in a `JSON.parse` of a large draft makes it worse. In a controlled editor this runs on every keystroke, for a value React throws away every time after mount. Worse, if the initializer has side effects (starting a timer, opening a socket, incrementing a counter) those side effects happen on every render and leak.
- **Severity intent:** warning (error when the discarded expression is impure)
- **Fires on:**
```tsx
import { useState } from "react";

type Draft = { title: string; body: string };

export function Editor({ docId }: { docId: string }) {
  const [draft, setDraft] = useState<Draft>(
    JSON.parse(localStorage.getItem(`draft:${docId}`) ?? '{"title":"","body":""}'),
  );
  return (
    <textarea
      value={draft.body}
      onChange={(e) => setDraft((d) => ({ ...d, body: e.target.value }))}
    />
  );
}
```
- **Silent on:**
```tsx
import { useState } from "react";

type Draft = { title: string; body: string };
function makeEmptyDraft(): Draft { return { title: "", body: "" }; }

export function Editor({ initial }: { initial?: Draft }) {
  const [draft, setDraft] = useState<Draft>(initial ?? makeEmptyDraft());
  return (
    <textarea
      value={draft.body}
      onChange={(e) => setDraft((d) => ({ ...d, body: e.target.value }))}
    />
  );
}
```
- **Semantic facts required:**
  - That the callee is `useState`/`useReducer` and the argument is in the *initial value* position, and that the argument is not itself a function value (a passed thunk, a function-typed state whose author intended the thunk form is a separate concern).
  - Whether the argument expression is a call, and the transitive cost of that call: a constant-time allocation of a small literal (`makeEmptyDraft()`) is fine; a call reaching synchronous I/O (`localStorage`, `document.cookie`, `getBoundingClientRect`), a parse of an unbounded string, or an unbounded loop is not.
  - Whether the argument expression is *pure*: does it write to anything reachable outside the call, register a listener, start a timer, or mutate a module value? An impure discarded initializer is a correctness bug, not just waste, and must be reported at error level.
  - Whether the initializer is only reached on some renders (behind a `??` on a cheap prop, as in the near-miss) — cheap short-circuited fallbacks must stay silent.
  - Render frequency of the owning component, again, for ranking.

---

### S-RENDER-7: memo-hook-with-per-render-fresh-dependency
- **What it flags:** a `useMemo` / `useCallback` whose dependency list contains a value that is provably a fresh reference on every render — including when that value arrives as a prop allocated inline by the caller — so the memo never hits.
- **Why it matters:** the memoization is a lie that propagates. `search` is a new function every render, so the `memo` on `Results` never bails out and any effect in `Results` that depends on `onSearch` tears down and re-runs. The team believes the subtree is memoized, so they keep adding weight to it, and the whole thing re-renders on every keystroke in an unrelated input.
- **Severity intent:** warning
- **Fires on:**
```tsx
import { memo, useCallback, useEffect, useState } from "react";

type Options = { limit: number; fuzzy: boolean };

const Results = memo(function Results({ onSearch }: { onSearch: (q: string) => void }) {
  useEffect(() => { onSearch(""); }, [onSearch]);
  return <div />;
});

function SearchBox({ options }: { options: Options }) {
  const search = useCallback((q: string) => runSearch(q, options), [options]);
  return <Results onSearch={search} />;
}

export function Page() {
  const [q, setQ] = useState("");
  return (
    <>
      <input value={q} onChange={(e) => setQ(e.target.value)} />
      <SearchBox options={{ limit: 20, fuzzy: true }} />
    </>
  );
}
```
- **Silent on:**
```tsx
import { memo, useCallback, useEffect, useMemo, useState } from "react";

type Options = { limit: number; fuzzy: boolean };

const Results = memo(function Results({ onSearch }: { onSearch: (q: string) => void }) {
  useEffect(() => { onSearch(""); }, [onSearch]);
  return <div />;
});

function SearchBox({ options }: { options: Options }) {
  const search = useCallback((q: string) => runSearch(q, options), [options]);
  return <Results onSearch={search} />;
}

export function Page({ limit, fuzzy }: { limit: number; fuzzy: boolean }) {
  const [q, setQ] = useState("");
  const options = useMemo(() => ({ limit, fuzzy }), [limit, fuzzy]); // object literal in render, but stable
  return (
    <>
      <input value={q} onChange={(e) => setQ(e.target.value)} />
      <SearchBox options={options} />
    </>
  );
}
```
- **Semantic facts required:**
  - A *stability lattice* over values: `stable-forever` (module constant, ref-held, setter from `useState`, `dispatch`), `stable-until-X-changes` (memo output keyed on a dep set), `fresh-every-render` (render-phase allocation site), `unknown` (from an opaque call or an unanalysable import). The rule fires only on `fresh-every-render`.
  - Propagation of that lattice *across component boundaries*: the stability of `options` inside `SearchBox` is a property of the caller's JSX attribute, joined over all call sites of `SearchBox` in the program. With multiple callers, join conservatively.
  - For a `useMemo`-produced dep, whether that upstream memo's own dep set is itself stable — i.e. the lattice must be computed to a fixpoint over the hook dependency graph, not one level deep.
  - Which values are primitives (compared by value, never "fresh") vs. references.
  - Whether the flagged hook's output feeds an identity-sensitive sink at all (memo'd child prop, dep array, context value, `useSyncExternalStore` argument) — if it feeds nothing identity-sensitive, S-RENDER-8 owns the case instead and the message should differ.
  - Distinguish "never hits because a dep is a fresh reference" (fixable by stabilising the dep) from "never hits because a dep is a primitive that genuinely changes every render" (not fixable; stay silent).

---

### S-RENDER-8: memoization-with-no-identity-sensitive-consumer
- **What it flags:** a `useMemo` / `useCallback` whose computation is provably cheap **and** whose result never reaches a position where referential identity is observed.
- **Why it matters:** not a runtime bug, a cost and a comprehension tax. Each hook allocates a closure and a dep array and runs a comparison loop on every render — usually more work than the multiplication it guards. More importantly it teaches the codebase that "everything is wrapped", which hides the three places where the wrapping is actually load-bearing and makes reviewers stop reading dep lists.
- **Severity intent:** info
- **Fires on:**
```tsx
import { useCallback, useMemo } from "react";

export function Price({ cents, count, sku }: { cents: number; count: number; sku: string }) {
  const total = useMemo(() => (cents * count) / 100, [cents, count]);
  const onBuy = useCallback(() => track("buy", sku), [sku]);
  return <button onClick={onBuy}>Pay {total.toFixed(2)}</button>;
}
```
- **Silent on:**
```tsx
import { memo, useCallback, useMemo } from "react";

const Buy = memo(function Buy({ style, onBuy }: { style: { w: number }; onBuy: () => void }) {
  return <button style={{ width: style.w }} onClick={onBuy}>Pay</button>;
});

export function Price({ w, sku }: { w: number; sku: string }) {
  const style = useMemo(() => ({ w }), [w]); // trivially cheap, but identity is load-bearing
  const onBuy = useCallback(() => track("buy", sku), [sku]);
  return <Buy style={style} onBuy={onBuy} />;
}
```
- **Semantic facts required:**
  - The full set of sinks the hook's result flows to, following local bindings, destructuring, object/array construction, returns from custom hooks, and props into callees.
  - A classification of each sink as identity-sensitive or not. Identity-sensitive: a prop of a `memo`-wrapped component, an entry in any hook dep array (here or in a callee), a `Context.Provider` value, an argument to `useSyncExternalStore`, an operand of `===`/`Object.is`/`Map`/`Set` keying, a value stored to compare against later, a `key`. Not identity-sensitive: string interpolation, arithmetic, a prop of a host element, a prop of a non-memo component, an `onClick` on a DOM node.
  - Whether the memoized computation is cheap: constant-time arithmetic, a property read, a small literal allocation. If it is expensive, the memo earns its keep even with no identity-sensitive consumer, and the rule must stay silent.
  - Whether the callee that receives the value is analysable at all; an opaque or externally-exported consumer means "unknown sink" ⇒ stay silent (this is an info rule; precision matters more than recall).
  - Whether the hook's dep list is empty and the value is used as a stable token deliberately (a `useCallback(fn, [])` handed to a subscription API) — that is identity-sensitive by intent.

---

### S-RENDER-9: memo-comparator-ignores-an-observed-prop
- **What it flags:** a custom `areEqual` passed to `React.memo` that returns `true` while ignoring a prop the component body actually reads.
- **Why it matters:** a certain stale-UI bug, not a performance smell. Flipping to dark mode leaves this chart painted in the light theme until something unrelated changes `data`. Worse, `onHover` is frozen at the closure from the render when `data` last changed, so hovering reports a `selectedId` from three interactions ago — the app dispatches an action about the wrong row and the user sees the wrong detail panel.
- **Severity intent:** error
- **Fires on:**
```tsx
import { memo } from "react";

type Props = { data: number[]; theme: "light" | "dark"; onHover: (i: number) => void };

export const Chart = memo(
  function Chart({ data, theme, onHover }: Props) {
    return (
      <svg className={theme}>
        {data.map((v, i) => <rect key={i} height={v} onMouseMove={() => onHover(i)} />)}
      </svg>
    );
  },
  (a, b) => a.data === b.data,
);
```
- **Silent on:**
```tsx
import { memo } from "react";

type Props = { items: string[]; onSelect: (s: string) => void; debugLabel?: string };

export const Picker = memo(
  function Picker({ items, onSelect }: Props) { // debugLabel is never read
    return <ul>{items.map((s) => <li key={s} onClick={() => onSelect(s)}>{s}</li>)}</ul>;
  },
  (a, b) =>
    a.onSelect === b.onSelect &&
    a.items.length === b.items.length &&
    a.items.every((s, i) => s === b.items[i]), // deliberate content comparison, not ===
);
```
- **Semantic facts required:**
  - The set of props the component body *observes*: every property read on the props object, including through destructuring (nested and defaulted), rest spreads (`{...rest}` observes everything not destructured), `props[k]` with a non-constant key (observes everything), and any prop forwarded to a child that reads it.
  - The set of props the comparator *discriminates on*: which property paths of `a` and `b` it reads and compares, and whether every path is reached on the `return true` path (a comparator that short-circuits can leave a prop uncompared on some path).
  - Whether the comparator can return `true` while some observed prop differs — this is the actual property to prove, and it must account for content comparisons (`length` + elementwise `===` is a valid discrimination for an array prop) and version fields (`a.rev === b.rev` is valid if `rev` provably changes whenever the data does).
  - For a function-typed prop that is ignored: whether that function is a closure over values that change, i.e. whether blocking the re-render freezes a stale closure. A prop that is a stable module function can safely be skipped.
  - Whether the ignored prop is genuinely unread (dead prop kept for API compatibility) — then the comparator is correct and the rule must stay silent, however lopsided it looks syntactically.
  - Whether children are among the props: `children` is a fresh element tree on nearly every parent render and ignoring it is almost always the intent — but it must be reported when the component renders `children`.

---

### S-RENDER-10: context-value-mixes-update-frequencies
- **What it flags:** a single, correctly-memoized context value that bundles a high-frequency field with a low-frequency field, when consumers exist that read only the low-frequency field.
- **Why it matters:** `Header` reads only `user`, but the context value is re-created on every mouse move, so `Header` and everything below it re-render at 60 fps for the entire session. Nothing is visibly wrong, so this is never caught in review; it shows up as a device that gets hot, a fan that spins during a static page, and 40 % of the profile spent in components that render identical output. The fix (split into two contexts) is invisible unless you know which consumer reads which field.
- **Severity intent:** info
- **Fires on:**
```tsx
import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

type User = { id: string; name: string };
type Ctx = { user: User; cursor: { x: number; y: number } };
const AppCtx = createContext<Ctx | null>(null);

export function AppProvider({ user, children }: { user: User; children: ReactNode }) {
  const [cursor, setCursor] = useState({ x: 0, y: 0 });
  useEffect(() => {
    const h = (e: MouseEvent) => setCursor({ x: e.clientX, y: e.clientY });
    window.addEventListener("mousemove", h);
    return () => window.removeEventListener("mousemove", h);
  }, []);
  const value = useMemo(() => ({ user, cursor }), [user, cursor]); // properly memoized
  return <AppCtx.Provider value={value}>{children}</AppCtx.Provider>;
}

export function Header() {
  const { user } = useContext(AppCtx)!; // re-renders 60×/s for a name that never changes
  return <h1>{user.name}</h1>;
}
```
- **Silent on:**
```tsx
import { createContext, useContext, useMemo, useState, type ReactNode } from "react";

type Ctx = { query: string; page: number };
const SearchCtx = createContext<Ctx | null>(null);

export function SearchProvider({ children }: { children: ReactNode }) {
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(1);
  const value = useMemo(() => ({ query, page }), [query, page]);
  return (
    <SearchCtx.Provider value={value}>
      <input value={query} onChange={(e) => { setQuery(e.target.value); setPage(1); }} />
      {children}
    </SearchCtx.Provider>
  );
}

export function Results() {
  const { query, page } = useContext(SearchCtx)!; // reads both fields; splitting buys nothing
  return <div>{query} p{page}</div>;
}
```
- **Semantic facts required:**
  - The field structure of the context value: which property of the provided object comes from which state slot, ref, or prop of the provider.
  - Per state slot, an *update-frequency class*: written from a `mousemove`/`scroll`/`resize`/`pointermove` listener or an interval faster than ~1 s ⇒ high; written from a controlled input's `onChange` ⇒ medium; written from a click, a fetch resolution, or once at mount ⇒ low.
  - Per consumer, the *read set*: which properties of the context value that consumer (and the components it feeds those properties to) actually observes, again handling destructuring, rest spreads and dynamic keys.
  - The pairing: does there exist a consumer whose read set excludes every high-frequency field? Only then is a split beneficial.
  - The cost of the affected consumer subtrees — a 60 fps re-render of a `<span>` is not worth an issue; a 60 fps re-render of a chart is.
  - Whether the value is already memoized (if not, S-RENDER-2 owns it and this rule should not double-report).

---

### S-RENDER-11: high-frequency-state-above-an-invariant-subtree
- **What it flags:** a component that owns a state slot written at high frequency and returns a heavy subtree whose props are provably invariant with respect to that slot, with no memo barrier in between.
- **Why it matters:** every keystroke in the note field re-renders `ExpensiveChart` and `BigTable` from scratch — thousands of `createElement` calls and a full reconciliation pass — to produce byte-identical output. Typing lags by 100 ms on a mid-range laptop. The fix is structural (push the state down into the textarea's own component, or take the heavy subtree as `children` so its element identity is preserved), and no dependency-array rule will ever surface it.
- **Severity intent:** info
- **Fires on:**
```tsx
import { useState } from "react";

type Report = { rows: { id: string; v: number }[] };
function ExpensiveChart({ report }: { report: Report }) { return <svg>{report.rows.map((r) => <rect key={r.id} height={r.v} />)}</svg>; }
function BigTable({ rows }: { rows: Report["rows"] }) { return <table><tbody>{rows.map((r) => <tr key={r.id}><td>{r.v}</td></tr>)}</tbody></table>; }

export function Dashboard({ report }: { report: Report }) {
  const [note, setNote] = useState("");
  return (
    <div>
      <textarea value={note} onChange={(e) => setNote(e.target.value)} />
      <ExpensiveChart report={report} />
      <BigTable rows={report.rows} />
    </div>
  );
}
```
- **Silent on:**
```tsx
import { memo, useState } from "react";

type Report = { rows: { id: string; v: number }[] };
const ExpensiveChart = memo(function ExpensiveChart({ report }: { report: Report }) {
  return <svg>{report.rows.map((r) => <rect key={r.id} height={r.v} />)}</svg>;
});

export function Dashboard({ report }: { report: Report }) {
  const [note, setNote] = useState("");
  return (
    <div>
      <textarea value={note} onChange={(e) => setNote(e.target.value)} />
      <ExpensiveChart report={report} /> {/* memo barrier + prop passed straight through */}
      <Preview markdown={note} />        {/* genuinely depends on the fast slot */}
    </div>
  );
}
```
- **Semantic facts required:**
  - Which state slots the component owns and the update frequency of each (same classification as S-RENDER-10, derived from the phase and source of each setter call).
  - For each child element in the returned JSX: the dependency set of every one of its props — does any prop's value depend, transitively, on the high-frequency slot? This needs value-level slicing of the render body, not just "is the identifier mentioned".
  - Whether a memo barrier stands between the state owner and the heavy subtree: the child is `memo`-wrapped **and** all of its props are stable across a write to the fast slot (which requires S-RENDER-7's stability lattice — a memo'd child with an inline object prop is not a barrier).
  - Whether the child element is the `children` prop passed in from above (React preserves the element object, so it is already a barrier) or constructed here.
  - The rendering cost of the invariant subtree: number of elements it produces, presence of loops over unbounded props, known-heavy leaves (`svg` with per-row nodes, tables, canvas, third-party chart components).
  - Which fix applies — "push state down" is only possible when the fast slot is read by a contiguous, small part of the tree; otherwise the advice is the `children` escape hatch or a memo barrier. The engine needs the read set of the fast slot to choose.

---

### S-RENDER-12: unstable-prop-drives-a-child-effect-dependency
- **What it flags:** a prop that is a per-render-fresh reference in the parent and appears in a `useEffect` / `useLayoutEffect` dependency list in the child, where the effect performs I/O or resource setup.
- **Why it matters:** the parent re-renders once a second because of an unrelated clock, so `query` is a new object every second, so the child's effect cleans up and re-runs every second: a network request per second per mounted `Feed`, forever. Users see the list flicker, the server sees a traffic multiplier, and if the response ever triggers a parent state write the whole thing becomes an unbounded loop.
- **Severity intent:** error
- **Fires on:**
```tsx
import { useEffect, useState } from "react";

type Query = { topic: string; limit: number };
type Item = { id: string; title: string };

function Feed({ query }: { query: Query }) {
  const [items, setItems] = useState<Item[]>([]);
  useEffect(() => {
    let live = true;
    fetchFeed(query).then((r) => { if (live) setItems(r); });
    return () => { live = false; };
  }, [query]);
  return <ul>{items.map((i) => <li key={i.id}>{i.title}</li>)}</ul>;
}

export function Parent() {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setTick((t) => t + 1), 1000);
    return () => clearInterval(id);
  }, []);
  return (
    <>
      <span>{tick}</span>
      <Feed query={{ topic: "react", limit: 20 }} />
    </>
  );
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";

type Query = { topic: string; limit: number };
type Item = { id: string; title: string };

function Feed({ query }: { query: Query }) {
  const [items, setItems] = useState<Item[]>([]);
  useEffect(() => {
    let live = true;
    fetchFeed({ topic: query.topic, limit: query.limit }).then((r) => { if (live) setItems(r); });
    return () => { live = false; };
  }, [query.topic, query.limit]); // unstable object, stable primitive deps
  return <ul>{items.map((i) => <li key={i.id}>{i.title}</li>)}</ul>;
}

export function Parent() {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setTick((t) => t + 1), 1000);
    return () => clearInterval(id);
  }, []);
  return (
    <>
      <span>{tick}</span>
      <Feed query={{ topic: "react", limit: 20 }} />
    </>
  );
}
```
- **Semantic facts required:**
  - Cross-component prop flow: for each parameter of the child, the join over all call sites of the abstract value passed by the callers, and whether that value is a render-phase allocation site in the caller (per S-RENDER-7's lattice).
  - Which dep-array entries denote the whole reference vs. a member path of it (`query` vs `query.topic`) — the second is a value comparison and immune to the parent's allocation churn.
  - Whether the parent actually re-renders more often than the prop's *content* changes: does the parent own a slot written by an interval, a subscription, a scroll listener, or a controlled input? Without that, the finding is theoretical.
  - What the effect body does on re-run: network I/O, `addEventListener`, `setInterval`, `new WebSocket`/`EventSource`, `IntersectionObserver`, an imperative animation start. Effects with no external effect (a `document.title` write) are not worth an error.
  - Whether the effect writes a state slot whose value flows back up to the parent (via a callback prop or a shared store) — that closes a cycle and upgrades the finding to a render-loop diagnosis.
  - Whether a cleanup exists: a missing cleanup on a re-running subscription effect means leaked listeners accumulating per second, which is a distinct and worse consequence to report.

---

### S-RENDER-13: render-phase-write-to-another-component-slot
- **What it flags:** a state setter (or store dispatch) that provably targets a slot owned by a *different* component instance, invoked synchronously during the render phase.
- **Why it matters:** React logs "Cannot update a component (`Parent`) while rendering a different component (`Child`)" and schedules an extra render pass; the user sees an extra frame of stale UI. When the parent's re-render feeds anything back into the child's props, the two ping-pong and the tab freezes with an unbounded render loop — the classic "Maximum update depth exceeded". This must be separated from the *legal* render-phase self-update pattern, which React explicitly blesses.
- **Severity intent:** error
- **Fires on:**
```tsx
import { useState } from "react";

type Row = { id: string; label: string };

function Child({ rows, onCount }: { rows: Row[]; onCount: (n: number) => void }) {
  onCount(rows.length); // render-phase write into Parent's slot
  return <ul>{rows.map((r) => <li key={r.id}>{r.label}</li>)}</ul>;
}

export function Parent({ rows }: { rows: Row[] }) {
  const [count, setCount] = useState(0);
  return (
    <div>
      <Child rows={rows} onCount={setCount} />
      <span>{count} rows</span>
    </div>
  );
}
```
- **Silent on:**
```tsx
import { useState } from "react";

type Row = { id: string; label: string };

export function Child({ rows }: { rows: Row[] }) {
  const [prevLen, setPrevLen] = useState(rows.length);
  const [selected, setSelected] = useState<string | null>(null);
  if (prevLen !== rows.length) {        // guard is falsified by the write below
    setPrevLen(rows.length);
    setSelected(null);
  }
  return (
    <ul>
      {rows.map((r) => (
        <li key={r.id} className={r.id === selected ? "on" : ""} onClick={() => setSelected(r.id)}>
          {r.label}
        </li>
      ))}
    </ul>
  );
}
```
- **Semantic facts required:**
  - For every setter value, the component *instance* whose hook slot it writes — tracked through the `useState` destructuring, through props (`onCount={setCount}`), through context, through a closure captured by a custom hook, and through objects that carry setters as fields.
  - The phase of the call site: render body of a component (including inside a `.map` callback evaluated during render, an IIFE, or a helper function called from the render body) vs. an event handler, an effect, a timer, or a promise continuation. Only the render phase is reportable here.
  - Whether the writing component and the owning component are the same instance. Same-component render-phase writes are legal and must be left alone.
  - For the same-component case, whether the write is guarded by a condition that the write itself falsifies (the "adjusting state during render" pattern) — a convergence argument. An *unguarded* same-component render-phase write is an infinite loop and deserves its own finding, not silence.
  - Whether the setter is called on a path reachable on every render, vs. behind a condition depending on props (which turns a certain error into a conditional one and changes severity).
  - The written value: whether the setter is called with a fresh reference that is structurally equal to the current slot value, since `Object.is` bail-out is what decides whether the loop terminates.

---

### S-RENDER-14: render-body-mutates-state-that-outlives-the-render
- **What it flags:** a write, during the render phase, to any location reachable from outside that render invocation: a module-level binding, a prop or context object, `ref.current`, a DOM node, or a shared cache.
- **Why it matters:** render must be replayable. StrictMode runs it twice in development, concurrent React discards and re-runs renders that get interrupted, and Suspense can throw away a render entirely. So the mutation lands zero, one, or two times, nondeterministically: counters skip, a cache is poisoned with values from a render whose props were never committed, and `rows.sort()` reorders an array the parent still holds — which means the parent's `===` memo checks report "unchanged" for data whose order silently changed, and every other consumer of that array sees a reordering it never asked for. These bugs surface as "only happens in production" or "only happens on slow connections".
- **Severity intent:** error
- **Fires on:**
```tsx
type Row = { id: string; n: number };

let renderSeq = 0;
const cache = new Map<string, Row[]>();

export function Table({ id, rows }: { id: string; rows: Row[] }) {
  renderSeq += 1;
  cache.set(id, rows);
  rows.sort((a, b) => a.n - b.n); // mutates the caller's array in place
  return <tbody>{rows.map((r) => <tr key={r.id}><td>{r.n}</td></tr>)}</tbody>;
}
```
- **Silent on:**
```tsx
type Row = { id: string; n: number };

export function Table({ rows }: { rows: Row[] }) {
  const sorted = [...rows];
  sorted.sort((a, b) => a.n - b.n); // mutates a value allocated in this render
  const seen = new Set<string>();
  for (const r of sorted) seen.add(r.id);
  return (
    <tbody>
      {sorted.map((r) => <tr key={r.id} data-dup={seen.has(r.id)}><td>{r.n}</td></tr>)}
    </tbody>
  );
}
```
- **Semantic facts required:**
  - Phase attribution for every write instruction: is this statement executed during the render phase of some component (directly, or in a helper/custom hook called from a render body, or in a `.map` callback evaluated during render)?
  - Escape analysis of the write target: does the mutated object's allocation site lie inside this render invocation, and does the object not escape into anything that outlives the render (it must not be returned in JSX props, stored to a ref, put in a module binding, or passed to an opaque call)? Purely local scratch mutation is fine and must be silent.
  - Aliasing: whether the mutated binding may alias a value that arrived as a prop, from context, from a ref, or from a module import. `const sorted = rows` followed by `sorted.sort()` is the firing case; `const sorted = [...rows]` is not.
  - Which array/object methods mutate (`sort`, `reverse`, `push`, `splice`, `Map.set`, `Set.add`, `Object.assign` with a non-fresh target) versus which return a new value (`toSorted`, `map`, `filter`, spread).
  - Whether the target is a React-managed mutable cell (`ref.current`), which deserves its own message: writing a ref during render is not observable-safe and the value read back may be from an abandoned render.
  - Whether the mutated value is observed by anyone else — a mutated prop object is far worse than a mutated module counter used only for logging, and the severity should reflect who else holds a reference.
  - Idempotence: a write that always stores the same value derived purely from this render's inputs (a memo cache keyed on the exact inputs) is replay-safe and can be downgraded.

---

### S-RENDER-15: external-store-snapshot-allocated-per-call
- **What it flags:** a `getSnapshot` / `getServerSnapshot` passed to `useSyncExternalStore` (or a reference-compared selector passed to a store hook) that returns a freshly allocated object or array on every invocation.
- **Why it matters:** React calls `getSnapshot` after every render and compares the result with `Object.is` to decide whether the store changed. A new array every call means "changed" every time, so React re-renders, calls it again, and loops. In development you get "The result of getSnapshot should be cached to avoid an infinite loop"; in production you get a pinned CPU core and a frozen tab, and it often only reproduces once a second component subscribes to the same store.
- **Severity intent:** error
- **Fires on:**
```tsx
import { useSyncExternalStore } from "react";

type Item = { id: string; qty: number };
declare const cartStore: { subscribe: (f: () => void) => () => void; items: Item[] };

export function useCartLines(): Item[] {
  return useSyncExternalStore(
    cartStore.subscribe,
    () => cartStore.items.filter((i) => i.qty > 0),
    () => [],
  );
}
```
- **Silent on:**
```tsx
import { useMemo, useSyncExternalStore } from "react";

type Item = { id: string; qty: number };
declare const cartStore: {
  subscribe: (f: () => void) => () => void;
  getItems: () => Item[]; // returns the same array until the store mutates
};

const EMPTY: Item[] = [];

export function useCartLines(): Item[] {
  const items = useSyncExternalStore(cartStore.subscribe, cartStore.getItems, () => EMPTY);
  return useMemo(() => items.filter((i) => i.qty > 0), [items]);
}
```
- **Semantic facts required:**
  - Recognise the identity-comparison contract: `useSyncExternalStore`'s snapshot arguments, and (with a library model) `useSelector`-style hooks whose default equality is `Object.is`.
  - For the snapshot function's body, whether the returned value is a fresh allocation on each call: does the return expression reach an allocation site (`[]`, `{}`, `.map`, `.filter`, `.slice`, spread, `Object.entries`) rather than reading a stored reference?
  - Whether that allocation is *cached across calls* by the callee — a `getItems` that recomputes and memoizes on mutation returns the same reference until the store changes, and must not be flagged. This requires looking inside the store implementation, or treating an opaque function as unknown-but-probably-cached and staying silent to preserve precision.
  - Whether a returned literal is hoisted (`() => EMPTY`) or inline (`() => []`) — the arrow wrapper is irrelevant, the returned reference is what matters.
  - The `subscribe` argument's stability across renders as a companion fact: a per-render-fresh `subscribe` makes React unsubscribe and resubscribe on every render, which is a real but different bug (dropped store events between the two).
  - Whether an equality function is supplied (`useSyncExternalStoreWithSelector`, a selector library's third argument) that makes reference churn harmless — then stay silent.
