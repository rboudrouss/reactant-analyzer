// oracle: react=loop
// #222. `d` is the slot on one arm only, and `c` never takes it: `d` stays
// `null`, the guard never dies, and every run stores a new object. A guard
// is settled by the write only on a name that reads the slot on every path.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState<{ n: number } | null>(null);
  const [c] = useState(false);
  const d = c ? s : null;
  useEffect(() => {
    if (d === null) setS({ n: 1 });
  }, [s]);
  return <div>{s ? s.n : 0}</div>;
}
