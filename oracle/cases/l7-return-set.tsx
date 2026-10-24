// oracle: react=loop
// oracle: known=fn #174
// L7: the splice drops the expression of `return set(..)` when the call is not bound.
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
