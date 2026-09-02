# Wish-list: effects & lifecycle rules for a semantic React analyzer

Domain: `useEffect` / `useLayoutEffect`, cleanup, subscriptions and event registration,
timers, async work in effects, mount/unmount, StrictMode double-invocation, effect
ordering, and effects that touch external systems.

Every "Silent on" below is deliberately chosen to be the shape a naive AST matcher
would wrongly flag.

---

### S-EFF-1: registration-without-matching-teardown
- **What it flags:** an effect acquires a long-lived resource (timer, listener, subscription, observer, socket) on some path through its body, and the cleanup returned on that path does not release it.
- **Why it matters:** the resource outlives the mount. An interval keeps firing after the user navigates away, calling a setter on a disposed component and re-fetching forever; after a few route changes the tab has N intervals and the CPU never idles.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useState } from "react";

export function Clock({ tz }: { tz: string }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    if (tz === "UTC") return () => clearInterval(id);
  }, [tz]);
  return <time>{new Date(now).toISOString()}</time>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";

export function Clock({ tz }: { tz: string }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (tz === "NONE") return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [tz]);
  return <time>{new Date(now).toISOString()}</time>;
}
```
- **Semantic facts required:**
  - which function literal is the *setup* argument of `useEffect` / `useLayoutEffect` / `useInsertionEffect`, and which literal is the value it returns (the cleanup).
  - a classification of call sites into resource-acquiring primitives and their paired release primitives (`setInterval`/`clearInterval`, `setTimeout`/`clearTimeout`, `requestAnimationFrame`/`cancelAnimationFrame`, `addEventListener`/`removeEventListener`, `x.subscribe()`→returned disposer, `new ResizeObserver`/`disconnect`, `new WebSocket`/`close`), including user-defined wrappers resolved interprocedurally.
  - for each acquisition, the set of CFG paths through the setup body on which it executes.
  - for each such path, the abstract value the setup returns on *that* path (a function / `undefined` / other) — the flagged case returns `undefined` on the non-UTC path.
  - whether the cleanup body, on **every** path through it, calls the paired release with an argument that must-alias the acquired handle (must-alias through local bindings and closure capture, not name equality).
  - for handles created in a loop or reassigned, whether the cleanup releases every allocation or only the last value of the binding.
  - for self-rescheduling schedulers (`rAF`/`setTimeout` that re-arms itself inside its own callback), whether the cell the cleanup reads is written by *each* reschedule, or only by the initial call.
  - reachability of the acquiring path: an acquisition under a condition that is provably false is not a leak.

---

### S-EFF-2: teardown-targets-a-different-reference
- **What it flags:** a cleanup calls the correct release primitive, but with a value that is provably a different runtime reference from the one used at acquisition.
- **Why it matters:** `removeEventListener` silently does nothing when the function identity differs. Every mount adds another scroll listener that is never removed; after ten navigations the page runs ten `setState` calls per scroll event and janks, and the old listeners keep closed-over component state alive.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useState } from "react";
import throttle from "lodash/throttle";

export function ScrollSpy() {
  const [y, setY] = useState(0);
  useEffect(() => {
    const onScroll = () => setY(window.scrollY);
    window.addEventListener("scroll", throttle(onScroll, 100));
    return () => window.removeEventListener("scroll", throttle(onScroll, 100));
  }, []);
  return <span>{y}</span>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";
import throttle from "lodash/throttle";

export function ScrollSpy() {
  const [y, setY] = useState(0);
  useEffect(() => {
    const onScroll = throttle(() => setY(window.scrollY), 100);
    window.addEventListener("scroll", onScroll);
    return () => {
      window.removeEventListener("scroll", onScroll);
      onScroll.cancel();
    };
  }, []);
  return <span>{y}</span>;
}
```
- **Semantic facts required:**
  - allocation-site identity for values: whether two expressions evaluate to the *same* runtime reference, not merely to structurally equal or same-typed values.
  - that any call to a function that returns a freshly-allocated closure (`throttle(...)`, `debounce(...)`, `fn.bind(...)`, `useCallback` invoked twice, an arrow literal) yields a distinct reference per evaluation — including for opaque/imported callees, where "unknown callee returning a function" must be treated as fresh.
  - which argument position of each release primitive participates in identity comparison (`removeEventListener` arg 1; the whole handle for a disposer).
  - the *option key* of a listener: `capture` (third argument, boolean or `{capture}`) is part of the identity tuple, so `add(…, {capture:true})` / `remove(…)` is also a mismatch, while a differing `passive`/`once` is not.
  - closure capture semantics: the cleanup captures the *binding*, so the check is "value of that binding at teardown time" — a binding reassigned after acquisition also breaks identity.
  - must-alias reasoning across the setup→cleanup boundary within the same effect run.

