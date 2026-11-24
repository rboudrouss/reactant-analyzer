// oracle: react=loop
// #360: `JSON.stringify` calls the `toJSON` its argument holds; the argument
// of a callee the analysis does not run is handed to it, its closures with
// it: the setter runs at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = {
    toJSON: () => {
      h();
      return 1;
    },
  };
  const s = JSON.stringify(o);
  return <div>{n}{s}</div>;
}
