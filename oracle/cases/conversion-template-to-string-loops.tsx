// oracle: react=loop
// #360: a template literal converts its substitution to a string, which calls
// the `toString` the literal holds, with the setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = {
    toString: () => {
      h();
      return "";
    },
  };
  const s = `${o}`;
  return <div>{n}{s}</div>;
}