---

### S-EFF-3: state-write-after-suspension-without-cancellation
- **What it flags:** an effect starts async work and writes state (or touches an external system) in a continuation reachable after an `await`/`.then`, with no cancellation token re-checked after the last suspension point.
- **Why it matters:** the user opens a detail page and hits back before the request lands. The continuation resolves against a torn-down mount: work is done for a component nobody sees, any subscription opened in that continuation is never cleaned up (nothing will call its cleanup — the effect already tore down), and error handling reports a failure for a screen that no longer exists.
- **Severity intent:** warning.
- **Fires on:**
```tsx
import { useEffect, useState } from "react";

export function User({ id }: { id: string }) {
  const [user, setUser] = useState<{ name: string } | null>(null);
  useEffect(() => {
    (async () => {
      const res = await fetch(`/api/users/${id}`);
      const json = await res.json();
      setUser(json);
    })();
  }, [id]);
  return <h1>{user?.name ?? "…"}</h1>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";

export function User({ id }: { id: string }) {
  const [user, setUser] = useState<{ name: string } | null>(null);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const res = await fetch(`/api/users/${id}`);
      const json = await res.json();
      if (!cancelled) setUser(json);
    })();
    return () => {
      cancelled = true;
    };
  }, [id]);
  return <h1>{user?.name ?? "…"}</h1>;
}
```
- **Semantic facts required:**
  - phase attribution for every function literal: effect setup, cleanup, deferred continuation (async body after an `await`, `.then`/`.catch`/`.finally` callback, timer or event callback registered by the effect).
  - the suspension points on each path, and which state setters / external writes are reachable *after* the last one.
  - identification of a cancellation token: a cell written by this effect's cleanup (or an `AbortSignal` aborted by it) and read on the continuation path.
  - a dominance query: does a read of that token, ordered **after** the final suspension point preceding the write, dominate the write? A guard checked before the last `await` must still fire — the classic "checked `mounted` then awaited again" bug.
  - for the `AbortController` form: whether the signal is passed to the awaited call, whether the cleanup aborts it, and whether the write is on the rejection-free path — an abort that is caught by a `try/catch` which then writes state anyway does **not** count as cancellation.
  - setter→state-slot mapping, so a "write" to a ref or a local is not counted as a state write.
  - which effect run the cleanup belongs to (per-run pairing), because the token must be the one this run's cleanup writes.

---

### S-EFF-4: stale-async-response-overwrites-newer-one
- **What it flags:** an effect with a non-empty dep set writes a state slot from an async continuation, where the only staleness guard is a *mount-scoped* cell (a ref, module flag, instance field) rather than a per-run token — so two overlapping runs can both write the same slot.
- **Why it matters:** the user types "re", then "react". The "re" request is slower and lands last: the input reads "react" and the result list shows results for "re", permanently, until the next keystroke. Nothing crashes and no warning is printed, so this ships.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useRef, useState } from "react";

