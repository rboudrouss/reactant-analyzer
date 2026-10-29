// oracle: react=converges
// #76, the precision side: the spread of a known object overrides `inc` with
// a no-op, so the effect writes nothing.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const api = { inc: () => setN(n + 1), ...{ inc: () => {} } };
  useEffect(() => {
    api.inc();
  });
  return <div>{n}</div>;
}
