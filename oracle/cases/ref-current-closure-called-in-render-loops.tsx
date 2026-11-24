// oracle: react=loop
// #351, the neighbour reported before the fix: a closure written into a ref's
// `current` and called back through it in the render runs the setter.
import { useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<any>(null);
  r.current = () => setN((c: number) => c + 1);
  r.current();
  return <div>{n}</div>;
}
