// @file App.tsx
// oracle: react=converges
// #356, the mirror. The hook's own `reducer` hands the state back (`s`), so
// the dispatch stores what `n` already holds and React bails out; the
// component file's `reducer` (`s + 1`) is not the one the hook runs.
import { useX } from "./hook";

export function reducer(s: number) {
  return s + 1;
}

export function App() {
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
