// oracle: react=loop
// #360: `o == 1` compares an object with a number: the object converts to a
// primitive, which calls its `valueOf`, with the setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = {
    valueOf: () => {
      h();
      return 1;
    },
  };
  const s = (o as unknown) == 1;
  return <div>{n}{s ? "a" : "b"}</div>;
}
