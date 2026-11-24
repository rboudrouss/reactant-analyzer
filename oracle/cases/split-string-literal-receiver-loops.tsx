// oracle: react=loop
// #312: a string's `split` is a new array at every render.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const o = "a,b".split(",");
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
