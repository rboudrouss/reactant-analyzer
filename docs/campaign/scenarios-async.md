# Wish-list: async / refs / event handlers / ecosystem integration

15 rule scenarios for a semantic (abstract-interpretation, CFG-based) React analyzer.
Each "Silent on" case is deliberately built to be the shape a naive syntactic rule
would flag.

---

### S-ASYNC-1: setstate-after-await-without-liveness-guard
- **What it flags:** an `await`/`.then` continuation started by an effect reaches a state setter, a ref write, or a DOM/ref access on a path where no cleanup-linked liveness guard has been checked since the last suspension point.
- **Why it matters:** the continuation of a request keeps the whole component closure (and the response body) alive after the tree is torn down; when the same continuation touches `ref.current` or a DOM node captured before the `await`, it operates on a detached node, and under Strict Mode / fast route re-entry the discarded first instance still runs its `setLoading(false)` and its follow-up work (toasts, analytics, `scrollIntoView`) for a screen the user has already left.
- **Severity intent:** warning
- **Fires on:**
```tsx
function UserCard({ id }: { id: string }) {
  const [user, setUser] = useState<User | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    async function load() {
      const res = await fetch(`/api/users/${id}`);
      const data = await res.json();
      setUser(data);                          // no guard since the last await
      boxRef.current?.scrollIntoView();       // node may be detached
    }
    void load();
  }, [id]);

  return <div ref={boxRef}>{user?.name}</div>;
}
```
- **Silent on:**
```tsx
function UserCard({ id }: { id: string }) {
  const [user, setUser] = useState<User | null>(null);

  useEffect(() => {
    let alive = true;
    // the guard is behind one level of indirection: a local closure,
    // not an `if (cancelled) return` in the awaiting function itself
    const commit = (data: User) => {
      if (!alive) return;
      setUser(data);
    };
    (async () => {
      const res = await fetch(`/api/users/${id}`);
      commit(await res.json());
    })();
    return () => {
      alive = false;
    };
  }, [id]);

  return <div>{user?.name}</div>;
}
```
- **Semantic facts required:**
  - For each function literal, the phase set it can execute in (render / effect body / cleanup / event handler / timer or microtask continuation).
  - The suspension points of a function: `await` expressions, `.then`/`.catch`/`.finally` callback entries, `setTimeout`/`queueMicrotask` callback entries. Each one splits the CFG into "before" and "after teardown is possible".
  - Which local bindings are *liveness flags*: a binding written in the effect body before the async call and written again (to a value that makes the guard fail) on every path of the cleanup returned by the same effect instance — including `AbortController.signal.aborted` and a ref slot whose only writers are effect body + cleanup.
  - Interprocedural: whether a guard check dominates the post-suspension write even when the check lives in a callee (`commit`) invoked after the await — so the fact needed is "does every path from the last suspension point to the write pass through a test of a liveness flag", computed over the inlined/summarised callee.
  - Which calls are state setters (and which slot they write), which are ref writes, and which are DOM reads through a ref bound to a JSX element of this component.
  - Whether the effect has a cleanup at all (an effect with no cleanup cannot have a liveness flag, which is itself the finding).

---

### S-ASYNC-2: out-of-order-response-overwrites-newer-state
- **What it flags:** an effect (or handler) that starts a request whose *inputs* vary across effect instances and writes the response into a state slot, with no ordering discriminator between the response and the slot write.
- **Why it matters:** the user types "ab" then "abc"; the "ab" response lands second and the list renders results for a query the input no longer contains, and it stays wrong until the next keystroke. This is not fixed by an unmount guard: both requests belong to a live component.
- **Severity intent:** warning
- **Fires on:**
```tsx
function Search() {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);

  useEffect(() => {
    if (!q) return;
    let alive = true;                       // unmount-safe, still racy
    fetch(`/api/search?q=${q}`)
      .then((r) => r.json())
      .then((data) => {
        if (alive) setHits(data);
      });
    return () => {
      alive = false;
    };
  }, [q]);

  return <input value={q} onChange={(e) => setQ(e.target.value)} />;
}
```
Note: `alive` is per-effect-instance here, so it *does* discriminate — this exact
snippet is safe. The firing shape is the one below, where the flag is hoisted to a
ref and therefore shared across effect instances:
```tsx
function Search() {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const mounted = useRef(true);
  useEffect(() => () => { mounted.current = false; }, []);

  useEffect(() => {
    if (!q) return;
    fetch(`/api/search?q=${q}`)
      .then((r) => r.json())
      .then((data) => {
        if (mounted.current) setHits(data);   // survives an older request too
      });
  }, [q]);

  return <input value={q} onChange={(e) => setQ(e.target.value)} />;
}
```
- **Silent on:**
```tsx
function Search() {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const seqRef = useRef(0);

  useEffect(() => {
    if (!q) return;
    const seq = ++seqRef.current;              // monotonic ticket
    fetch(`/api/search?q=${q}`)
      .then((r) => r.json())
      .then((data) => {
        if (seq !== seqRef.current) return;    // older response discarded
        setHits(data);
      });
  }, [q]);

  return <input value={q} onChange={(e) => setQ(e.target.value)} />;
}
```
- **Semantic facts required:**
  - Whether two *concurrent* instances of the same effect can exist: does the effect's dependency set contain a value that can change while a started request is still pending (i.e. the effect can re-run before the continuation runs) — this needs the dep set, plus the fact that the continuation is scheduled, not synchronous.
  - Whether a guard binding is *per-effect-instance* (declared inside the effect body, captured by that instance's closure and only mutated by that instance's cleanup) or *shared across instances* (a ref slot, a module-level `let`, a `useState` value). Only per-instance flags, or a comparison against a shared monotonic/latest-value slot, discriminate ordering.
  - For a shared discriminator: that the value read after the suspension point (`seqRef.current`, `latestQueryRef.current`) is written on *entry* of every effect instance before the request starts, and that the post-await test compares the captured copy to the shared slot.
  - Whether the effect's cleanup aborts the in-flight request (an `AbortController` whose `signal` provably reaches the request call and whose `abort()` is on every cleanup path) — that also discriminates.
  - Which slot the continuation writes, and whether that slot is also written by the newer instance (same slot ⇒ overwrite; disjoint slots keyed by request input ⇒ no overwrite).

