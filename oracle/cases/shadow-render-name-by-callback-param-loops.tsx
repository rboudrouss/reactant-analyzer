// oracle: react=loop
// #310: the callback's parameter `x` shadows the render's `x`, which the
// effect's env holds.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  const x = 5;
  useEffect(() => {
    [s0 + 1].forEach((x) => setS0(x));
  }, [s0, x]);
  return <div>{s0}</div>;
}
