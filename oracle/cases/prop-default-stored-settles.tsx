// oracle: react=converges
// #309: the effect stores the prop the parent allocated once; a default may
// replace it. The second write stores the same array: React bails out.
import { useEffect, useState } from "react";

function C0({ p1 = 0 }) {
  const [s1, setS1] = useState<unknown>(null);
  useEffect(() => {
    setS1(p1);
  }, [s1]);
  return <div>{String(s1)}</div>;
}

export function App() {
  return (
    <div>
      <C0 p1={[]} />
    </div>
  );
}
