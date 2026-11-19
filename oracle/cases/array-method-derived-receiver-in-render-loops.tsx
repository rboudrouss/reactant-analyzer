// oracle: react=loop
// A receiver the walk cannot list holds whatever flows into it.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [setN].flatMap((s) => [s]).forEach((set) => set(n + 1));
  return <div>{n}</div>;
}
