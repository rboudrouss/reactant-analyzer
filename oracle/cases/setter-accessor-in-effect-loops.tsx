// oracle: react=loop
// #321. The effect on `[n]` writes `o.p`, whose setter accessor writes
// `n + 1` after every commit.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set p(v: number) {
      setN(v + 1);
    },
  };
  useEffect(() => {
    o.p = n;
  }, [n]);
  return <div>{n}</div>;
}
