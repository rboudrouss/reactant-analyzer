// oracle: react=loop
// #312: the parent memoises the array it passes, but `items.map(f)` is still
// a new array at every render of the child, so the effect loops. Off a prop
// the analysis reads the result as one that may be new: a possible loop.
import { useEffect, useMemo, useState } from "react";

function C({ items }: { items: number[] }) {
  const [s, setS] = useState(0);
  const o = items.map((x) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

export function App() {
  const items = useMemo(() => [1, 2], []);
  return <C items={items} />;
}
