// oracle: react=loop
// #264: the closures are members of an object literal handed to a hook the
// analyzer cannot inline, which calls them in an effect. The writer walk
// entered a function argument, not one inside an argument object: no row,
// no claim.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useRun({ a: () => setN(n + 1), b: () => setN(n + 1) });
  return <div>{n}</div>;
}
