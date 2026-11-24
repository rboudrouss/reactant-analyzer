// oracle: react=loop
// #360: unary `+` converts its operand to a number, which calls the
// `valueOf` the literal holds, with the setter, at every render.
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
  const s = +o;
  return <div>{n}{s}</div>;
}
