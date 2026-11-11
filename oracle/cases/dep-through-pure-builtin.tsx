// oracle: react=converges
// #224. `JSON.stringify` computes the same string from the same object, and
// `filters` is never written: the effect runs once.
import { useEffect, useState } from "react";

export function App() {
  const [filters] = useState({ q: "a" });
  const [s, setS] = useState<number[]>([]);
  useEffect(() => {
    setS([]);
  }, [JSON.stringify(filters)]);
  return <div>{s.length}</div>;
}
