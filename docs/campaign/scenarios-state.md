# Wish-list: state & data-flow rules for a semantic React analyzer

Domain: `useState` / `useReducer`, derived state, props-into-state, update batching,
functional vs value updaters, mutation, redundant and unobservable writes, state that
should be a ref, state machines, state lifted across components.

---

### S-STATE-1: stale-snapshot-update-after-suspension
- **What it flags:** a setter call whose argument reads the slot's render-time binding, executed after a suspension point (timer, promise continuation, subscription callback) where that binding is not refreshed between invocations.
- **Why it matters:** the callback keeps the value captured at subscription time, so the counter/accumulator advances exactly once and then freezes. The user clicks "start" and watches a ticker stuck at 1.
- **Severity intent:** error (the interval case is certain), warning when the refresh path is unprovable.
- **Fires on:**
```tsx
import { useEffect, useState } from 'react';

function Ticker() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setCount(count + 1), 1000);
    return () => clearInterval(id);
  }, []);
  return <p>{count}</p>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from 'react';

function Ticker() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    const id = setTimeout(() => setCount(count + 1), 1000);
    return () => clearTimeout(id);
  }, [count]);
  return <p>{count}</p>;
}
```
- **Semantic facts required:**
  - Which state slot each setter identifier writes (setter identity resolved through destructuring, aliasing, and being passed as an argument).
  - Whether the setter's argument expression transitively reads the *slot binding of the enclosing render* (as opposed to a functional updater parameter, a ref read, or a value passed in as an argument).
  - The phase and the *re-arm schedule* of the enclosing function literal: does the closure get recreated whenever the read slot changes? This requires the deps of the owning effect and the fact that the effect's cleanup provably cancels the pending callback before the next run.
  - Whether the callback can execute more than once per closure instance (`setInterval`, event subscription, `.then` on a long-lived stream) vs at most once (`setTimeout` cancelled by a cleanup keyed on the same slot).
  - Reachability from a suspension point: the setter call is on a continuation edge (post-`await`, `.then` body, timer body) rather than the straight-line effect body.

---

### S-STATE-2: lost-update-same-slot-in-batch
- **What it flags:** two or more value-form writes to the same slot reachable on one path through a single batch region, where the later write's value depends on the pre-batch snapshot.
- **Why it matters:** the writes collapse: "add two" adds one. Quantities, cursor positions and undo depths silently drift out of sync with what the user did.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useState } from 'react';

function Cart() {
  const [qty, setQty] = useState(0);
  const addPair = () => {
    setQty(qty + 1);
    setQty(qty + 1);
  };
  return <button onClick={addPair}>{qty}</button>;
}
```
- **Silent on:**
```tsx
import { useState } from 'react';

