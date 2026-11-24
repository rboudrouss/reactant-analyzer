// oracle: react=converges
// #372: the getter the module installs on `Object.prototype` runs at the
// conversion and at the read, as module code, but no module binding holds
// anything of the render, and the literal holds no closure: the setter the
// handler holds runs only on a click.
import { useState } from "react";

let count = 0;
Object.defineProperty(Object.prototype, "valueOf", {
  get() {
    count += 1;
    return () => 1;
  },
  configurable: true,
});

export function App() {
  const [n, setN] = useState(0);
  const x = { a: 1 };
  const s = +x;
  const t = typeof x.valueOf;
  return (
    <div onClick={() => setN(n + 1)}>
      {n}
      {s}
      {t}
      {count > 0 ? "y" : "n"}
    </div>
  );
}
