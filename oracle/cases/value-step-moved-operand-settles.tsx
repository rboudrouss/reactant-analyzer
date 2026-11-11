// oracle: react=converges
// #280: `next` is `n + k` with `k` still 0; `k` moves after, so the write stores `n` and React bails out.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    let k = 0;
    const next = n + k;
    k = 1;
    setN(next);
  }, [n]);
  return <div>{n}</div>;
}
