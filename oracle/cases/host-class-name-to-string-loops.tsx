// oracle: react=loop
// #371: React DOM sets `className` as the `class` attribute
// (`setValueForKnownAttribute`), a string, which calls the `toString` the
// object holds, with the setter.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "c";
    },
  };
  return <div className={o as unknown as string}>{n}</div>;
}
