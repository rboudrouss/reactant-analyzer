// oracle: react=loop
// oracle: known=fn #268
// A closure kept in a ref and called from an effect with no deps runs after
// every commit. The heap does not place `ref.current = cb`, so the closure
// escapes and runs with the handlers (#254), and the effect's call of
// `ref.current` writes nothing the churn graph reads: no claim. The same
// holds through a local hook that keeps its callback in a ref.
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
