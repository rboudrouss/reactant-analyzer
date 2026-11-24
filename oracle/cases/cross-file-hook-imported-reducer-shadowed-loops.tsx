// @file App.tsx
// oracle: react=loop
// #356. The hook's file imports its `reducer` (`s + 1`) from a third file;
// the component file binds another (`s`). The hook runs the one its own
// file imports.
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
import { reducer } from "./reducer";

export function useX() {
  const [n, dispatch] = useReducer(reducer, 0);
  useEffect(() => {
    dispatch();
  }, [n]);
  return n;
}

// @file reducer.ts
export function reducer(s: number) {
  return s + 1;
}
