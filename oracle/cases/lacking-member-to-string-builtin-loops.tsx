// oracle: react=loop
// #340, beside the lacking member: `toString` is `Object.prototype`'s, the
// builtin, which reads the literal's `Symbol.toStringTag` — a getter here
// that writes state, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const opts = {
    a: 1,
    get [Symbol.toStringTag]() {
      setN(n + 1);
      return "Opts";
    },
  };
  const s = opts.toString();
  return (
    <div>
      {n}
      {s}
    </div>
  );
}
