// oracle: react=loop
// #232: a utility called twice in one effect. The recursion guard of the
// utility splice remembered every name it had inlined in the CFG, so the
// second call — a sibling, not a recursion — stayed opaque, and its write to
// `b`, under `[b]`, was never seen.
import { useEffect, useState } from "react";

function bump(set: (f: (n: number) => number) => void) {
  set((n) => n + 1);
}

export function App() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  useEffect(() => {
    bump(setA);
    bump(setB);
  }, [b]);
  return (
    <div>
      {a}
      {b}
    </div>
  );
}