---

### S-ASYNC-3: event-object-used-after-a-suspension-point
- **What it flags:** a read of `event.currentTarget` / `event.target` / a call to `preventDefault()` / `stopPropagation()` on a path that is only reachable after an `await` or a `.then` callback boundary inside an event handler.
- **Why it matters:** `currentTarget` is nulled once dispatch finishes, so `new FormData(e.currentTarget)` throws `TypeError` after the first `await`; `preventDefault()` after an `await` is a no-op, so the form does a full page navigation and the user loses everything they typed, intermittently, depending on how fast the validation promise resolves.
- **Severity intent:** error
- **Fires on:**
```tsx
function SignupForm() {
  const [email, setEmail] = useState("");

  async function onSubmit(e: React.FormEvent<HTMLFormElement>) {
    const ok = await validateEmail(email);
    if (!ok) {
      e.preventDefault();                     // too late: already navigated
      return;
    }
    const body = new FormData(e.currentTarget); // currentTarget === null
    await fetch("/api/signup", { method: "POST", body });
  }

  return (
    <form onSubmit={onSubmit}>
      <input value={email} onChange={(e) => setEmail(e.target.value)} />
    </form>
  );
}
```
- **Silent on:**
```tsx
function SignupForm() {
  const [email, setEmail] = useState("");

  async function onSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;              // captured synchronously
    const body = new FormData(form);
    const ok = await validateEmail(email);
    if (!ok) return;
    form.reset();                              // alias, still live: the node exists
    await fetch("/api/signup", { method: "POST", body });
  }

  return (
    <form onSubmit={onSubmit}>
      <input value={email} onChange={(e) => setEmail(e.target.value)} />
    </form>
  );
}
```
- **Semantic facts required:**
  - Which function literals are event handlers: values flowing into a JSX `on*` prop, into `addEventListener`, or into a prop that a callee installs as a handler (cross-component prop flow).
  - Which parameter binding of that function is the event object (position 0 of a handler), and its alias set through locals, destructuring, and object spread.
  - Per member/method: whether it is *dispatch-scoped* (`currentTarget`, `preventDefault`, `stopPropagation`, `nativeEvent.composedPath()`) or durable (`target`, extracted values, a captured node).
  - The dominance relation between the handler entry, each suspension point, and each dispatch-scoped access: flag exactly when every path to the access crosses at least one suspension point.
  - That reading a dispatch-scoped member into a local *before* any suspension point makes the resulting node reference durable (so the alias is not itself flagged).
  - Whether the handler is `async` or returns a promise chain at all (a sync handler never fires this rule).

---

