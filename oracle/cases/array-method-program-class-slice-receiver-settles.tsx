// oracle: react=converges
// #312: an object, not an array, whose `slice` returns `this`: `q.slice()` is
// the same object at every render, so the effect runs once.
import { useEffect, useState } from "react";

class Q {
  v = 1;
  slice() {
    return this;
  }
}
const q = new Q();

export function App() {
  const [s, setS] = useState(0);
  const o = q.slice();
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