export function Search({ q }: { q: string }) {
  const [hits, setHits] = useState<string[]>([]);
  const mounted = useRef(true);
  useEffect(() => () => { mounted.current = false; }, []);
  useEffect(() => {
    fetch(`/api/search?q=${q}`)
      .then((r) => r.json())
      .then((d) => { if (mounted.current) setHits(d); });
  }, [q]);
  return <ul>{hits.map((h) => <li key={h}>{h}</li>)}</ul>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";

export function Search({ q }: { q: string }) {
  const [hits, setHits] = useState<string[]>([]);
  useEffect(() => {
    let current = true;
    fetch(`/api/search?q=${q}`)
      .then((r) => r.json())
      .then((d) => { if (current) setHits(d); });
    return () => { current = false; };
  }, [q]);
  return <ul>{hits.map((h) => <li key={h}>{h}</li>)}</ul>;
}
```
- **Semantic facts required:**
  - the effect's dep set is non-empty (or the effect is otherwise re-runnable within one mount), so two runs can be in flight simultaneously.
  - **allocation scope of the guard cell**: allocated once per effect run (a `let` in the setup body, a per-run object) versus once per mount (`useRef`, module-level, closure over a mount-scoped binding). Only the per-run form can discriminate run *k* from run *k+1*; a `mounted` ref stays `true` across dep changes and therefore guards nothing here.
  - whether this effect's cleanup invalidates the in-flight work of the run it tears down (writes the token that the continuation reads, or aborts the signal used by the awaited call).
  - setter→slot mapping, plus the fact that continuations of two distinct runs can write the *same* slot.
  - whether the write is a whole-slot overwrite (order-sensitive) or an order-insensitive merge keyed by the run's own input (`setCache(c => ({...c, [q]: d}))` is safe even without a token) — this distinction is what keeps the rule from firing on correct cache-fill code.
  - suspension-point reachability as in S-EFF-3.
  - whether the request is deduplicated/serialised by the callee (an interprocedural fact: a data-layer hook that cancels its own previous request) — if so, no finding.

---

### S-EFF-5: effect-returns-a-non-callable-cleanup
- **What it flags:** the effect setup returns, on some path, a value that is neither `undefined` nor a function.
- **Why it matters:** React calls whatever you return at teardown. Returning a timer id throws `TypeError: destroy is not a function` when the component unmounts — often during a route transition, so the whole page white-screens at the error boundary. Returning a Promise (an `async` setup) is worse: no crash, but the cleanup inside it never runs, so every subscription leaks silently.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useState } from "react";

export function Poll() {
  const [n, setN] = useState(0);
  useEffect(() => setInterval(() => setN((v) => v + 1), 1000), []);
  return <output>{n}</output>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";
import { subscribe } from "./bus"; // (cb: () => void) => () => void

export function Poll() {
  const [n, setN] = useState(0);
  useEffect(() => subscribe(() => setN((v) => v + 1)), []);
  return <output>{n}</output>;
}
```
- **Semantic facts required:**
  - the abstract return value of the setup body on every path, including implicit returns from concise arrow bodies, and including paths that fall off the end (`undefined`).
  - interprocedural return-shape for the callee at the tail position: `setInterval`/`setTimeout` → number (or a `Timeout` object in Node typings), `subscribe` → function, `new X()` → object, an `async` function → Promise. Unknown callees must be resolved through imports, re-exports and custom hooks before concluding.
  - whether the function literal is `async` or a generator — its return is never a valid cleanup regardless of body.
  - path-sensitivity: mixed returns (a function on one path, an object on another) are a certain crash on the bad path, not a wash.
  - the distinction between "returns `undefined`" (legal: no cleanup — combine with S-EFF-1 to decide whether one was needed) and "returns a non-function value" (certain crash).
  - identification of the setup argument position of the effect hooks specifically, so callbacks passed to `useMemo`, `useCallback`, or a user-defined `useAsync` are not judged by this rule (unless the wrapper forwards to an effect — an interprocedural fact).

---

### S-EFF-6: effect-writes-a-state-slot-it-depends-on
- **What it flags:** an effect unconditionally writes a state slot that also appears in its dep set, with a value that is not `Object.is`-stable across consecutive runs.
- **Why it matters:** render → effect → setState → render → effect → … React aborts with "Maximum update depth exceeded" and unmounts the tree; in production the tab locks up before that, with the fan spinning. This is the single most expensive effect bug a team can ship.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useState } from "react";

