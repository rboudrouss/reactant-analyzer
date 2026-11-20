// @file App.tsx
// oracle: react=loop
// #356, #141. A hook of another file names its own file's `reducer`
// (`s + 1`), never the local the component binds under that name (`s`).
import { useX } from "./hook";

export function App() {
  const reducer = (s: number) => s;
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
