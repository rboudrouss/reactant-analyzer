// oracle: react=loop
// #310: the callback's parameter `x` shadows the effect's `x`.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    [s0 + 1].forEach((x) => setS0(x));
  }, [s0]);
  return <div>{s0}</div>;
}