export function Chart({ raw }: { raw: number[] }) {
  const [series, setSeries] = useState<number[]>([]);
  useEffect(() => {
    setSeries(raw.map((n) => n * 2));
  }, [raw, series]);
  return <span>{series.length}</span>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";

export function Chart({ raw }: { raw: number[] }) {
  const [max, setMax] = useState(0);
  useEffect(() => {
    setMax(Math.max(...raw, max));
  }, [raw, max]);
  return <span>{max}</span>;
}
```
- **Semantic facts required:**
  - setter→state-slot mapping, including setters that arrive through destructuring, custom hooks, props, or a reducer `dispatch` whose reducer writes the slot.
  - which dep-list entries denote state slots (versus props, refs, or derived expressions), and whether the effect writes one of them on a path that executes on **every** run.
  - freshness of the written value: a literal object/array/function allocation, `map`/`filter`/`slice`/spread result, `new X()` — never `Object.is`-equal to the previous value — versus a primitive or a must-alias of the slot's current value.
  - React's bail-out semantics: a setter call whose argument is `Object.is`-equal to the current slot value schedules no re-render. Convergence therefore requires "the value computed on run *k+1*, given the value written on run *k*, equals that value" — a one-step fixed point of the effect's transfer function. In the silent-on case `max` is monotone and idempotent, so run 2 writes the same number and React bails out.
  - for functional updaters, whether the updater is idempotent on its own output (`f(f(x)) === f(x)`).
  - guard analysis: `if (!ready) setReady(true)` writes its own dep but the guard provably becomes false — the guard variable must be the written slot and the predicate must be monotone.
  - a dep that is a per-render allocation rather than a state slot is S-EFF-7, not this rule; the two must not double-report.

---

### S-EFF-7: effect-dependency-reallocated-every-render
- **What it flags:** a dep-list entry whose value is freshly allocated on every render of the component, so the effect tears down and re-runs on every commit.
- **Why it matters:** the effect is written as "fetch when the query changes" but actually fires on every render — one network request per keystroke elsewhere in the tree, a WebSocket that reconnects every frame, a subscription that churns. If the effect also writes state, it becomes a self-sustaining render loop with a request per iteration; the backend sees a request storm from a single user.
- **Severity intent:** warning (escalate to error when the re-run writes a slot that feeds back into the dep).
- **Fires on:**
```tsx
import { useEffect, useState } from "react";
import { fetchPosts, type Post } from "./api";

export function Feed({ userId }: { userId: string }) {
  const [posts, setPosts] = useState<Post[]>([]);
  const opts = { userId, limit: 20 };
  useEffect(() => {
    let live = true;
    fetchPosts(opts).then((p) => { if (live) setPosts(p); });
    return () => { live = false; };
  }, [opts]);
  return <span>{posts.length}</span>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";
import { fetchPosts, type Post } from "./api";
import { useFeedOptions } from "./useFeedOptions"; // memoises internally on userId

export function Feed({ userId }: { userId: string }) {
  const [posts, setPosts] = useState<Post[]>([]);
  const opts = useFeedOptions(userId);
  useEffect(() => {
    let live = true;
    fetchPosts(opts).then((p) => { if (live) setPosts(p); });
    return () => { live = false; };
  }, [opts]);
  return <span>{posts.length}</span>;
}
```
- **Semantic facts required:**
  - for each dep expression, the allocation site of its value and whether that site is re-executed on every render of this component.
  - a *stability lattice* propagated through the hook graph: `useRef` objects and `useState` setters are always stable; a state value is stable until written; `useMemo`/`useCallback` results are stable iff all of their own deps are stable; a context value is stable iff the provider's `value` expression is stable (cross-component flow); a prop is stable iff every parent passes a stable value (cross-component prop flow, over all call sites).
  - interprocedural stability of custom-hook return values, per return position — `useFeedOptions` must be entered and its internal `useMemo` recognised; a hook returning `[stableFn, freshObject]` is stable in position 0 only.
  - that React compares deps with `Object.is`, so structurally identical fresh objects are unequal — and conversely that a `.length`/primitive projection of a fresh object *is* stable.
  - the observable cost of a re-run: does the body perform I/O, open/close an external registration, restart a timer, or is it idempotent and cheap? This grades the severity and suppresses noise on trivial effects.
  - whether the re-run writes a state slot that transitively reaches the unstable dep, which turns "runs too often" into "never terminates".

---

### S-EFF-8: cascading-effect-chain
- **What it flags:** a chain of effects where each writes a state slot that is a dependency of the next, and every effect in the chain is a pure computation over values already available during render.
- **Why it matters:** one prop change produces four commits instead of one. Between them the browser paints frames in which `rows` is already updated but `totals` still describes the previous data — the user sees the summary flash the old number, then the new one, and a screenshot test catches a different frame every run. On a large table each intermediate commit is a full reconcile.
- **Severity intent:** warning.
- **Fires on:**
```tsx
import { useEffect, useState } from "react";
import { computeTotals, type Row, type Totals } from "./rows";

export function Dashboard({ raw }: { raw: Row[] }) {
  const [rows, setRows] = useState<Row[]>([]);
  const [totals, setTotals] = useState<Totals | null>(null);
  const [label, setLabel] = useState("");
  useEffect(() => { setRows(raw.filter((r) => r.visible)); }, [raw]);
  useEffect(() => { setTotals(computeTotals(rows)); }, [rows]);
  useEffect(() => { setLabel(`${rows.length} rows, ${totals?.sum ?? 0}`); }, [totals]);
  return <footer>{label}</footer>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";
import { openChannel, type Row } from "./rows";

export function Dashboard({ boardId }: { boardId: string }) {
  const [rows, setRows] = useState<Row[]>([]);
  const [presence, setPresence] = useState(0);
  useEffect(() => {
    let live = true;
    fetch(`/api/boards/${boardId}/rows`)
      .then((r) => r.json())
      .then((d) => { if (live) setRows(d); });
    return () => { live = false; };
  }, [boardId]);
  useEffect(() => {
    if (rows.length === 0) return;
    const ch = openChannel(rows.map((r) => r.id), setPresence);
    return () => ch.close();
  }, [rows]);
  return <footer>{rows.length} / {presence}</footer>;
}
```
- **Semantic facts required:**
  - an effect-dependency graph over the component: an edge A→B whenever A writes, on some path, a state slot that appears in B's dep set or is read by B in a way that changes its behaviour. Longest path length = number of extra commits, and is the number to report.
  - for each effect, whether its body is *pure with respect to the outside world*: no I/O, no external registration, no read of a mutable external source (`Date.now()`, `window.*`, `ref.current`, an awaited result). Only pure state-only effects are removable; the silent-on chain is driven by a network read and a socket registration and is therefore genuine synchronisation.
  - whether every input the effect reads is available during render: in render scope, and not the product of a suspension point. If so the computation belongs in the render body / `useMemo`.
  - setter→slot mapping and per-effect write sets, path-sensitive (an effect that writes only under a rare condition is a weaker edge).
  - observability of intermediate commits: does the returned JSX read slots that are updated at different links of the chain (so a frame shows a mixed state), or is the whole chain hidden behind one loading gate?
  - phase attribution of the writes: an event-handler-initiated chain is a different cost model (one user gesture) and must not be reported here.

---

### S-EFF-9: layout-measurement-and-paint-write-in-a-passive-effect
- **What it flags:** a `useEffect` (passive, post-paint) that performs a layout-forcing DOM read and then writes a state slot that flows into the visual geometry of this component's output.
- **Why it matters:** the browser paints the pre-measurement frame first. The tooltip appears in the top-left corner for one frame and jumps to the anchor; on a slow device it is two or three frames and reads as a visible flicker. Users report "the menu flashes in the wrong place".
- **Severity intent:** warning.
- **Fires on:**
```tsx
import { useEffect, useRef, useState } from "react";

export function Tooltip({ anchor }: { anchor: React.RefObject<HTMLElement | null> }) {
  const [top, setTop] = useState(0);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const r = anchor.current!.getBoundingClientRect();
    setTop(r.bottom + 8);
  }, [anchor]);
  return <div ref={box} style={{ position: "fixed", top }} role="tooltip" />;
}
```
- **Silent on:**
```tsx
import { useEffect, useRef, useState } from "react";

export function Tooltip({ anchor }: { anchor: React.RefObject<HTMLElement | null> }) {
  const [width, setWidth] = useState(0);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const r = anchor.current!.getBoundingClientRect();
    setWidth(r.width);
  }, [anchor]);
  return (
    <div ref={box} style={{ position: "fixed" }} onClick={() => report(width)} role="tooltip" />
  );
}
```
- **Semantic facts required:**
  - a classification of layout-forcing DOM reads (`getBoundingClientRect`, `offsetWidth/Height/Top/Left`, `client*`, `scroll*`, `getComputedStyle`, `Range.getClientRects`, `element.checkVisibility`) performed on a node reached through a ref or a DOM query.
  - which hook the enclosing literal is the setup of: `useEffect` runs after paint, `useLayoutEffect` before paint, `useInsertionEffect` before layout effects. Only the passive one flickers.
  - reachability from the layout read to a state setter with no suspension point in between (a measurement written from a `ResizeObserver` callback or after an `await` has different timing and needs a separate judgement).
  - dataflow from the written slot into the component's rendered output, and specifically into a **visually positional sink**: a `style` property, `className`, `transform`, `width`/`height`, `hidden`, or a condition that gates rendering a positioned node. In the silent-on case the slot reaches only an event-handler argument, so no frame is wrong.
  - whether the affected node is rendered in the first commit at all (a value consumed only after a user interaction never flickers).
  - whether the initial state value provably equals the first measurement (then no visual change occurs).
  - the inverse check, worth reporting separately at info level: a `useLayoutEffect` that performs no layout read and no synchronous DOM write is blocking paint for nothing.

---

### S-EFF-10: non-idempotent-effect-under-remount
- **What it flags:** an effect performs an *accumulating* external operation (create, join, append, increment, emit) whose inverse is not performed by its cleanup, in a component whose effects can run more than once per logical mount.
- **Why it matters:** StrictMode's dev double-invoke posts the join twice, so occupancy reads 2 for a single user and the "room full" check locks everyone out. In production the same defect fires on every Fast Refresh, every back-navigation to a cached route, and every dep change: the server-side counter drifts upward and never comes back down.
- **Severity intent:** warning.
- **Fires on:**
```tsx
import { useEffect } from "react";

