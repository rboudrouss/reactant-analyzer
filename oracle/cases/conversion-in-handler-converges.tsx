// oracle: react=converges
// #360: the conversion runs in a click handler, not in the render: the
// `toString` it calls writes the state once per click.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN((c) => c + 1);
      return "";
    },
  };
  return <div onClick={() => console.log(`${o}`)}>{n}</div>;
}