### S-ASYNC-4: render-reads-a-ref-slot-written-outside-render
- **What it flags:** a `ref.current` read that is reachable on a render-phase path and whose value flows into the render result, when the same ref slot has at least one writer in a non-render phase.
- **Why it matters:** the rendered output is derived from a value React never observes changing, so the UI shows a stale number forever, or — worse under concurrent rendering — shows a value that depends on how many times the render was replayed, producing hydration mismatches and torn output between two components reading the same ref.
- **Severity intent:** error
- **Fires on:**
```tsx
function Counter() {
  const count = useRef(0);
  return (
    <button
      onClick={() => {
        count.current += 1;                 // written in the handler phase
      }}
    >
      clicked {count.current} times          {/* read during render: never updates */}
    </button>
  );
}
```
- **Silent on:**
```tsx
function Chart({ data }: { data: number[] }) {
  const engine = useRef<Engine | null>(null);
  if (engine.current === null) {
    engine.current = new Engine();           // lazy init: a render-phase read+write
  }

  const lastPaint = useRef(0);
  useEffect(() => {
    engine.current!.draw(data);
    lastPaint.current = Date.now();          // non-render writer...
  }, [data]);

  return (
    <canvas
      onDoubleClick={() => alert(`last painted ${lastPaint.current}`)} // ...read in the handler phase only
      ref={(node) => engine.current!.attach(node)}
    />
  );
}
```
- **Semantic facts required:**
  - For every `.current` read: whether the enclosing function literal executes in the render phase, i.e. the read is reachable from the component body without crossing a function value that only escapes into an effect / handler / timer.
  - Whether the read's value flows into the render result: the returned element tree, a JSX attribute expression, a hook argument that influences output, or a branch condition that selects the returned tree. Reads whose value is discarded or only logged do not fire.
  - Per ref slot, the full writer set with the phase of each writer, including writes through aliases (`const r = someRef; r.current = x`) and writes performed by a callee to a ref passed as an argument or prop.
  - The lazy-initialisation exemption, stated mechanically: every render-phase write to the slot is guarded by a nullish test of the *same* slot, and the slot has no non-render writer whose value the guarded read can observe. (A slot that is lazily initialised *and* mutated from a handler is still a finding.)
  - Ref identity across renders: `useRef` returns the same object for the component instance, so slot-level facts must be per (component instance, hook site), not per render.
  - Whether the ref is a *callback ref* parameter (a node handed to the ref function during commit) rather than a ref object — those never fire.

---

### S-ASYNC-5: state-slot-that-never-reaches-render
- **What it flags:** a `useState`/`useReducer` slot whose value never flows into the render result of any component and is not compared or used as a dependency to gate work, while its setter is called from a high-frequency non-render source.
- **Why it matters:** every scroll/mousemove/tick calls a setter, which schedules a full re-render of the subtree (here an expensive list) to produce byte-identical output; on a mid-range phone this turns a 60fps scroll into a 15fps one. A `useRef` is behaviour-preserving except for the re-render.
- **Severity intent:** warning
- **Fires on:**
```tsx
function ScrollSpy() {
  const [lastY, setLastY] = useState(0);

  useEffect(() => {
    const onScroll = () => {
      const y = window.scrollY;
      if (y > lastY) analytics("scroll_down");
      setLastY(y);                            // re-render per scroll event
    };
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, [lastY]);

  return <ExpensiveList />;                   // lastY reaches nothing rendered
}
```
- **Silent on:**
```tsx
function ScrollSpy() {
  const [lastY, setLastY] = useState(0);

  useEffect(() => {
    const onScroll = () => {
      const y = window.scrollY;
      if (y > lastY) analytics("scroll_down");
      setLastY(y);
    };
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, [lastY]);

  const sticky = lastY > 100;                 // flows into the tree via a boolean
  return <ExpensiveList sticky={sticky} />;
}
```
- **Semantic facts required:**
  - A reachability relation from the slot's read sites to the render result: direct JSX interpolation, an attribute value, a prop of a child element, the condition of a branch that selects between returned trees, an argument of a hook whose result is rendered, or a value written into a context provider.
  - Transitive flow through `useMemo`/`useCallback` results, local consts, and object/array literals that end up as props — the boolean derivation in the silent case must be followed.
  - Whether the slot is used as a dependency-array entry, or as the input of a comparison that gates a *rendered* effect of the program (e.g. an effect that itself writes a rendered slot). Such uses make the state semantically necessary even though it is not rendered.
  - The call sites of each setter for the slot, and their phase + expected frequency class: a setter reachable only from a subscription callback / timer / animation frame is high-frequency; a setter in a click handler is not.
  - The cost estimate of the subtree that re-renders: whether the component returns a memoised leaf or a large tree — used to pick warning vs info, not to decide correctness.
  - Whether reads of the slot are all in phases where the *latest* value would be available from a ref (i.e. no read depends on the render-time snapshot semantics of state).

---

