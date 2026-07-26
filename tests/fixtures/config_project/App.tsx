import { useState, useEffect } from "react";

// Two known findings, one per severity:
// - setter-in-render (Error): unconditional setter call during render.
// - missing-deps (Warning): `n` (written state) captured by the effect,
//   not declared in its deps array.
export function App() {
  const [n, setN] = useState(0);
  setN(1);
  useEffect(() => {
    console.log(n);
  }, []);
  return <div>{n}</div>;
}
