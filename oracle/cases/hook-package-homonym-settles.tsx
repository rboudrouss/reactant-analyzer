// @file App.tsx
// oracle: react=converges
// #389, #226. `useRun` is imported from a package (`ext`): with nothing to
// call it does nothing. The program defines a `useRun` of its own whose
// effect bumps its state after every render; it is not the hook that runs
// here, so no loop may rest on it.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n] = useState(0);
  useRun(null);
  return <div>{n}</div>;
}

// @file hooks.tsx
import { useEffect, useState } from "react";

export function useRun(_x: unknown) {
  const [c, setC] = useState(0);
  useEffect(() => {
    setC(c + 1);
  });
  return c;
}
