// oracle: react=loop
// #372: a getter the module installs on `Object.prototype` at load is module
// code the analysis does not run. The conversion `+x` reads it off the
// literal, which lacks `valueOf`, and it calls what the module binding `hit`
// holds: the closure the render stored there, with the setter.
import { useState } from "react";

let hit: null | (() => void) = null;
Object.defineProperty(Object.prototype, "valueOf", {
  get() {
    if (hit) hit();
    return () => 1;
  },
  configurable: true,
});

export function App() {
  const [n, setN] = useState(0);
  hit = () => setN(n + 1);
  const x = { a: 1 };
  const s = +x;
  return (
    <div>
      {n}
      {s}
    </div>
  );
}
