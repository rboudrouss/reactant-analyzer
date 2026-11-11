// oracle: react=loop
// #76: a computed key no literal spells may be any member, so `api.inc` may
// be the closure stored under `[k]`.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const k = "inc";
  const api = { [k]: () => setN(n + 1) };
  useEffect(() => {
    api.inc();
  });
  return <div>{n}</div>;
}
