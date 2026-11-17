// oracle: react=loop
// An omitted argument is the parameter's default.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = (f = setN) => f(n + 1);
  run();
  return <div>{n}</div>;
}
