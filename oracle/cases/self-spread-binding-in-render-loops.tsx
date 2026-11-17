// oracle: react=loop
// A name bound to a spread of itself still holds the setter it was given.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = (f: () => void) => {
    let acc: Array<() => void> = [];
    acc = [...acc, f];
    acc.forEach((g) => g());
  };
  run(() => setN(n + 1));
  return <div>{n}</div>;
}