export function Room({ id }: { id: string }) {
  useEffect(() => {
    fetch(`/api/rooms/${id}/join`, { method: "POST" });
  }, [id]);
  return <section>Room {id}</section>;
}
```
- **Silent on:**
```tsx
import { useEffect } from "react";

export function Room({ id }: { id: string }) {
  useEffect(() => {
    const prev = document.title;
    document.title = `Room ${id}`;
    return () => { document.title = prev; };
  }, [id]);
  return <section>Room {id}</section>;
}
```
- **Semantic facts required:**
  - the set of ways this effect can run more than once for one logical mount: StrictMode dev double-invoke (setup → cleanup → setup), dep-triggered re-runs, Fast Refresh, remount on `key` change, offscreen/`<Activity>` restore, route revisit.
  - a classification of each external operation as **idempotent by construction** (an assignment or a PUT whose value does not read the current state), **accumulating** (POST-create, append, increment, enqueue, emit, `push` onto a module array), or **unknown** — resolved interprocedurally through API-client wrappers, and from the HTTP method / mutation name where inferable.
  - whether the cleanup performs the inverse of every accumulating operation reachable on the setup path, under a known inverse relation (join↔leave, subscribe↔unsubscribe, acquire↔release, create↔delete, push↔splice, emit-start↔emit-end).
  - must-alias between the identity argument at acquisition and at release — including the case where the identity is only known from the response (`const { id } = await post(...)`), which forces the cleanup to read a cell written by the continuation, with the S-EFF-12 timing question attached.
  - writes from an effect body to state that outlives the mount (module-level bindings, a singleton's fields, a cache): these accumulate across remounts and are the strongest signal.
  - detection of an ad-hoc once-guard (a module `Set`, a `useRef` boolean, `if (didRun.current) return`): it suppresses the double-fire claim, so the rule must not fire, but it deserves its own lower-severity finding because it does not survive a real remount.

---

### S-EFF-11: escaping-callback-reads-a-stale-render-value
- **What it flags:** an effect hands a closure to a long-lived external registration, and that closure reads a render-scope value which can provably change before the closure is invoked — while the effect does not re-register on that value.
- **Why it matters:** the autosave timer captures `text` from the mount render, so every five seconds it writes the empty string over the user's document. The user types for ten minutes, reloads, and the work is gone. Nothing throws, and the deps list looks deliberate.
- **Severity intent:** error (when the stale value flows into an external write; warning when it only feeds a state write that is later corrected).
- **Fires on:**
```tsx
import { useEffect, useState } from "react";
import { save } from "./api";

