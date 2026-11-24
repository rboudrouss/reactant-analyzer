// oracle: react=loop
// #371: React DOM writes a `style` object member by member
// (`setValueForStyle`: `("" + value).trim()`), which calls the `toString` the
// member's object holds, with the setter.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "1px";
    },
  };
  return <div style={{ width: o as unknown as string }}>{n}</div>;
}
