// oracle: react=loop
// A parameter holds what its body assigns it, not only its argument.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = (f: any) => {
    f = setN;
    f(n + 1);
  };
  run(() => {});
  return <div>{n}</div>;
}
