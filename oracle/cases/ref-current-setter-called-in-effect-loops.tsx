// oracle: react=loop
// #351: a setter written into a ref's `current` in the render, called back
// through it by an effect whose dep it moves.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<any>(null);
  r.current = setN;
  useEffect(() => {
    r.current(n + 1);
  }, [n]);
  return <div>{n}</div>;
}
