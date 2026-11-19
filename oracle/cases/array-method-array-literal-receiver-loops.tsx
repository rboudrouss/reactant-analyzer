// oracle: react=loop
// #312: off an array literal `map` is the language's, a new array at every
// render: the effect re-runs after each counter write.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const o = [1, 2].map((x: number) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