function Cart() {
  const [qty, setQty] = useState(0);
  const step = (up: boolean) => {
    if (up) setQty(qty + 1);
    else setQty(qty - 1);
  };
  return <button onClick={() => step(true)}>{qty}</button>;
}
```
- **Semantic facts required:**
  - Slot identity for each setter call, including calls made through a helper function reached from the handler (interprocedural, with the setter arriving as a parameter or a captured binding).
  - Path feasibility: are both writes reachable on a *single* execution path? Mutually exclusive branches must not be joined into a false "two writes" fact.
  - Batch-region partitioning: both writes must sit in the same synchronous region, with no `await`, `.then` boundary, timer boundary or `flushSync` between them.
  - Data dependence of the second write's argument on the slot's *render-time* binding (value form) rather than on the updater parameter (functional form).
  - Whether the second value is a function of the first written value; if the second write is a constant overwrite, this is S-STATE-4, not a lost update.

---

### S-STATE-3: stale-read-after-write-in-batch
- **What it flags:** a read of a state slot that is dominated by a write to the same slot inside one batch region, where the read is used for anything other than the previous value on purpose.
- **Why it matters:** the code below the setter still sees the old value, so the request body, the analytics event or the log line carries the value from before the interaction. The screen looks right and the data sent to the server is wrong, which is the worst failure mode to debug.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useState } from 'react';

function Search() {
  const [query, setQuery] = useState('');
  const [log, setLog] = useState<string[]>([]);
  const onPick = (next: string) => {
    setQuery(next);
    setLog(l => [...l, `searched ${query}`]);
  };
  return <button onClick={() => onPick('shoes')}>{query} {log.length}</button>;
}
```
- **Silent on:**
```tsx
import { useState } from 'react';

function Search() {
  const [query, setQuery] = useState('');
  const [log, setLog] = useState<string[]>([]);
  const onPick = (next: string) => {
    setLog(l => [...l, `searched ${query}`]);
    setQuery(next);
  };
  return <button onClick={() => onPick('shoes')}>{query} {log.length}</button>;
}
```
- **Semantic facts required:**
  - Write/read ordering within the CFG: the read must be *dominated* by the write, on the same path, in the same batch region. Program order matters, so a purely "this handler mentions both" fact is useless.
  - Which reads of the binding count: reads inside a functional updater body for a *different* slot still observe the stale binding and must count; reads of a local that was also passed to the setter must not.
  - The identity relation between the written value and the read: if the written expression and the read expression provably evaluate to the same value, the read is harmless.
  - Batch-region boundaries again (a read after an `await` is stale for a different reason and belongs to S-STATE-1).
  - Escape of the read value: does it flow into a call argument, a JSX attribute, or another setter (a real observation), or is it dead?

---

### S-STATE-4: unobservable-intermediate-state
- **What it flags:** a write whose value is overwritten by a later write to the same slot in the same batch region, with no suspension point and no read of the slot in between.
- **Why it matters:** the intermediate state never renders. The "Saving…" label, the disabled button and the optimistic row are dead code, so the user gets no feedback during a slow synchronous step, or the developer believes a state machine passes through a phase it never enters.
- **Severity intent:** warning.
- **Fires on:**
```tsx
import { useState } from 'react';

function Saver() {
  const [status, setStatus] = useState<'idle' | 'saving' | 'done'>('idle');
  const onSave = () => {
    setStatus('saving');
    localStorage.setItem('doc', 'payload');
    setStatus('done');
  };
  return <button onClick={onSave}>{status}</button>;
}
```
- **Silent on:**
```tsx
import { useState } from 'react';

function Saver() {
  const [status, setStatus] = useState<'idle' | 'saving' | 'done'>('idle');
  const onSave = async () => {
    setStatus('saving');
    await fetch('/api/save', { method: 'POST' });
    setStatus('done');
  };
  return <button onClick={onSave}>{status}</button>;
}
```
- **Semantic facts required:**
  - Post-domination: every path from write A reaches write B before leaving the batch region.
  - Batch-region membership for both writes, with suspension points (`await`, `.then`, timer scheduling, `flushSync`, an event-loop yield inside a called function) splitting regions. This must be interprocedural: a synchronous helper does not split, an `await`ing helper does.
  - Absence of any read of the slot between A and B, including reads inside functions called in between.
  - Whether the second value is control-dependent on something computed between A and B (still unobservable, still worth flagging) vs on a suspension result (observable).
  - `Object.is` comparison between the two written values: writing the same value twice is a different (weaker) finding.

---

### S-STATE-5: effect-synced-derived-state
- **What it flags:** a state slot whose every write computes a pure function of values already available during render (props, other state, constants), where the writes execute in effect phase.
- **Why it matters:** the first paint shows the placeholder value, then a second render replaces it, so the user sees a flash of empty or wrong content. Every upstream change costs two renders plus a commit, and anything downstream that depends on the slot fires a third.
- **Severity intent:** warning.
- **Fires on:**
```tsx
import { useEffect, useState } from 'react';

function Name({ first, last }: { first: string; last: string }) {
  const [full, setFull] = useState('');
  useEffect(() => {
    setFull(`${first} ${last}`);
  }, [first, last]);
  return <h1>{full}</h1>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from 'react';

function Name({ first, last }: { first: string; last: string }) {
  const [full, setFull] = useState('');
  useEffect(() => {
    setFull(`${first} ${last}`);
  }, [first, last]);
  return <input value={full} onChange={e => setFull(e.target.value)} />;
}
```
- **Semantic facts required:**
  - The complete set of writers of the slot, across the whole component, including writers reached through helper functions and writers passed as props to children.
  - The phase of each writer (render / layout effect / passive effect / event handler / timer / promise continuation). A single event-handler writer turns "derived state" into "seeded editable state" and must suppress the finding.
  - For each effect writer: whether the written expression is a pure function of render-scope values, meaning no read of an external mutable source (fetch result, `Date.now`, DOM measurement, ref), no side effect on the way.
  - Whether the effect's deps cover exactly the free render-scope reads of the written expression (if the effect also depends on something non-render-derivable, it is not pure derivation).
  - Whether the slot's initial value differs from the derived value on first render (the flash), and whether the slot is read during render at all.