export function Autosave({ docId }: { docId: string }) {
  const [text, setText] = useState("");
  useEffect(() => {
    const id = setInterval(() => { save(docId, text); }, 5000);
    return () => clearInterval(id);
  }, [docId]);
  return <textarea value={text} onChange={(e) => setText(e.target.value)} />;
}
```
- **Silent on:**
```tsx
import { useEffect, useRef, useState } from "react";
import { save } from "./api";

export function Autosave({ docId }: { docId: string }) {
  const [text, setText] = useState("");
  const latest = useRef(text);
  latest.current = text;
  useEffect(() => {
    const id = setInterval(() => { save(docId, latest.current); }, 5000);
    return () => clearInterval(id);
  }, [docId]);
  return <textarea value={text} onChange={(e) => setText(e.target.value)} />;
}
```
- **Semantic facts required:**
  - which function literals **escape** the effect run: stored into a timer, a listener, a subscription, an observer, a promise continuation, or any registration whose invocation is scheduled after the setup returns.
  - the phase in which the escaping callback executes (timer / event / microtask) and whether that phase can be entered *after* a later render of the same mount — a callback that provably runs only synchronously inside the setup cannot be stale.
  - per read inside the callback: is the value **captured** (a `const` binding of a render value, frozen at capture) or read through a **mount-stable cell** (`ref.current`, module state, a getter) that is current at invocation time?
  - for a mount-stable cell, whether its writer dominates the invocation: a ref assigned during render or in a separate always-running effect is current; a ref assigned only inside *this* effect's setup is exactly as stale as a capture.
  - whether the captured slot can change at all after the effect run: is there any reachable writer (a setter called from an event handler, another effect, a parent re-rendering the prop)? A value that provably never changes after mount is not stale, which is what keeps this rule from degenerating into exhaustive-deps.
  - use-sensitivity of the read: `setX(prev => prev + 1)` does not read the stale value; `setX(stale + 1)` does; `save(docId, stale)` does and is externally observable.
  - consequence classification, to set severity: stale value → external write (data loss / wrong request), versus stale value → state write that a later correct run overwrites.

---

### S-EFF-12: teardown-key-differs-from-setup-key
- **What it flags:** a cleanup releases a resource using a value read from a mutable cell that is written *outside* the effect (during render, or by another phase), so at teardown time the cell already holds the next generation's value.
- **Why it matters:** on a room change React renders with the new id, then runs the old effect's cleanup — which reads the ref and leaves the room the user just entered, while the old room stays joined. The user appears in the wrong room's presence list, receives its messages, and receives none from the room on screen. Unmount happens to work, so this only reproduces on navigation between rooms.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useRef } from "react";
import { socket } from "./socket";

export function Presence({ roomId }: { roomId: string }) {
  const roomRef = useRef(roomId);
  roomRef.current = roomId;
  useEffect(() => {
    socket.join(roomId);
    return () => socket.leave(roomRef.current);
  }, [roomId]);
  return null;
}
```
- **Silent on:**
```tsx
import { useEffect, useRef } from "react";
import { socket } from "./socket";

export function Presence({ roomId }: { roomId: string }) {
  const joinedRef = useRef<string | null>(null);
  useEffect(() => {
    joinedRef.current = roomId;
    socket.join(roomId);
    return () => {
      if (joinedRef.current) socket.leave(joinedRef.current);
    };
  }, [roomId]);
  return null;
}
```
- **Semantic facts required:**
  - React's commit schedule as an ordering fact: render(*n+1*) completes → cleanups of the changing effects run → setups run. Therefore any cell written **during render** (or by a hook that has already run for generation *n+1*) is at generation *n+1* when a generation-*n* cleanup executes, while a cell written **inside the setup** is still at generation *n*.
  - for every read in a cleanup body, the set of write sites that reach it, and the phase of each write (render body, effect setup, event handler, another effect's setup or cleanup, an async continuation).
  - acquisition/release pairing **per effect run**, and which argument of each is the identity key (join/leave on `roomId`, subscribe/unsubscribe on a topic, observe/unobserve on a node).
  - a must-alias query between the value passed at acquisition on run *n* and the value read at release on run *n*'s cleanup — name equality is not enough, and `props.x` read in the cleanup is *not* the setup's `x` when the effect re-runs.
  - whether the acquisition value is a per-run local (always safe), a captured render value (safe, frozen at the right generation), or a shared mutable cell (needs the write-phase check above).
  - the async variant of the same fact pattern: when the handle only exists after an `await`, the cleanup can run *before* the cell is written and release nothing — a certain leak that the same machinery detects (release reads a cell whose only writer is a continuation not ordered before the cleanup).

---

### S-EFF-13: effect-dereferences-a-ref-that-can-be-null
- **What it flags:** an effect dereferences a DOM ref on a path whose condition does not imply that some rendered node in the committed subtree attached that ref.
- **Why it matters:** `Cannot read properties of null (reading 'focus')` at mount, thrown from a commit-phase callback — React unwinds to the nearest error boundary, so the whole panel disappears instead of just failing to focus. It only reproduces when the dialog opens closed-first, which is exactly the path QA does not take.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useRef } from "react";

