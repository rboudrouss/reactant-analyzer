// @file App.tsx
// oracle: react=loop
// #356. A hook of another file calls a hook of a third: the inner hook's
// `useReducer` names its own file's `reducer` (`s + 1`), not the outer
// hook's file's nor the component file's (`s`).
import { useOuter } from "./outer";

export function reducer(s: number) {
  return s;
}

export function App() {
  const n = useOuter();
  return <div>{n + reducer(0)}</div>;
}

// @file outer.ts
import { useInner } from "./inner";

export function reducer(s: number) {
  return s;
}

export function useOuter() {
  return useInner() + reducer(0);
}

// @file inner.ts
import { useReducer, useEffect } from "react";

function reducer(s: number) {
  return s + 1;
}

export function useInner() {
  const [n, dispatch] = useReducer(reducer, 0);
  useEffect(() => {
    dispatch();
  }, [n]);
  return n;
}
