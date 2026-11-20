// @file App.tsx
// oracle: react=loop
// #356. A hook of another file names its own file's `reducer` (`s + 1`),
// not the component file's (`s`): every dispatch moves `n`, which the effect
// depends on.
import { useX } from "./hook";

export function reducer(s: number) {
  return s;
}

export function App() {
  const n = useX();
  return <div>{n + reducer(0)}</div>;
}

// @file hook.tsx
import { useReducer, useEffect } from "react";

function reducer(s: number) {
  return s + 1;
}

export function useX() {
  const [n, dispatch] = useReducer(reducer, 0);
  useEffect(() => {
    dispatch();
  }, [n]);
  return n;
}
