// oracle: react=loop
// #307: an index read directly off an array literal is the element the
// literal holds there. `absent` is `undefined`, so `[absent ?? {}][0]` is a
// new object at every render, with nothing it is computed from moving: the
// effect re-runs after each `s2` write.
import { useEffect, useState } from "react";
import { absent } from "ext";

export function App() {
  const [s2, setS2] = useState(0);
  const obj3 = [absent ?? { v: 0 }][0];
  useEffect(() => {
    setS2(4 - s2);
  }, [obj3]);
  return <div>{s2}</div>;
}
