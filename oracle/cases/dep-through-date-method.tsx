// oracle: react=converges
// #224. A `Date` built from a held string is a new object at every render,
// but its `getTime()` is the same number: the effect runs once.
import { useEffect, useState } from "react";

export function App() {
  const [iso] = useState("2020-01-01T00:00:00Z");
  const [s, setS] = useState<number[]>([]);
  const date = new Date(iso);
  useEffect(() => {
    setS([]);
  }, [date.getTime()]);
  return <div>{s.length}</div>;
}
