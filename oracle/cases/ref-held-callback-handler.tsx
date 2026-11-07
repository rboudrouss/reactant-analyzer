// oracle: react=converges
// #268 negative: the closure kept in the ref is only called from a click
// handler — a user event stands between each render and the write.
import { useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const cb = () => setN(n + 1);
  const ref = useRef(cb);
  ref.current = cb;
  return <button onClick={() => ref.current()}>{n}</button>;
}
