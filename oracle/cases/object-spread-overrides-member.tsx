// oracle: react=loop
// #76: a spread overrides the member written before it — `inc` is the
// spread object's closure, not the no-op.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const api = { inc: () => {}, ...{ inc: () => setN(n + 1) } };
  useEffect(() => {
    api.inc();
  });
  return <div>{n}</div>;
}
