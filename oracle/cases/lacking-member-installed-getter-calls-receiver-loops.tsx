// oracle: react=loop
// #334: a getter installed on `Object.prototype` runs at a read of a member
// the literal does not hold, with the literal as `this`: it calls the
// literal's `run`, with the setter, at every render. The literal is handed
// to that getter at the read.
import { useState } from "react";

Object.defineProperty(Object.prototype, "label", {
  get() {
    return (this as { run?: () => string }).run?.();
  },
  configurable: true,
});

export function App() {
  const [n, setN] = useState(0);
  const o = {
    run: () => {
      setN(n + 1);
      return "";
    },
  };
  const s = (o as { label?: string }).label;
  return <div>{n}{s}</div>;
}
