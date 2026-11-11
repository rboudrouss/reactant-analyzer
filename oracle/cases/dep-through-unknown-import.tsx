// oracle: react=converges
// #224. `format` is a function of its argument, but it comes from a package
// the analyzer cannot see into, so it may be `Math.random()`: the dep may
// move at any render, a Warning React does not bear out here (fp).
import { useEffect, useState } from "react";
import { format } from "ext";

export function App() {
  const [q] = useState("a");
  const [s, setS] = useState<number[]>([]);
  useEffect(() => {
    setS([]);
  }, [format(q)]);
  return <div>{s.length}</div>;
}
