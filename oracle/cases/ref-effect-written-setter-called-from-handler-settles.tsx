// oracle: react=converges
// #351 negative: the setter is written into the ref by a mount effect and
// called back only from a click handler.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<any>(null);
  useEffect(() => {
    r.current = setN;
  }, []);
  return <button onClick={() => r.current((c: number) => c + 1)}>{n}</button>;
}
