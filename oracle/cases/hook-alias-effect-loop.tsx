// @file App.tsx
// oracle: react=loop
// A module binds a hook name to a conditional choosing between React's
// effect hooks (mantine's `useIsomorphicEffect`): both lower alike, so a
// call through it is that effect, here one with no deps writing a new value.
import { useState } from "react";
import { useIsoEffect } from "./lib";

export function App() {
  const [n, setN] = useState(0);
  useIsoEffect(() => {
    setN(n + 1);
  });
  return <div>{n}</div>;
}

// @file lib.tsx
import { useEffect, useLayoutEffect } from "react";

export const useIsoEffect = typeof document !== "undefined" ? useLayoutEffect : useEffect;
