// oracle: react=loop
// #310: the IIFE's parameter `x` is the argument it is called with.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    ((x: number) => setS0(x))(s0 + 1);
  }, [s0]);
  return <div>{s0}</div>;
}
