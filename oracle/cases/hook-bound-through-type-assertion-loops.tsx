// oracle: react=loop
// #383. A hook bound through parentheses and a type assertion,
// `const useX = (() => …) as () => number`: the assertion changes no value,
// the hook is the arrow, and its effect loops.
import { useState, useEffect } from "react";

const useX = (() => {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return n;
}) as () => number;

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
