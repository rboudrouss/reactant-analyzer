import { useState } from "react";
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
