// oracle: react=loop
// #321. The getter's read bound to a name first: `d` is a new object at
// every render.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = { get p() { return {}; } };
  const d = o.p;
  useEffect(() => {
    setN((c) => c + 1);
  }, [d]);
  return <div>{n}</div>;
}
