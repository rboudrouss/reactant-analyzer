// @file App.tsx
// oracle: react=converges
// A local hook of another file keeps its callback in a ref and calls it from
// a listener no event fires: the hook is inlined, so the callback runs where
// its body runs it, and nothing loops.
import { useState } from "react";
import { useOnResize } from "./hooks";

export function App() {
  const [n, setN] = useState(0);
  useOnResize(() => setN(n + 1));
  return <div>{n}</div>;
}

// @file hooks.tsx
import { useEffect, useRef } from "react";

export function useOnResize(cb: () => void) {
  const ref = useRef(cb);
  ref.current = cb;
  useEffect(() => {
    const h = () => ref.current();
    window.addEventListener("resize", h);
    return () => window.removeEventListener("resize", h);
  }, []);
}
