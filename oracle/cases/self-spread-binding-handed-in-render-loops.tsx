// oracle: react=loop
// A name bound to a spread of itself, handed to a callee that calls it.
import { useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const run = (f: () => void) => {
    let hs: Array<() => void> = [f];
    hs = [...hs];
    runNow(...hs);
  };
  run(() => setN(n + 1));
  return <div>{n}</div>;
}
