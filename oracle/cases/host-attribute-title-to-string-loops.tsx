// oracle: react=loop
// #371: React DOM sets a host element's attribute as a string
// (`setValueForAttribute`), which calls the `toString` the object holds, with
// the setter, when the element is mounted — in the render of every commit
// that mounts it — and at every commit that changes the prop.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "t";
    },
  };
  return <div title={o as unknown as string}>{n}</div>;
}
