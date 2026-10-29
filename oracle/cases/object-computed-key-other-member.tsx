// oracle: react=converges
// #76, the open side: the computed key names another member at run time, so
// `inc` stays the no-op; the analysis cannot tell and may warn, never
// certify.
import { useEffect, useState } from "react";
import { format } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const api = { inc: () => {}, [format("k")]: () => setN(n + 1) };
  useEffect(() => {
    api.inc();
  });
  return <div>{n}</div>;
}
