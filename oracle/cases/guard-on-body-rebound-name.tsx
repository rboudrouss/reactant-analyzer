// oracle: react=loop
// #222. The effect body rebinds `s`: its guard reads that `null`, not the
// slot, and never dies; every run stores a new object.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState<{ n: number } | null>(null);
  useEffect(() => {
    let s = null;
    if (s === null) setS({ n: 1 });
  }, [s]);
  return <div>{s ? s.n : 0}</div>;
}
