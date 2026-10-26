// oracle: react=loop
// L7: the splice dropped the expression of `return set(..)` when the call was
// not bound (#174).
import { useEffect, useState } from "react";

function bump(set: (n: number) => void, n: number) {
  return set(n + 1);
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    bump(setN, n);
  });
  return <div>{n}</div>;
}