### S-ASYNC-6: mount-pinned-callback-prop
- **What it flags:** a function-valued prop (or context value) that a callee captures into a mount-only subscription — timer, listener, imperative library registration — so that later values of that prop are never invoked.
- **Why it matters:** the parent's callback closes over the parent's current state; the child pinned the very first one at mount, so the timer forever logs `0`, or "save" forever posts the document as it was on first render. `exhaustive-deps` cannot see this: the child's dep array is genuinely correct for the child, and the stale binding lives in the *parent*.
- **Severity intent:** warning
- **Fires on:**
```tsx
function Ticker({ onTick }: { onTick: () => void }) {
  useEffect(() => {
    const id = setInterval(onTick, 1000);     // pins the first onTick
    return () => clearInterval(id);
  }, []);                                     // intentionally mount-only
  return null;
}

function Parent() {
  const [n, setN] = useState(0);
  return (
    <>
      <button onClick={() => setN(n + 1)}>{n}</button>
      <Ticker onTick={() => console.log(n)} /> {/* always logs 0 */}
    </>
  );
}
```
- **Silent on:**
```tsx
function Ticker({ onTick }: { onTick: () => void }) {
  const latest = useRef(onTick);
  useEffect(() => {
    latest.current = onTick;                  // refreshed every render
  });
  useEffect(() => {
    const id = setInterval(() => latest.current(), 1000);
    return () => clearInterval(id);
  }, []);                                     // still mount-only, still [] deps
  return null;
}

function Parent() {
  const [n, setN] = useState(0);
  return (
    <>
      <button onClick={() => setN(n + 1)}>{n}</button>
      <Ticker onTick={() => console.log(n)} />
    </>
  );
}
```
- **Semantic facts required:**
  - Cross-component prop flow: which function literal (at which parent render) reaches which parameter binding of the callee, per JSX call site.
  - Whether the callee's capture is *mount-only*: the value is stored into a structure created in an effect with an empty dep set (or into a `useRef`/`useState` initialiser, a `useMemo` with empty deps, a module registry) and that structure outlives the render.
  - Whether the pinned function is *identity-fresh per parent render*: it is a literal allocated in the parent's render body, or the result of a `useCallback` whose dep set can change.
  - Whether the pinned function's body reads any render-scoped binding of the parent whose value can differ between renders (a state slot, a prop, a value derived from either). If every free variable is provably invariant (module scope, a setter, a ref object, a stable context), the pin is harmless.
  - The "latest ref" exemption, mechanically: the subscription invokes `slot.current` (not the captured binding) where `slot` is a ref written on every render/commit with the current prop value.
  - Which registration APIs create a long-lived capture: `setInterval`/`setTimeout`, `addEventListener`, `.on`/`.subscribe`/`.observe` on an object whose lifetime spans renders, and `requestAnimationFrame` loops that re-arm themselves.

---

### S-ASYNC-7: unsubscribe-uses-a-different-reference
- **What it flags:** a teardown call (`removeEventListener`, `off`, `unsubscribe`, `clearTimeout`) whose handler/handle argument is provably not the same runtime value as the one passed to the matching registration on that path.
- **Why it matters:** the listener is never removed. After ten route visits, ten `resize` handlers run per event, each holding a whole component closure; the page slowly degrades and stale handlers write into torn-down state. Nothing throws, so it is only ever found by profiling.
- **Severity intent:** error
- **Fires on:**
```tsx
function Sidebar() {
  const [w, setW] = useState(0);

  useEffect(() => {
    const handleResize = () => setW(window.innerWidth);
    window.addEventListener("resize", debounce(handleResize, 100)); // fresh wrapper
    return () => window.removeEventListener("resize", handleResize); // different value
  }, []);

  return <aside style={{ width: w }} />;
}
```
- **Silent on:**
```tsx
function Sidebar() {
  const [w, setW] = useState(0);

  useEffect(() => {
    const handleResize = () => setW(window.innerWidth);
    const debounced = debounce(handleResize, 100);
    const teardown = debounced;                  // aliased under a second name
    window.addEventListener("resize", debounced, { passive: true });
    return () => {
      window.removeEventListener("resize", teardown, { passive: true });
    };
  }, []);

  return <aside style={{ width: w }} />;
}
```
- **Semantic facts required:**
  - Allocation-site identity for function values: two expressions denote the same runtime function iff they resolve to the same allocation on the path in question. `debounce(f, 100)` allocates a *new* value at each call site and each execution; `f` and an alias of `f` do not.
  - Alias tracking through locals, `const` re-binding, object properties, and ref slots, so that a differently-named binding holding the same allocation is recognised as equal.
  - The pairing relation between a registration call and a teardown call: same target object (abstract value of the receiver), same event/topic string (or the same abstract string value), and the same listener slot — plus the `capture` flag of the options argument, since a mismatched `capture` fails removal even with identical identity.
  - Which effect instance a cleanup belongs to: the cleanup closure sees the same scope as its own effect body, so a value allocated in the body is comparable to one read in the cleanup.
  - Handle-valued teardowns: `clearTimeout(id)` must receive the value produced by the `setTimeout` on every path (including when the handle variable is conditionally assigned or reassigned between registration and teardown).
  - Whether the registration is on an object that outlives the component (window, document, a store, a socket) — teardown mismatches on a per-effect-lifetime object are still findings but weaker.

---