---

### S-STATE-6: props-into-state-without-resync
- **What it flags:** a slot initialised from a prop, where the call site passes a value that changes across renders of the same mounted instance and nothing resets the slot when it changes.
- **Why it matters:** the parent switches records and the child keeps showing (and submitting) the previous record's field. Editing then saves the old value into the new record.
- **Severity intent:** warning (error when the slot feeds a submit path).
- **Fires on:**
```tsx
import { useState } from 'react';

type User = { id: string; name: string };

function Profile({ user }: { user: User }) {
  const [name, setName] = useState(user.name);
  return <input value={name} onChange={e => setName(e.target.value)} />;
}

function Page({ users }: { users: User[] }) {
  const [i, setI] = useState(0);
  return (
    <>
      <button onClick={() => setI(i + 1)}>next</button>
      <Profile user={users[i]} />
    </>
  );
}
```
- **Silent on:**
```tsx
import { useState } from 'react';

type User = { id: string; name: string };

function Profile({ user }: { user: User }) {
  const [name, setName] = useState(user.name);
  return <input value={name} onChange={e => setName(e.target.value)} />;
}

function Page({ users }: { users: User[] }) {
  const [i, setI] = useState(0);
  return (
    <>
      <button onClick={() => setI(i + 1)}>next</button>
      <Profile key={users[i].id} user={users[i]} />
    </>
  );
}
```
- **Semantic facts required:**
  - Data dependence of the `useState` initial-value expression on a prop (through destructuring, member access, and helper calls).
  - Cross-component prop flow: for every call site of the component, the expression passed for that prop, and whether it is invariant across re-renders of the same instance (a constant, a mount-only value) or varies.
  - Whether the element at that call site carries a `key` that is a function of the same prop (or of something that changes whenever the prop changes), which makes the instance remount and the initial value re-evaluate.
  - Whether the child contains a resync writer: a render-phase adjust comparing a previous-prop slot, or an effect writing the slot with the prop in its deps.
  - Mount-vs-update distinction for the initialiser: it evaluates on every render but only the first result reaches the slot.

---

### S-STATE-7: state-never-read-during-render
- **What it flags:** a slot that is written outside render but never read by anything that influences the rendered output or the re-run of an effect.
- **Why it matters:** every write re-renders the component and its whole non-memoised subtree for nothing. On a high-frequency writer (scroll position, mouse coordinates, a timer id) this is a visible frame-rate drop, and the value read back from the closure is a stale-closure hazard on top.
- **Severity intent:** warning.
- **Fires on:**
```tsx
import { useState } from 'react';

function Poller() {
  const [timerId, setTimerId] = useState<number | null>(null);
  const start = () => setTimerId(window.setInterval(() => {}, 1000));
  const stop = () => {
    if (timerId !== null) window.clearInterval(timerId);
  };
  return (
    <>
      <button onClick={start}>go</button>
      <button onClick={stop}>stop</button>
    </>
  );
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from 'react';

function Poller() {
  const [delay, setDelay] = useState<number | null>(null);
  useEffect(() => {
    if (delay === null) return;
    const id = window.setInterval(() => {}, delay);
    return () => window.clearInterval(id);
  }, [delay]);
  return <button onClick={() => setDelay(1000)}>go</button>;
}
```
- **Semantic facts required:**
  - The transitive read set of the slot: direct reads, reads through locals derived from it, reads inside functions called during render, and reads inside JSX attributes and children.
  - Whether any read is *render-reachable*: it flows into returned JSX, into a child's props, into a `useMemo`/`useCallback` result that is itself render-reachable, or into a hook argument that changes rendering.
  - Whether the slot (or a value derived from it) appears in a dependency array, since the re-render is then the mechanism that re-runs the effect and is load-bearing.
  - Whether reads occur only in non-render phases (handlers, effect bodies, timers), which is the "should be a ref" signature.
  - Write frequency evidence: is a writer inside a high-frequency source (scroll/mousemove/timer/animation frame)? This upgrades the severity rather than deciding the rule.

