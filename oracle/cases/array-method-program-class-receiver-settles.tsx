// oracle: react=converges
// #312: `None.map(f)` is the program's own method, which returns `this`: the
// same object at every render, so the effect runs once. A receiver the
// analysis does not know to be an array may have any `map`: the result may be
// new, a possible loop at most, never an Error.
import { useEffect, useState } from "react";

class NoneImpl {
  none = true;
  map(_f: unknown) {
    return this;
  }
}
const None = new NoneImpl();

export function App() {
  const [s, setS] = useState(0);
  const o = None.map((x: number) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
