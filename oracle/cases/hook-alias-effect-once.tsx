// @file App.tsx
// oracle: react=converges
// The alias of `hook-alias-effect-loop`, with `[]` deps: the effect runs
// once, after the first commit, and its write is not a render write.
import { useState } from "react";
import { useIsoEffect } from "./lib";

export function App() {
  const [n, setN] = useState(0);
  useIsoEffect(() => {
    setN(1);
  }, []);
  return <div>{n}</div>;
}

// @file lib.tsx
import { useEffect, useLayoutEffect } from "react";

export const useIsoEffect = typeof document !== "undefined" ? useLayoutEffect : useEffect;
