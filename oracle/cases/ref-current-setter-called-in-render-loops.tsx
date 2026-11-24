// oracle: react=loop
// #351: a setter written into a ref's `current` and called back through it in
// the render. React keeps a setter's identity across renders, so the ref's
// member keeps the setter it holds, as a literal's member does.
import { useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<any>(null);
  r.current = setN;
  r.current((c: number) => c + 1);
  return <div>{n}</div>;
}
