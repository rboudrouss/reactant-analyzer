// oracle: react=loop
// #269, the twin whose guard the write does not kill: `n + 1` keeps
// `n < 100` live past React's render-phase update limit.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [1].forEach(() => {
    if (n < 100) setN(n + 1);
  });
  return <div>{n}</div>;
}
