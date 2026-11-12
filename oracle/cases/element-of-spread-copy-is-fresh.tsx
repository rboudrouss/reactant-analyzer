// oracle: react=loop
// #307: an element a spread copies may sit at any position of the new array.
// `[...a][0]` is the `{ v: 0 }` that `a` allocates at every render.
import { useEffect, useState } from "react";

export function App() {
  const [s2, setS2] = useState(0);
  const a = [{ v: 0 }];
  const arr = [...a];
  const obj3 = arr[0];
  useEffect(() => {
    setS2(4 - s2);
  }, [obj3]);
  return <div>{s2}</div>;
}
