// oracle: react=converges
// #407, N2's neighbour: in a page, the sibling's input holds the focus when
// the effect focuses the other one, so it fires `blur` and `focusout`, and
// its `onBlur` writes a new object. The element that held the focus may be
// any element: every `onBlur` stays reachable from a `focus()`, so the
// analysis claims the effect (a tolerated Warning). The oracle renders into
// a container no document holds (`react-run.mjs`), where `focus()` fires
// nothing: React settles there, and would settle in a page too, the next run
// focusing the input that already holds the focus.
import { useEffect, useRef, useState } from "react";

function Sibling() {
  const [b, setB] = useState<unknown>(0);
  return (
    <div>
      {String(!!b)}
      <input autoFocus onBlur={() => setB({})} />
    </div>
  );
}

export function App() {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
  });
  return (
    <div>
      <input ref={ref} />
      <Sibling />
    </div>
  );
}
