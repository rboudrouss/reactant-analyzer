// oracle: react=loop
// #304: `absent || {}` over an import that is `undefined` is a new object at
// every render.
import { useEffect, useState } from "react";
import { absent } from "ext";

export function App() {
  const [s2, setS2] = useState(0);
  const obj3 = absent || { v: 0 };
  useEffect(() => {
    setS2(4 - s2);
  }, [obj3]);
  return <div>{s2}</div>;
}
