// oracle: react=converges
// #327. `s` is `setA`, then `0`: the call is of a number, which throws, and
// no slot is written. The name keeps no setter label from its first binding.
import { useEffect, useState } from "react";

export function App() {
  const [a, setA] = useState(0);
  useEffect(() => {
    let s: any = setA;
    s = 0;
    try {
      s(a + 1);
    } catch {}
  }, [a]);
  return <div>{a}</div>;
}
