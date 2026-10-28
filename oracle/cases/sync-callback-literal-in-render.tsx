// oracle: react=loop
// #248, the certain side: a `forEach` over an array literal with one entry
// runs its callback on every render, so the write is a certain render write
// (an Error), placed at the call.
import { useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  [1].forEach(() => setV(v + 1));
  return <div>{v}</div>;
}
