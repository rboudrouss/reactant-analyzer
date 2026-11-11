// oracle: react=converges
// #224. A `URLSearchParams` built from a held string answers `get` the same
// way at every render: the effect runs once.
import { useEffect, useState } from "react";

export function App() {
  const [search] = useState("q=a");
  const [s, setS] = useState<number[]>([]);
  const searchParams = new URLSearchParams(search);
  useEffect(() => {
    setS([]);
  }, [searchParams.get("q")]);
  return <div>{s.length}</div>;
}