export function Editor({ open }: { open: boolean }) {
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    inputRef.current!.focus();
  }, []);
  return open ? <input ref={inputRef} /> : <p>closed</p>;
}
```
- **Silent on:**
```tsx
import { forwardRef, useEffect, useRef } from "react";

const Field = forwardRef<HTMLInputElement>((props, ref) => <input ref={ref} {...props} />);

export function Editor() {
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    inputRef.current!.focus();
  }, []);
  return <Field ref={inputRef} />;
}
```
- **Semantic facts required:**
  - which host elements a given ref object is attached to, following the ref value through props into child components (`forwardRef`, `ref`-as-prop in React 19, a ref forwarded two levels down, a ref stored in an array or a `Map`) — cross-component flow is mandatory, otherwise the silent-on case is wrongly flagged.
  - the path condition under which each attaching JSX node is produced, expressed over the same render-scope values the effect body can test, so the two can be compared.
  - commit ordering: all refs of the committed tree are attached before any effect of that commit runs, and child effects run before parent effects — a ref attached anywhere in the committed subtree is non-null in the parent's effect.
  - the implication check: for each deref, does the path condition reaching it imply the disjunction of the attachment conditions? `if (!open) return;` before the deref makes it imply; a `[]` dep list with a conditional render does not.
  - ref *detachment*: when the attaching node stops being rendered, React writes `null` before the next commit's effects, so an effect that re-runs on a dep change can observe null even though the mount run saw a node.
  - provenance of the cell: a React-managed host ref (nullable by the above rules) versus an application-written `ref.current = x` (nullability determined by the writing phase and its ordering against the read).
  - whether the deref happens in the setup body (conclusion: crash at commit) or inside a deferred callback registered by the effect (weaker conclusion: the node may have been unmounted by then — report separately, not as a certain crash).

---

### S-EFF-14: sibling-effect-cleanup-uses-a-torn-down-resource
- **What it flags:** an effect's cleanup reads a shared resource that an *earlier-declared* effect's cleanup has already invalidated in the same commit.
- **Why it matters:** React runs cleanups in hook declaration order. On unmount the socket is closed and the ref nulled first, then the heartbeat cleanup tries a farewell `send` and throws — during unmount, which React does not recover from gracefully, so a route change leaves a broken screen. Reordering the two `useEffect` calls fixes it and nothing else changes, which is why this is impossible to find by reading either effect on its own.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useRef } from "react";

export function Live({ url }: { url: string }) {
  const sockRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    const s = new WebSocket(url);
    sockRef.current = s;
    return () => { s.close(); sockRef.current = null; };
  }, [url]);

  useEffect(() => {
    const id = setInterval(() => sockRef.current?.send("ping"), 5000);
    return () => { clearInterval(id); sockRef.current!.send("bye"); };
  }, [url]);

  return null;
}
```
- **Silent on:**
```tsx
import { useEffect, useRef } from "react";

export function Live({ url }: { url: string }) {
  const sockRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    const id = setInterval(() => sockRef.current?.send("ping"), 5000);
    return () => { clearInterval(id); sockRef.current!.send("bye"); };
  }, [url]);

  useEffect(() => {
    const s = new WebSocket(url);
    sockRef.current = s;
    return () => { s.close(); sockRef.current = null; };
  }, [url]);

  return null;
}
```
- **Semantic facts required:**
  - hook declaration order within the component, and React's schedule: within a commit, cleanups run in declaration order, then setups in declaration order; across the tree, a child's effects run before its parent's.
  - membership in the same teardown batch: only effects whose deps changed are torn down in a given commit, so the ordering claim must be conditioned on both effects tearing down together (here both key on `url`; on unmount, all of them do).
  - a resource lifetime per shared cell: the writes that *publish* a resource (`sockRef.current = s`) and the operations that *invalidate* it (`close()`, `abort()`, `unsubscribe()`, `disconnect()`, assignment of `null`).
  - for each read of that cell inside a cleanup, whether an invalidating operation is ordered before it under the cleanup schedule.
  - must-alias between the object that was invalidated and the object reached through the cell at the later read.
  - state-machine facts about known external objects: a `WebSocket` after `close()` throws on `send`, an aborted `AbortController` fails subsequent fetches, a disconnected observer silently no-ops. Invalidation is not only nulling, so a nullish-check (`?.`) does not necessarily make the read safe.
  - the symmetric case, same machinery: a *setup* that reads a cell published only by a later-declared effect's setup observes the stale/absent value on the first commit.

