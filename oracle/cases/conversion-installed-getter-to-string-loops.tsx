// oracle: react=loop
// #360, #334: a getter installed on `Object.prototype` under `toString` runs
// at the conversion of a literal that does not hold the member, with the
// literal as `this`: what it returns, the literal's `run`, is called, with
// the setter, at every render.
import { useState } from "react";

Object.defineProperty(Object.prototype, "toString", {
  get() {
    return (this as { run?: () => string }).run ?? (() => "x");
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
  const s = `${o}`;
  return <div>{n}{s}</div>;
}