---

### S-STATE-8: mutate-then-set-same-reference
- **What it flags:** an in-place mutation of a value reachable from a state slot, followed by a write of that same reference back into the slot.
- **Why it matters:** `Object.is(next, prev)` holds, so React bails out and nothing re-renders. The item the user just added appears only when an unrelated interaction re-renders the component, and then several appear at once. In StrictMode the double-invoked handler can also double-apply the mutation.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useState } from 'react';

function Todos() {
  const [items, setItems] = useState<string[]>([]);
  const add = (t: string) => {
    items.push(t);
    setItems(items);
  };
  return (
    <>
      <button onClick={() => add('a')}>add</button>
      <ul>{items.map(i => <li key={i}>{i}</li>)}</ul>
    </>
  );
}
```
- **Silent on:**
```tsx
import { useState } from 'react';

const withItem = (xs: string[], x: string) => [...xs, x];

function Todos() {
  const [items, setItems] = useState<string[]>([]);
  const add = (t: string) => {
    const next = withItem(items, t);
    next.push('!' + t);
    setItems(next);
  };
  return (
    <>
      <button onClick={() => add('a')}>add</button>
      <ul>{items.map(i => <li key={i}>{i}</li>)}</ul>
    </>
  );
}
```
- **Semantic facts required:**
  - Allocation-site identity for the value passed to the setter, interprocedurally: does it come from the state slot's current value, or from a fresh allocation (spread, `slice`, `map`, `Object.assign({}, …)`, a helper that returns a fresh object)?
  - Points-to / aliasing: which locals may alias the object reachable from the slot, including through parameters and returned values.
  - Mutation effects of calls: `push`, `sort`, `splice`, `reverse`, property assignment, `delete`, and user functions summarised by the writes they perform on their parameters.
  - Reachability from the slot root: mutating `state.a.b` counts even if the setter receives a shallow copy of the root (a shallow spread does not detach nested objects) — which makes the "fresh top-level object, mutated nested member" case a separate true positive.
  - The `Object.is` verdict React will compute between the incoming value and the current slot value.

---

### S-STATE-9: module-scope-value-as-mutable-initial-state
- **What it flags:** a slot initialised with a value allocated once at module scope, where some path mutates a location reachable from that value.
- **Why it matters:** the "default" is shared by every instance and every mount, so a second component (or the same component after a route change) starts with the first one's data. It survives navigation, does not reproduce on a fresh page load, and looks like a caching bug in the server.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useState } from 'react';

const DEFAULT_FILTERS = { tags: [] as string[] };

function Filters() {
  const [filters, setFilters] = useState(DEFAULT_FILTERS);
  const addTag = (t: string) => {
    filters.tags.push(t);
    setFilters({ ...filters });
  };
  return <button onClick={() => addTag('new')}>{filters.tags.length}</button>;
}
```
- **Silent on:**
```tsx
import { useState } from 'react';

const DEFAULT_FILTERS = { tags: [] as string[] };

function Filters() {
  const [filters, setFilters] = useState(DEFAULT_FILTERS);
  const addTag = (t: string) =>
    setFilters(f => ({ ...f, tags: [...f.tags, t] }));
  return <button onClick={() => addTag('new')}>{filters.tags.length}</button>;
}
```
- **Semantic facts required:**
  - Allocation site of the initial value: module scope (evaluated once per module, shared by all instances) vs per-render allocation vs lazy initialiser body.
  - Whether the allocation escapes into the slot without copying (a spread at the `useState` call site breaks the sharing).
  - Whether any reachable path mutates a location reachable from that allocation — the same mutation/aliasing machinery as S-STATE-8, but rooted at the module allocation and including nested members not detached by a shallow spread.
  - Multi-instance and remount reasoning: the finding requires the allocation to outlive the component instance, so the same object literal written inline in the component body is fine.
  - Writers in every phase count, including writers in effects and in callbacks handed to children.

