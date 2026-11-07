// oracle: react=loop
// #264: the closure is an element of an array literal handed to a hook the
// analyzer cannot inline, which calls it in an effect; the walk did not
// enter it.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useRun([() => setN(n + 1)]);
  return <div>{n}</div>;
}
