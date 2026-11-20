// @file App.tsx
// oracle: react=loop
// #356, #344. A hook bound to the closure another file's function returns
// names the factory file's `reducer` (`s + 1`), not the component file's
// nor the binding file's (`s`).
import { useX } from "./hooks";

export function reducer(s: number) {
  return s;
}

export function App() {
  const n = useX();
  return <div>{n + reducer(0)}</div>;
}

// @file hooks.ts
import { make } from "./make";

export function reducer(s: number) {
  return s;
}

export const useX = make();

// @file make.ts
import { useReducer, useEffect } from "react";

function reducer(s: number) {
  return s + 1;
}

export function make() {
  return function useX() {
    const [n, dispatch] = useReducer(reducer, 0);
    useEffect(() => {
      dispatch();
    }, [n]);
    return n;
  };
}
