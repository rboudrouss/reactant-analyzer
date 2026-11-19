// oracle: react=loop
// What a callback returns flows into the receiver it builds: the setter it
// returns is an entry the next callback calls.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [0].flatMap(() => [setN]).forEach((set) => set(n + 1));
  return <div>{n}</div>;
}