### S-ASYNC-8: acquired-resource-not-released-on-every-cleanup-path
- **What it flags:** a resource acquired on some path of an effect body (observer, subscription, timer, socket, object URL, media stream) for which some cleanup path does not execute a release.
- **Why it matters:** the observer keeps firing on a disconnected node and keeps the node, the callback and the component's state alive; with a route the user visits repeatedly this is an unbounded leak, and the resurrected callbacks do real work (layout reads, network) for invisible components.
- **Severity intent:** error when no release exists at all; warning when the release is conditional on state that may differ between acquisition and cleanup.
- **Fires on:**
```tsx
function Panel({ enabled, node }: { enabled: boolean; node: Element }) {
  const [size, setSize] = useState(0);

  useEffect(() => {
    let timer: number | undefined;
    if (enabled) {
      const obs = new ResizeObserver(([e]) => setSize(e.contentRect.width));
      obs.observe(node);                       // acquired, never disconnected
      timer = window.setTimeout(() => setSize(0), 5000);
    }
    return () => clearTimeout(timer);
  }, [enabled, node]);

  return <div>{size}</div>;
}
```
- **Silent on:**
```tsx
function Panel({ enabled, node }: { enabled: boolean; node: Element }) {
  const [size, setSize] = useState(0);

  useEffect(() => {
    const teardown: Array<() => void> = [];
    if (enabled) {
      const obs = new ResizeObserver(([e]) => setSize(e.contentRect.width));
      obs.observe(node);
      teardown.push(() => obs.disconnect());   // released indirectly
    }
    const timer = window.setTimeout(() => setSize(0), 5000);
    teardown.push(() => clearTimeout(timer));
    return () => teardown.forEach((fn) => fn());
  }, [enabled, node]);

  return <div>{size}</div>;
}
```
- **Semantic facts required:**
  - A resource model: constructor/acquire calls (`new ResizeObserver` + `.observe`, `new MutationObserver`, `setInterval`, `addEventListener`, `.subscribe`, `URL.createObjectURL`, `getUserMedia`) paired with their release operations, matched on the receiver's abstract value, not on the identifier.
  - For each acquisition site reachable in the effect body, whether *every* execution path of the cleanup closure performs the matching release on the same abstract value — a must-analysis, not may.
  - Escape analysis of the handle: whether the acquired value is reachable from the cleanup closure's captured environment, directly or through a container (array, object, ref slot, `Map`) it was pushed into.
  - Higher-order release: a release performed by invoking closures stored in a container (the `teardown.forEach` shape) counts, which requires resolving the call target of `fn()` to the set of closures the container can hold.
  - Path correlation between the effect body and the cleanup: both see the same `enabled`, so a cleanup guarded by the same condition that guarded the acquisition is complete; a cleanup guarded by a *ref* or by state read at cleanup time is not (that is the warning case).
  - Whether the effect returns a cleanup at all, and whether an early `return` in the body skips the cleanup registration while a resource has already been acquired.

---

