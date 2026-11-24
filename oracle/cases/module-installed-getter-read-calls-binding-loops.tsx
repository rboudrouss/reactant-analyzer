// oracle: react=loop
// #372: a plain read of a member the literal lacks runs the getter the module
// installed on `Object.prototype` at load — module code, which calls what the
// module binding `hit` holds: the closure the render stored there, with the
// setter.
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
  const s = typeof x.valueOf;
  return (
    <div>
      {n}
      {s}
    </div>
  );
}
