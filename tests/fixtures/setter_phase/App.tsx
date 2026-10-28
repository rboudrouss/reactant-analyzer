import { useCallback, useMemo, useState } from "react";
import { compose } from "./util";

// #130 — a setter call that is not in statement position. The write is real
// and the walk classifies it Sync: `wrap`'s argument is evaluated at the call
// site, in the render pass.
export function NestedArgument({ wrap }: { wrap: (x: unknown) => void }) {
  const [n, setN] = useState(0);
  wrap(setN(1));
  return <div>{n}</div>;
}

// The same write inside a JSX prop's value — still evaluated during render.
export function NestedInJsxProp() {
  const [n, setN] = useState(0);
  return <div title={String(setN(1))}>{n}</div>;
}

// A callback handed to a callee with no timing summary. ⊤: the callee may run
// it during render, so the row stays — but it is never a certainty, and the
// wording must not claim the setter was called in the render body.
export function UnknownTiming() {
  const [n, setN] = useState(0);
  return <div onClick={compose(() => setN(1))}>{n}</div>;
}

// A known deferring registrar took the callback: proof that the write does not
// happen in the render pass. Not this rule's finding.
export function DeferredWrite() {
  const [n, setN] = useState(0);
  setTimeout(() => setN(1), 0);
  return <div>{n}</div>;
}

// A render-phase write under a guard the written value kills: React's
// adjust-state-during-render idiom. The setter is called during render and the
// rule drops it as converging, so the rule has proven only that no such call
// fails to settle — never that none is made (#245).
export function SettlingGuard() {
  const [s, setS] = useState<null | { v: number }>(null);
  if (s) setS(null);
  return <div>{s?.v}</div>;
}

// #249 — a memo body runs during render. With no deps array it runs at every
// render, so an unconditional write there is a certain render loop.
export function MemoNoDeps() {
  const [n, setN] = useState(0);
  const m = useMemo(() => {
    setN(n + 1);
    return n;
  });
  return <div>{m}</div>;
}

// Re-run on `[n]`: the write moves `n` only if `n + 1` differs from it, which
// the analysis does not prove, so a possible loop.
export function MemoSlotDep() {
  const [n, setN] = useState(0);
  const m = useMemo(() => {
    setN(n + 1);
    return n;
  }, [n]);
  return <div>{m}</div>;
}

// A guarded write in a memo with no deps array: never certain.
export function MemoGuarded() {
  const [n, setN] = useState(0);
  const m = useMemo(() => {
    if (n < 3) setN(n + 1);
    return n;
  });
  return <div>{m}</div>;
}

// An empty list runs the body once per mount: one update, which settles.
export function MemoMountOnly() {
  const [n, setN] = useState(0);
  const m = useMemo(() => {
    setN(n + 1);
    return n;
  }, []);
  return <div>{m}</div>;
}

// A `useCallback` body does not run during render.
export function CallbackBody() {
  const [n, setN] = useState(0);
  const inc = useCallback(() => {
    setN(n + 1);
  }, [n]);
  return <div onClick={inc}>{n}</div>;
}