### S-ASYNC-9: controlled-input-does-not-write-its-value-slot-on-every-path
- **What it flags:** a controlled input whose `onChange` has a reachable path that does not write the state slot the `value` prop reads.
- **Why it matters:** on that path React re-renders with the old value, so the DOM value is reverted: the user's keystroke disappears and, if the input is not empty, the caret jumps to the end of the text. It usually only happens on the validation-failure branch, so it ships.
- **Severity intent:** warning (deliberate input masking has the same shape)
- **Fires on:**
```tsx
function AmountField({ max }: { max: number }) {
  const [amount, setAmount] = useState("");
  const [error, setError] = useState<string | null>(null);

  return (
    <>
      <input
        value={amount}
        onChange={(e) => {
          const v = e.target.value;
          if (Number(v) > max) {
            setError("too large");            // amount slot untouched -> keystroke lost
            return;
          }
          setError(null);
          setAmount(v);
        }}
      />
      {error}
    </>
  );
}
```
- **Silent on:**
```tsx
function AmountField({ max }: { max: number }) {
  const [amount, setAmount] = useState("");
  const [error, setError] = useState<string | null>(null);

  return (
    <>
      <input
        value={amount}
        onChange={(e) => {
          const v = e.target.value;
          if (Number(v) > max) {
            setError("too large");
            setAmount(v.slice(0, -1));        // still writes the slot on this path
            return;
          }
          setError(null);
          setAmount(v);
        }}
      />
      {error}
    </>
  );
}
```
- **Semantic facts required:**
  - Which state slot the `value`/`checked` expression of the element reads, following derivations (`value={form.amount}` where `form` is one slot ⇒ that slot with a path selector; `value={String(amount)}` ⇒ that slot).
  - Which function literal is the element's `onChange`, including when it arrives as a prop from a parent or is produced by a factory (`makeHandler("amount")`).
  - Whether every terminating path of the handler writes the identified slot (or, for an object slot, the same member path) — a must-write analysis over the handler's CFG including early returns, `throw`, and branches inside callees.
  - Whether the element is actually controlled: the `value` prop's abstract value is not `undefined` on any path, and there is no `defaultValue`/`readOnly`/`disabled` that makes the write unnecessary.
  - Whether a writing path exists at all in a *different* function reachable synchronously from the handler (a `dispatch` to a reducer whose case writes the slot) — the reducer's case-to-slot mapping must be resolved, not just the `dispatch` call.
  - Whether a non-writing path is only reachable for event values the element cannot produce (e.g. guarded by a condition on a value that is not the event's) — such paths do not fire.

---

### S-ASYNC-10: lost-update-from-a-stale-state-snapshot
- **What it flags:** two writes to the same state slot in one execution of a handler/effect where the second write's value is computed from the render-time snapshot rather than from the first write's result — and the same shape where the stale snapshot is sent to a server or storage.
- **Why it matters:** the first update is silently discarded: the user toggles two filters and only the second sticks; or `api.save(items)` persists the list *without* the item that was just added, so a refresh makes the item disappear and the bug is blamed on the backend.
- **Severity intent:** error for the double-write case, warning for the persist-stale-value case
- **Fires on:**
```tsx
function Filters() {
  const [f, setF] = useState({ archived: false, mine: false });

  const showMyArchived = () => {
    setF({ ...f, archived: true });
    setF({ ...f, mine: true });      // recomputed from the same snapshot: archived lost
  };

  return <button onClick={showMyArchived}>mine + archived</button>;
}
```
- **Silent on:**
```tsx
function Filters() {
  const [f, setF] = useState({ archived: false, mine: false });

  const showMyArchived = () => {
    let next = { ...f, archived: true };
    setF(next);
    next = { ...next, mine: true };  // threaded through the local, not re-read from f
    setF(next);                      // last write wins, and it carries both changes
  };

  return <button onClick={showMyArchived}>mine + archived</button>;
}
```
- **Semantic facts required:**
  - Which slot each setter call writes, and the abstract value of its argument (including through spreads and object literals).
  - Whether the argument of the second write *depends on the pre-update read* of the same slot: a data-dependence from the slot's render-time binding to the argument expression, with locals threaded (the silent case's `next` breaks that dependence because it depends on the first write's value, not on `f`).
  - Batching semantics: two setter calls in the same phase body both apply to the same snapshot, so the second overwrites the first unless it is a functional updater — the analyser must know `setF(fn)` receives the pending value while `setF(v)` does not.
  - For the persist variant: whether the stale-read value flows into an escaping call (network, `localStorage`, a store `dispatch`) that is ordered *after* the setter in the CFG, and whether an alternative post-update value exists in scope.
  - The reachability of both writes on a common path (two setters in mutually exclusive branches are not a lost update).
  - Identification of functional-updater arguments even behind indirection (`setF(withMine)` where `withMine` is a named function of one parameter).

---

### S-ASYNC-11: query-key-omits-an-input-of-the-fetcher
- **What it flags:** a data-fetching hook (`useQuery`/`useSWR`/`useInfiniteQuery`) whose fetcher closure reads a render-scoped value that flows into the *request*, while that value is absent from the evaluated cache key.
- **Why it matters:** two components (or two routes) share one cache entry: opening user B's profile shows user A's data from cache, and because the key did not change nothing ever refetches. Writes are worse — an optimistic update lands on the wrong entry.
- **Severity intent:** error when the omitted value provably varies; warning otherwise
- **Fires on:**
```tsx
function Profile({ userId }: { userId: string }) {
  const { data } = useQuery({
    queryKey: ["profile"],                    // does not mention userId
    queryFn: () => fetch(`/api/profile/${userId}`).then((r) => r.json()),
  });
  return <span>{data?.name}</span>;
}
```
- **Silent on:**
```tsx
const profileKeys = {
  detail: (id: string) => ["profile", "detail", id] as const,
};

function Profile({ userId }: { userId: string }) {
  const { t } = useTranslation();
  const { data } = useQuery({
    queryKey: profileKeys.detail(userId),     // key built by a factory
    queryFn: ({ signal }) =>
      fetch(`/api/profile/${userId}`, { signal })
        .then((r) => (r.ok ? r.json() : Promise.reject(new Error(t("failed"))))),
  });
  return <span>{data?.name}</span>;
}
```
- **Semantic facts required:**
  - Which hook call is a keyed-cache hook, and which argument/property is the key and which the fetcher (object form, positional form, SWR's `(key, fetcher)`).
  - The abstract value of the key expression, evaluated through helper calls, spreads and template literals, flattened to the set of *value-carrying leaves* it contains — the factory in the silent case must be summarised, not pattern-matched.
  - The set of render-scoped bindings free in the fetcher closure (transitively through helper functions it calls), with their abstract values.
  - A taint relation "flows into the request": the value reaches a URL string, a path segment, a query parameter, a request body, a header, or a positional argument of an API-client call. Captures that only reach logging, error messages, i18n or the `signal` are exempt — that is what keeps the silent case quiet.
  - Whether a captured value can vary at all: a module constant or a value provably invariant for the component's lifetime is not a key input; a prop, a state slot, a router param, or a context value is.
  - Equality of a key leaf with a captured value: same slot / same binding, or a value derived from it by an injective-enough operation (`String(userId)`), so that including `userId` under a different shape still counts.

---

### S-ASYNC-12: store-selector-allocates-a-fresh-snapshot
- **What it flags:** a selector passed to an external-store hook (`useSelector`, zustand's `useStore`, `useSyncExternalStore`'s `getSnapshot`) that allocates a new object/array on every invocation, without an equality comparator or memoised producer.
- **Why it matters:** the subscription compares snapshots by reference, so the component re-renders on *every* store action anywhere in the app — and under `useSyncExternalStore` React detects the never-stable snapshot and either logs "The result of getSnapshot should be cached to avoid an infinite loop" or spins in a render loop that locks the tab.
- **Severity intent:** error when the hook compares by reference with no comparator; warning when a comparator exists but is `Object.is`-equivalent
- **Fires on:**
```tsx
function CartBadge() {
  const { items, total } = useStore((s) => ({
    items: s.items,
    total: s.total,                            // new object per call
  }));
  return <span>{items.length} / {total}</span>;
}
```
- **Silent on:**
```tsx
function CartBadge() {
  const { items, total } = useStore(
    useShallow((s) => ({                       // wrapper installs shallow equality
      items: s.items,
      total: s.total,
    })),
  );
  const visible = useSelector(selectVisibleItems); // createSelector-memoised producer
  return <span>{items.length} / {total} / {visible.length}</span>;
}
```
- **Semantic facts required:**
  - Which hooks subscribe to an external store and how they compare successive snapshots: by `Object.is` on the selector result, by a comparator argument, or by a comparator installed by a wrapper.
  - Whether the selector's return value is a *fresh allocation on every call*: an object/array literal, a `.map`/`.filter`/`.slice`/spread result, or a call to a function that allocates — versus a projection that returns a value stored in the store (`s.items`), which is reference-stable while the store slice is.
  - Whether the selector is wrapped by a known equality-installing combinator (`useShallow`, a second argument such as `shallowEqual`) — recognised by the wrapper's effect on the comparison, not by its name alone.
  - Whether the selector's producer is memoised across calls (a `createSelector`/`reselect` result, a module-level memoised function, a `useMemo`'d selector with a stable dep set): the fact needed is "for equal inputs, does this call return the identical reference as the previous call".
  - Whether the selector is allocated fresh per render *and* the hook resubscribes on selector identity (a second-order re-subscription churn).
  - Whether the returned snapshot flows into a dependency array or a memoised child's props, which turns the re-render into a cascade (severity input).

---

### S-ASYNC-13: unstable-custom-hook-result-drives-a-self-retriggering-effect
- **What it flags:** a value returned freshly allocated by a custom hook on every render, used as a dependency-array entry of an effect that (transitively) writes state read during render — a closed feedback cycle.
- **Why it matters:** the effect re-runs after every render, sets state, which re-renders, which allocates a new object, which re-runs the effect: an infinite fetch loop that hammers the API from every mounted client, usually only observed as a mysterious server load spike.
- **Severity intent:** error (the cycle is provable)
- **Fires on:**
```tsx
function useAuth() {
  const [user, setUser] = useState<User | null>(null);
  return { user, login: (u: User) => setUser(u) };   // fresh object every render
}

function Dashboard() {
  const auth = useAuth();
  const [data, setData] = useState<Data | null>(null);

  useEffect(() => {
    fetchDashboard(auth.user).then(setData);          // setData -> render -> new auth -> effect
  }, [auth]);

  return <pre>{JSON.stringify(data)}</pre>;
}
```
- **Silent on:**
```tsx
function useAuth() {
  const [user, setUser] = useState<User | null>(null);
  return { user, login: (u: User) => setUser(u) };   // still a fresh object
}

function Dashboard() {
  const auth = useAuth();
  const [data, setData] = useState<Data | null>(null);

  useEffect(() => {
    fetchDashboard(auth.user).then(setData);
  }, [auth.user]);                                    // depends on the stable field

  return <pre>{JSON.stringify(data)}</pre>;
}
```
- **Semantic facts required:**
  - Interprocedural return-value identity for custom hooks: does the hook return a value allocated during this render (object/array literal, closure, spread result) or one that is stable across renders (a state value, a ref object, a setter, a `useMemo` result whose deps did not change).
  - Which dependency-array entries the effect compares, and the abstract value of each entry expression — `auth` and `auth.user` must be distinguished, including through destructuring at the call site.
  - The re-run predicate: an effect re-runs when any dep entry is reference-unequal to the previous render's; an entry that is freshly allocated each render makes the predicate unconditionally true.
  - The feedback cycle itself: the effect body reaches a state write (possibly asynchronously, through `.then(setData)`), that slot is read during render, and the render allocates the unstable dep — a cycle in the graph (effect → slot → render → dep → effect). Without the cycle, the finding is at most "effect runs every render" (info).
  - Whether an intervening equality check breaks the cycle: the setter writes a value that is `Object.is`-equal to the current one (React bails out), or the write is guarded by a comparison against the current slot value.
  - Whether the unstable value is produced inside the *same* component or by a hook from another module — for reporting, the fix site is the hook, the symptom site is the effect.

---

### S-ASYNC-14: non-serializable-prop-crosses-the-client-boundary
- **What it flags:** a Server Component rendering an element whose component resolves to a `"use client"` module, with a prop whose abstract value can be a function, class instance, symbol, or other non-serializable value.
- **Why it matters:** the render throws `Functions cannot be passed directly to Client Components` (or silently ships a `[object Object]` for a class instance whose methods are gone), so the route 500s in production while it worked in dev because the branch that passed the callback was never hit locally.
- **Severity intent:** error
- **Fires on:**
```tsx
// app/reports/page.tsx  — server component (no "use client")
import { Chart } from "./chart";              // ./chart is a "use client" module
import { loadRows } from "@/lib/db";

export default async function Page() {
  const rows = await loadRows();
  return <Chart rows={rows} format={(n: number) => n.toFixed(2)} />;
}
```
- **Silent on:**
```tsx
// app/reports/page.tsx  — server component (no "use client")
import { Chart } from "./chart";              // same "use client" module
import { loadRows, persist } from "@/lib/db";
import { Legend } from "./legend";            // a server component

export default async function Page() {
  const rows = await loadRows();

  async function save(value: number) {
    "use server";                             // a server action: serializable as a reference
    await persist(value);
  }

  return <Chart rows={rows} onSave={save} footer={<Legend rows={rows} />} />;
}
```
- **Semantic facts required:**
  - The client-boundary graph: which modules carry a `"use client"` directive, and therefore which imported bindings are client components. A component defined in a server module that is only *rendered* by a client component is not a boundary crossing at the JSX site — the boundary is the import edge.
  - Which module the current component is defined in and whether that module is in the server graph (no `"use client"` on it or on any module in its import chain from the entry).
  - For each JSX attribute at a boundary crossing: the abstract *value kind* of the expression — function, class instance (a value whose prototype is not `Object.prototype`), symbol, `Promise`, React element, or plain data — including through spreads (`{...props}`) and nested object/array literals, since the check is deep.
  - Whether a function value carries the `"use server"` marker (directive in its body or in its defining module), which makes it a serializable action reference.
  - The exemption for React elements: an element value crosses as serialized RSC payload even though its own subtree contains handlers, so `footer={<Legend/>}` and `children` must not fire.
  - Nullability/branch sensitivity: a prop that is a function on only one path still fires, because the analysis must be a may-analysis over prop values.

---

### S-ASYNC-15: imperative-navigation-during-render
- **What it flags:** a call to a router's imperative navigation API (`router.push/replace`, `navigate(...)`, `history.pushState`) on a path that executes in a client component's render phase.
- **Why it matters:** React logs `Cannot update a component (Router) while rendering a different component`; the navigation may be issued twice (Strict Mode, or a re-render React throws away), producing a duplicated history entry so the back button appears broken, and if the render is discarded the navigation can be dropped entirely — the user sees the protected page flash before the redirect, or never redirects at all.
- **Severity intent:** error
- **Fires on:**
```tsx
"use client";
function Guard({ user }: { user: User | null }) {
  const router = useRouter();

  if (!user) {
    router.replace("/login");                 // side effect in the render phase
    return null;
  }
  return <Dashboard user={user} />;
}
```
- **Silent on:**
```tsx
// app/dashboard/page.tsx — server component
import { redirect } from "next/navigation";
import { getUser } from "@/lib/auth";

export default async function Page() {
  const user = await getUser();
  if (!user) redirect("/login");              // throws a control-flow signal: sanctioned
  return <Dashboard user={user} />;
}
```
```tsx
"use client";
function Guard({ user }: { user: User | null }) {
  if (!user) return <Navigate to="/login" replace />;   // declarative
  return <Dashboard user={user} />;
}
```
- **Semantic facts required:**
  - The phase of each call site: reachable from the component body during render, versus only from an effect body, a cleanup, an event handler, or a promise continuation.
  - Whether the enclosing component is a client component or a server component (module-level `"use client"` and the server/client module graph) — the same-looking call is correct in a server render.
  - Which callees are *imperative* navigations (they enqueue an update into a router store shared with the tree) versus *control-flow* navigations (`redirect()` throws a sentinel and never mutates a component) versus *declarative* ones (returning an element that navigates in its own commit phase).
  - The value flow that identifies the receiver as a router: `useRouter()`'s result, a `navigate` binding from `useNavigate()`, including through destructuring, aliasing and being passed into a helper called from render.
  - Reachability: the call must be reachable on some render path (a navigation inside a closure that only escapes into `onClick` is not a render-phase call, even though it is written in the render body).
  - Idempotence of the render: whether any other render-phase side effect (state setter on another component, store `dispatch`, mutation of a module-level binding) shares this shape, so the rule generalises rather than special-casing routers.
