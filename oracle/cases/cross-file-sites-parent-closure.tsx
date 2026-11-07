// @file App.tsx
// oracle: react=loop
// #247, the soundness side of cross-file-sites-own-closure.tsx: the parent's
// `() => setV(…)` shared an allocation id with the child's own `g`, which
// replaced it in the child's heap, and the loop `f()` drives was silent.
import { useState } from "react";
import { Child } from "./Child";

export function App() {
  const [v, setV] = useState(0);
  return <Child f={() => setV((x) => x + 1)} />;
}

// @file Child.tsx
import { useEffect } from "react";

export function Child({ f }: { f: () => void }) {
  const g = () => 0;
  const h = () => 1;
  useEffect(() => {
    f();
  });
  return <div>{g() + h()}</div>;
}
