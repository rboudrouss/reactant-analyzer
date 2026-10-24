// oracle: react=converges
// L14 (#181): nothing loops without a click. The `infinite-loop` Warning reported
// here is a tolerated FP (P2 only constrains Errors), so the case passes; it pins
// that the Warning never becomes an Error.
import { useEffect, useState } from "react";

export function App() {
  const [a, setA] = useState(1);
  const [b, setB] = useState(0);
  useEffect(() => {
    setB(a * 2);
  }, [a]);
  return <button onClick={() => setA(a + 1)}>{b}</button>;
}
