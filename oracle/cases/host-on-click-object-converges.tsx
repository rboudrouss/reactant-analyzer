// oracle: react=converges
// #371: React DOM never converts an event prop (`setProp`: `onClick` and
// every `on*` name only register a listener), so the `toString` of an object
// passed there never runs.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "x";
    },
  };
  return <div onClick={o as unknown as () => void}>{n}</div>;
}
