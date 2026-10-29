// oracle: react=loop
// #254: a closure handed to a hook the analyzer cannot inline, which calls it
// in an effect. The write reached no pass, so `k` stayed 0 in the store.
import { useEffect, useState } from "react";
import { useRun } from "ext";

export function App() {
  const [k, setK] = useState(0);
  const [n, setN] = useState(0);
  useRun(() => setK(1));
  useEffect(() => {
    setN(n + k);
  });
  return <div>{n}</div>;
}
