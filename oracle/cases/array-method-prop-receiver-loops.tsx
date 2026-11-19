// oracle: react=loop
// #312: `items.map(f)` off a prop is a new array at every render of the
// child, whatever the parent passes, so the effect loops.
import { useEffect, useState } from "react";

function C({ items }: { items: number[] }) {
  const [s, setS] = useState(0);
  const o = items.map((x) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

export function App() {
  return <C items={[1, 2]} />;
}
