// oracle: react=loop
// #198: the memo is the number 0, so `!zero` holds and the effect bumps `n`
// at every run. A memo read as an object was killed by the guard.
import { useEffect, useMemo, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const zero = useMemo(() => 0, []);
  useEffect(() => {
    if (!zero) setN(n + zero + 1);
  }, [n, zero]);
  return <div>{n}</div>;
}
