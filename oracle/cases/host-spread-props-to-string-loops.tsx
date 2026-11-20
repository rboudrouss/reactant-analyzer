// oracle: react=loop
// #371: a spread of props hands React DOM every member as a prop: `title` is
// set as a string attribute, and `style`'s members one by one, each of which
// calls the `toString` its object holds, with the setter.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "1px";
    },
  };
  const rest = { id: "x", style: { width: o as unknown as string } };
  return <div {...rest}>{n}</div>;
}
