// oracle: react=converges
// #336. `useRef`'s object is React's, a plain object: a `Proxy` elsewhere in
// the program cannot be on its chain, so `ref.current` is the same object at
// every render and the effect runs once.
import { useEffect, useRef, useState } from "react";

export const trap: any = new Proxy({}, { get: () => ({}) });

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef({ k: 1 });
  useEffect(() => {
    setN((c) => c + 1);
  }, [ref.current]);
  return <div>{n}</div>;
}
