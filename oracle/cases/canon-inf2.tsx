// oracle: react=loop
// React-tRace §3.1.2 (`Inf2`): an unconditional setter call during render throws
// "Too many re-renders" even though the value never changes.
import { useState } from "react";

export function App() {
  const [x, setX] = useState(0);
  setX(0);
  return <div>{x}</div>;
}
