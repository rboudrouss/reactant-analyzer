// oracle: react=converges
// #309: the parent renders once, so the array it passes is allocated once:
// the child's dep holds after its first re-render, though a default may
// replace it.
import { useEffect, useState } from "react";

function C0({ p1 = 0 }) {
  const [s1, setS1] = useState(0);
  useEffect(() => {
    setS1({ v: 0 });
  }, [p1]);
  return <div>{String(s1)}</div>;
}

export function App() {
  return (
    <div>
      <C0 p1={[]} />
    </div>
  );
}
