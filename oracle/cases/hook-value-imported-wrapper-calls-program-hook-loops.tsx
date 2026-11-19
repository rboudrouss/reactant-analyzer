// @file App.tsx
// oracle: react=loop
// #344. `wrap` is another file's and hands back `(...args) => hook(...args)`:
// the parameter's call is a call of the hook the binding's file passes, a
// hook of that file.
import { useState, useEffect } from "react";
import { wrap } from "./wrap";

function useY() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return n;
}

const useX = wrap(useY);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

// @file wrap.ts
export function wrap(hook) {
  return (...args) => hook(...args);
}
