// @file App.tsx
// oracle: react=converges
// #356, the mirror of the caller-local shadow. The hook's own `reducer`
// hands the state back, so React bails out. The analysis isolates the
// hook's name from the component's local and does not run it (a reducer it
// does not see stores any value): a tolerated false positive, documented.
import { useX } from "./hook";

export function App() {
  const reducer = (s: number) => s + 1;
  const n = useX();
  return <div>{n + reducer(0)}</div>;
}

// @file hook.tsx
import { useReducer, useEffect } from "react";

function reducer(s: number) {
  return s;
}

export function useX() {
  const [n, dispatch] = useReducer(reducer, 0);
  useEffect(() => {
    dispatch();
  }, [n]);
  return n;
}
