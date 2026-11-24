// oracle: react=converges
// #351 negative: the setter kept in the ref is only called from a click
// handler — a user event stands between each render and the write.
import { useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<any>(null);
  r.current = setN;
  return <button onClick={() => r.current((c: number) => c + 1)}>{n}</button>;
}
