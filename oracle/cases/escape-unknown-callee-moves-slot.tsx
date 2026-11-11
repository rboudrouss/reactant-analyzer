// oracle: react=loop
// #254: a closure handed to a callee the analyzer cannot see. Its write never
// reached the state store, so `k` stayed 0 there, `setN(n + k)` read as the
// bail-out `setN(n)`, and the loop went unclaimed.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [k, setK] = useState(0);
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(() => setK(1));
  }, []);
  useEffect(() => {
    setN(n + k);
  });
  return <div>{n}</div>;
}