---

### S-STATE-10: reducer-returns-mutated-input
- **What it flags:** a `useReducer` reducer that returns its `state` argument (or a value aliasing it) on a path that mutated something reachable from `state`.
- **Why it matters:** React compares the returned reference with the previous one, sees no change and skips the re-render, so dispatching does nothing visible. Under StrictMode the reducer is invoked twice, so the mutation is applied twice and the eventual re-render shows doubled data.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useReducer } from 'react';

type S = { items: string[] };
type A = { type: 'add'; item: string };

function reducer(state: S, action: A): S {
  switch (action.type) {
    case 'add':
      state.items.push(action.item);
      return state;
    default:
      return state;
  }
}

function List() {
  const [state, dispatch] = useReducer(reducer, { items: [] });
  return <button onClick={() => dispatch({ type: 'add', item: 'a' })}>{state.items.length}</button>;
}
```
- **Silent on:**
```tsx
import { useReducer } from 'react';

type S = { items: string[] };
type A = { type: 'add'; item: string };

function reducer(state: S, action: A): S {
  switch (action.type) {
    case 'add':
      if (state.items.includes(action.item)) return state;
      return { ...state, items: [...state.items, action.item] };
    default:
      return state;
  }
}

function List() {
  const [state, dispatch] = useReducer(reducer, { items: [] });
  return <button onClick={() => dispatch({ type: 'add', item: 'a' })}>{state.items.length}</button>;
}
```
- **Semantic facts required:**
  - Which function value reaches the first argument of `useReducer` (through identifiers, `useCallback`, curried factories, `combineReducers`-style composition).
  - Per return statement: does the returned value alias the `state` parameter, or is it a fresh allocation?
  - Per path to each such return: was any location reachable from `state` written (property store, mutating array method, a callee summarised as mutating its argument)?
  - The distinction between an intentional bail-out (returns `state` unchanged, no mutation on the path) and the bug (returns `state` after mutation). These are syntactically identical returns.
  - Purity of the reducer more generally: writes to captured variables, I/O, `Math.random`, since StrictMode double-invokes it.

---

### S-STATE-11: non-converging-render-phase-write
- **What it flags:** a setter called during render on a path where the written value is not `Object.is`-equal to the current slot value at the fixpoint, so the render loop does not converge.
- **Why it matters:** React re-renders immediately, the write happens again, and after ~25 passes it throws "Too many re-renders". Before it throws, the tab is frozen. The most common source is a fresh array or object written unconditionally.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useState } from 'react';

function Chart({ points }: { points: number[] }) {
  const [scaled, setScaled] = useState<number[]>([]);
  setScaled(points.map(p => p * 2));
  return <p>{scaled.length}</p>;
}
```
- **Silent on:**
```tsx
import { useState } from 'react';

function Chart({ points }: { points: number[] }) {
  const [prev, setPrev] = useState(points);
  const [selected, setSelected] = useState(0);
  if (prev !== points) {
    setPrev(points);
    setSelected(0);
  }
  return <p>{points[selected]}</p>;
}
```
- **Semantic facts required:**
  - Phase classification of the call site: the component body and anything it calls synchronously during render, as opposed to a handler literal defined there.
  - Reachability of the write on the *next* render given the abstract state produced by this render's writes: a fixpoint over the render function, not a single pass. The guarded form must be shown to disable itself.
  - Reference-freshness of the written value: does the expression allocate on every evaluation (`map`, spread, object/array literal, `new`), which makes `Object.is` always false even when contents match?
  - `Object.is` comparison between the written value and the slot value at the fixpoint, per slot.
  - Which slot each write targets, so that the pair "write prev, write selected" is analysed as a mutually reinforcing set rather than two independent writes.

