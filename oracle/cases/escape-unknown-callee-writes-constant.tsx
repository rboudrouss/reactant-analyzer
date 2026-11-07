// oracle: react=converges
// #254, the negative: the escaped closure writes the value the slot already
// holds, so `setN(n + k)` is `setN(n)` and React bails out.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [k, setK] = useState(0);
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(() => setK(0));
  }, []);
  useEffect(() => {
    setN(n + k);
  });
  return <div>{n}</div>;
}
