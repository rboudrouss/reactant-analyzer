// oracle: react=converges
// #268 negative: the ref holds a closure that writes a constant; the effect's
// re-run stores the value already there and React bails out.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const cb = () => setN(1);
  const ref = useRef(cb);
  ref.current = cb;
  useEffect(() => {
    ref.current();
  });
  return <div>{n}</div>;
}