---

### S-STATE-12: non-converging-effect-write-loop
- **What it flags:** an effect that writes a slot which is also in that effect's dependency set (directly or through a derived value), where the write is not disabled after the first cycle.
- **Why it matters:** render → effect → setState → render, forever. The tab pegs a core, and everything else in the effect (a fetch, an analytics call) repeats at full speed, which turns a UI bug into a server incident.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useEffect, useState } from 'react';

function Profile({ id }: { id: string }) {
  const [user, setUser] = useState<{ id: string; seen?: boolean }>({ id });
  useEffect(() => {
    setUser({ ...user, seen: true });
  }, [user]);
  return <p>{user.id}</p>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from 'react';

function Profile({ id }: { id: string }) {
  const [user, setUser] = useState<{ id: string; seen?: boolean }>({ id });
  useEffect(() => {
    if (user.seen) return;
    setUser({ ...user, seen: true });
  }, [user]);
  return <p>{user.id}</p>;
}
```
- **Semantic facts required:**
  - The effect's dependency set after resolution: which slots and which values derived from slots it actually compares, including a dep that is a memo whose inputs include the written slot.
  - Whether the written value is reference-fresh on every evaluation (spread, literal, `map`, `new Date`), because that defeats the dep comparison even when the contents are stable.
  - A fixpoint over the render/commit cycle: apply the write, re-evaluate the guard and the dep comparison under the resulting abstract state, and decide whether a second write is reachable.
  - Guard reasoning strong enough to see that a property set by the write falsifies the condition that reached the write (a relational fact between the written value and the guard predicate).
  - The functional-updater case, which does not fix the loop (`setUser(u => ({...u}))` with `[user]` in deps still loops), so the rule must not treat the updater form as a cure.

---

### S-STATE-13: parent-setter-invoked-during-child-render
- **What it flags:** a call, during a component's render phase, to a function value that is a state setter (or a dispatch) belonging to a different component instance.
- **Why it matters:** React logs "Cannot update a component while rendering a different component", and when the reported value is reference-fresh it becomes an unbounded loop across the two components. Under concurrent rendering the parent's tree can be re-entered mid-work.
- **Severity intent:** error.
- **Fires on:**
```tsx
import { useState } from 'react';

function Child({ onMeasure }: { onMeasure: (n: number) => void }) {
  const width = 42;
  onMeasure(width);
  return <div>{width}</div>;
}

function Parent() {
  const [width, setWidth] = useState(0);
  return (
    <>
      <span>{width}</span>
      <Child onMeasure={setWidth} />
    </>
  );
}
```
- **Silent on:**
```tsx
import { useRef } from 'react';

function Child({ onMeasure }: { onMeasure: (n: number) => void }) {
  const width = 42;
  onMeasure(width);
  return <div>{width}</div>;
}

function Parent() {
  const last = useRef(0);
  return <Child onMeasure={n => { last.current = n; }} />;
}
```
- **Semantic facts required:**
  - Cross-component prop flow: for every call site, which function value reaches the prop, resolved through wrappers (`onMeasure={n => setWidth(n)}` must resolve to the setter too), `useCallback`, and objects/contexts carrying callbacks.
  - Whether a function value is a state setter or a `dispatch`, and which component instance owns the slot it writes.
  - Phase of the call site inside the child: render body (including anything called synchronously from it) vs effect vs event handler vs timer.
  - Owner comparison: setter's owning component ≠ the component currently rendering. A component calling its *own* setter during render is S-STATE-11 with different convergence rules.
  - Whether the argument is reference-fresh or value-stable, which separates "React warning" from "React warning plus infinite loop".

---

### S-STATE-14: asymmetric-correlated-slot-writes
- **What it flags:** slots that are written together on most paths but where some path (typically an exception path) writes one and leaves the other at a value that path never intended.
- **Why it matters:** a failed request sets the error but never clears the loading flag, so the spinner spins forever and the retry button stays disabled. The user's only recovery is a page reload, and the failure is invisible in tests that only exercise the happy path.
- **Severity intent:** warning (error when the abandoned slot gates an interactive control).
- **Fires on:**
```tsx
import { useState } from 'react';

function Loader() {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const load = async () => {
    setLoading(true);
    try {
      await fetch('/api');
      setLoading(false);
    } catch {
      setError('failed');
    }
  };
  return <button onClick={load} disabled={loading}>{error ?? 'load'}</button>;
}
```
- **Silent on:**
```tsx
import { useState } from 'react';

function Loader() {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const load = async () => {
    setLoading(true);
    try {
      await fetch('/api');
    } catch {
      setError('failed');
    } finally {
      setLoading(false);
    }
  };
  return <button onClick={load} disabled={loading}>{error ?? 'load'}</button>;
}
```
- **Semantic facts required:**
  - Per-path write sets: for every path through the operation, which slots are written and with which abstract values.
  - Inference of a correlated slot group: slots co-written on a majority of paths from a common entry, or slots that appear together in one rendered condition (`loading ? … : error ? …`), which is evidence they encode one machine.
  - Exception edges: a `throw` or a rejected `await` inside `try` must produce a real CFG edge to the `catch`, and `finally` must post-dominate both the normal and exceptional exits.
  - Value tracking sufficient to know the abandoned slot is *not already* at its resting value on that path (`loading` is `true` on entry to the catch because of the write above).
  - Which slot values gate rendered affordances (`disabled=`, an early `return <Spinner/>`), to rank the severity.
  - Async lifetime: the write on the failing path happens in a continuation of the same logical operation, so paths must be tracked across `await`.

---

### S-STATE-15: unguarded-async-write-race
- **What it flags:** a setter invoked from an async continuation started in an effect, where the effect can re-run (or unmount) before the continuation resolves and no cleanup provably suppresses the write.
- **Why it matters:** two in-flight requests resolve out of order and the slower, older response wins, so the results list shows matches for a query the user already replaced and keeps showing them until the next keystroke. On unmount the same write leaks the request and any state it captured.
- **Severity intent:** warning (error when the effect's deps change on a high-frequency input).
- **Fires on:**
```tsx
import { useEffect, useState } from 'react';

function Search({ query }: { query: string }) {
  const [results, setResults] = useState<string[]>([]);
  useEffect(() => {
    fetch(`/api?q=${query}`)
      .then(r => r.json())
      .then(setResults);
  }, [query]);
  return <ul>{results.map(r => <li key={r}>{r}</li>)}</ul>;
}
```
- **Silent on:**
```tsx
import { useEffect, useState } from 'react';

function Search({ query }: { query: string }) {
  const [results, setResults] = useState<string[]>([]);
  useEffect(() => {
    let current = true;
    fetch(`/api?q=${query}`)
      .then(r => r.json())
      .then(data => {
        if (current) setResults(data);
      });
    return () => {
      current = false;
    };
  }, [query]);
  return <ul>{results.map(r => <li key={r}>{r}</li>)}</ul>;
}
```
- **Semantic facts required:**
  - Identification of the async continuation: the `.then` callback or the code after an `await`, and the fact that the setter call is reachable from it.
  - Whether the enclosing effect can run more than once for one mounted instance (non-empty deps, or deps that are reference-fresh each render).
  - Whether a cleanup function exists and whether it writes a variable captured by the continuation that *guards* the setter call on every path (the flag must dominate the write, and no path may write the slot unguarded).
  - Alternative guards that are equally valid: an `AbortController` whose `signal` is passed to the request *and* whose `abort` is called from the cleanup; a request-sequence counter compared before the write; a check that the response's key matches the current dep.
  - Ordering semantics of the underlying API: fetch/XHR responses may arrive out of order, so the finding needs no proof of a specific interleaving, only that two runs can overlap.
  - Whether the write is idempotent with respect to the dep (writing a value that does not depend on `query` makes the race harmless).
