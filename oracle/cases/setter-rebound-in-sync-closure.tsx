// oracle: react=converges
// #235. `set` is the setter until a closure the render runs synchronously
// rebinds it to the no-op: `set(1)` writes nothing. `setter-in-render`
// certified an Error from the render body's bindings alone, where `set` has
// one binding.
import { useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const noop = (_: number) => {};
  let set: (n: number) => void = setV;
  [1].forEach(() => {
    set = noop;
  });
  set(1);
  return <div>{v}</div>;
}