---

### S-EFF-15: subscribe-without-reading-the-current-value
- **What it flags:** an effect subscribes to an external mutable source whose current value was read during render, without re-reading that source after subscribing.
- **Why it matters:** between the render that snapshotted the value and the passive effect that subscribes, React can yield (concurrent rendering, a suspended sibling, a slow commit) — and StrictMode's unsubscribe/resubscribe reopens the window on purpose. Any change in that gap is lost forever: the app renders the light theme while the OS is in dark mode, and stays wrong until the user toggles the OS setting twice.
- **Severity intent:** warning.
- **Fires on:**
```tsx
import { useEffect, useState } from "react";

const mq = window.matchMedia("(prefers-color-scheme: dark)");

export function Theme() {
  const [dark, setDark] = useState(() => mq.matches);
  useEffect(() => {
    const onChange = (e: MediaQueryListEvent) => setDark(e.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return <div className={dark ? "dark" : "light"} />;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from "react";

const mq = window.matchMedia("(prefers-color-scheme: dark)");

export function Theme() {
  const [dark, setDark] = useState(() => mq.matches);
  useEffect(() => {
    const onChange = () => setDark(mq.matches);
    mq.addEventListener("change", onChange);
    onChange();
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return <div className={dark ? "dark" : "light"} />;
}
```
- **Semantic facts required:**
  - identification of an **external mutable source**: an object outside React's ownership that exposes both a current-value read (`.matches`, `.getState()`, `.value`, `navigator.onLine`, `localStorage.getItem`, `document.hidden`, a store instance) and a change-notification registration.
  - that the render-time read (including inside a `useState` initializer, a `useMemo`, or a helper called during render) and the effect's subscription target **must-alias** the same source object.
  - slot correspondence: the state slot initialised from the render-time read and the slot written by the notification callback are the same slot.
  - on every path of the setup body, whether a current-value read of that source reaches a write of that slot **after** the subscription call. Before the subscription is not enough — the gap simply moves.
  - re-subscription paths: if the effect's deps can change, each re-subscription reopens the window, so the re-read must be on the re-run path too, not gated by a first-mount flag.
  - suppression when the value is consumed through `useSyncExternalStore` (or a library hook that resolves to it interprocedurally), which closes the window structurally.
  - suppression when the source is provably immutable between render and the effect (a frozen constant, a value snapshotted before render begins).
  - phase facts: the read must be attributed to the render phase and the subscription to the effect phase — the same code inside an event handler has no window.
