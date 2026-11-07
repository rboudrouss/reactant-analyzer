// oracle: react=loop
// #268: a closure kept in a ref and called from an effect with no deps runs
// after every commit. `useRef` returns its hook's one object, so the closure
// written into `current` is what the effect's `ref.current()` runs, with the
// effect's timing: a write the churn graph reads.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const cb = () => setN(n + 1);
  const ref = useRef(cb);
  ref.current = cb;
  useEffect(() => {
    ref.current();
  });
  return <div>{n}</div>;
}
