// @file App.tsx
// oracle: react=loop
// #389. `useThing` is this file's own, defined after its use and imported
// from nowhere: the call runs it, not the other file's homonym. Its effect
// bumps its state after every render.
import { useEffect, useState } from "react";

export function App() {
  const n = useThing();
  return <div>{n}</div>;
}

function useThing() {
  const [c, setC] = useState(0);
  useEffect(() => {
    setC(c + 1);
  });
  return c;
}

// @file hooks.tsx
export function useThing() {
  return 0;
}
