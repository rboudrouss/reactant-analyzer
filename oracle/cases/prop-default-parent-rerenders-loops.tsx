// oracle: react=loop
// #309: the parent re-renders at every commit (its own counter), so the
// array it passes is new each time.
import { useEffect, useState } from "react";

function C0({ p1 = 0 }) {
  const [s1, setS1] = useState(0);
  useEffect(() => {
    setS1({ v: 0 });
  }, [p1]);
  return <div>{String(s1)}</div>;
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  });
  return (
    <div>
      <C0 p1={[]} />
    </div>
  );
}
