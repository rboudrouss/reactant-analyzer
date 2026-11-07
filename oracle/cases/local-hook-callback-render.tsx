// @file App.tsx
// oracle: react=loop
// The local hook calls its callback while it renders: a write during every
// render, `setter-in-render`'s finding.
import { useState } from "react";
import { useNow } from "./hooks";

export function App() {
  const [n, setN] = useState(0);
  useNow(() => setN(n + 1));
  return <div>{n}</div>;
}

// @file hooks.tsx
export function useNow(cb: () => void) {
  cb();
}
