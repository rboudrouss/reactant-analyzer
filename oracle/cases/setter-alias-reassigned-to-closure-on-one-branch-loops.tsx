// oracle: react=loop
// #327. `s` is `setA`, or a closure when `c` holds. The join may be either:
// the call is no certain setter, so the closure runs as well, and its write
// arms the growing effect on `n`.
import { useEffect, useState } from "react";

export function App({ c = true }: { c?: boolean }) {
  const [a, setA] = useState(0);
  const [n, setN] = useState(0);
  useEffect(() => {
    let s = setA;
    if (c) s = (v: number) => setN(n + v);
    s(1);
  }, [n]);
  return (
    <div>
      {a}
      {n}
    </div>
  );
}
