// @file App.tsx
// oracle: react=loop
// Three files (docs/semantics.md §4): `Counter` lives in its own file and
// writes its state through `bump`, a helper of a third. The analyzer follows
// both imports and finds the loop on `Counter`.
import { Counter } from "./Counter";

export function App() {
  return (
    <div>
      <Counter />
    </div>
  );
}

// @file Counter.tsx
import { useEffect, useState } from "react";
import { bump } from "./lib";

export function Counter() {
  const [n, setN] = useState(0);
  useEffect(() => {
    bump(setN, n);
  });
  return <p>{n}</p>;
}

// @file lib.tsx
export function bump(set, n) {
  set(n + 1);
}
