// @file App.tsx
// oracle: react=loop
// A local hook of another file calls its callback from an effect with no
// deps: it runs after every commit.
import { useState } from "react";
import { useEveryCommit } from "./hooks";

export function App() {
  const [n, setN] = useState(0);
  useEveryCommit(() => setN(n + 1));
  return <div>{n}</div>;
}

// @file hooks.tsx
import { useEffect } from "react";

export function useEveryCommit(cb: () => void) {
  useEffect(() => {
    cb();
  });
}
