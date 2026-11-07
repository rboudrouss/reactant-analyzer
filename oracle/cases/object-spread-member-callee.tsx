// oracle: react=loop
// #76: `{ ...base, x: 1 }` holds `base`'s members, so `api.inc` is the closure
// `base` holds. The spread was kept for its reads only, and the call ran
// nothing.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const base = { inc: () => setN(n + 1) };
  const api = { ...base, x: 1 };
  useEffect(() => {
    api.inc();
  });
  return <div>{n}</div>;
}
