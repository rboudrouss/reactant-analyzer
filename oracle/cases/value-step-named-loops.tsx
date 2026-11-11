// oracle: react=loop
// #280: the step written into a name first is the write's step: a certain loop.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const next = n + 1;
    setN(next);
  }, [n]);
  return <div>{n}</div>;
}
