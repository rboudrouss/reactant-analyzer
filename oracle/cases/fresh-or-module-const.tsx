// oracle: react=converges
// #304: a module const the analysis sees is an object, never nullish, so
// `D ?? {}` is `D` at every render and the effect runs once.
import { useEffect, useState } from "react";

const D = { v: 1 };

export function App() {
  const [s2, setS2] = useState(0);
  const obj3 = D ?? { v: 0 };
  useEffect(() => {
    setS2(4 - s2);
  }, [obj3]);
  return <div>{s2}</div>;
}
