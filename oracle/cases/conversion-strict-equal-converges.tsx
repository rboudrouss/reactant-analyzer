// oracle: react=converges
// #360: `===` converts nothing, nor does `== null`, `typeof`, `!`, `??` or a
// condition: the `valueOf` and `toString` the literal holds never run.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = {
    valueOf: () => {
      h();
      return 1;
    },
    toString: () => {
      h();
      return "";
    },
  };
  const a = (o as unknown) === 1;
  const b = o == null || typeof o === "string" || !o;
  const c = o ?? 1;
  if (o) {
    void c;
  }
  return <div>{n}{a || b ? "a" : "b"}</div>;
}
