// oracle: react=loop
// #204. The first effect reads `a` but re-runs on `b`: `a` moves without
// re-running it, and the next re-run (caused by `b`) stores the new `a`. A
// read that is not a dependency carries an edge of its own.
import { useEffect, useState } from "react";

export function App() {
  const [a, setA] = useState(false);
  const [b, setB] = useState(false);
  const [y, setY] = useState<boolean | null>(null);
  useEffect(() => {
    setY(a);
  }, [b]);
  useEffect(() => {
    setA(!a);
    setB(!b);
  }, [y]);
  return <div>{String(y)}</div>;
}
