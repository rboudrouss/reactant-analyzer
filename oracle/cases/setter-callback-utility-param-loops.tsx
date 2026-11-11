// oracle: react=loop
// #291, the utility variant of setter-callback-foreach-effect-loops.tsx: the
// setter reaches `forEach` through a utility's parameter.
import { useEffect, useState } from "react";

function each(items: number[], fn: (x: number) => void) {
  items.forEach(fn);
}

export function App() {
  const [v, setV] = useState(0);
  useEffect(() => {
    each([v + 1], setV);
  }, [v]);
  return <div>{v}</div>;
}
