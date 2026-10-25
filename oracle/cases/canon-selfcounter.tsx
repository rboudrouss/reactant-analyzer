// oracle: react=converges
// React-tRace (`SelfCounter`): the same effect under a guard settles after four renders.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  useEffect(() => {
    if (s < 3) setS(s + 1);
  });
  return <div>{s}</div>;
}
