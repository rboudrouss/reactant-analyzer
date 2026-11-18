// oracle: react=loop
// #324: `arguments` holds every argument of the call of a function.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = function (_f: any) {
    arguments[0](n + 1);
  };
  run(setN);
  return <div>{n}</div>;
}
