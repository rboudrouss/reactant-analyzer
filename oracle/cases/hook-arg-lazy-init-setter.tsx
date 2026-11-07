// oracle: react=converges
// A lazy initializer runs at the mount render only (#193): its setter call is a
// render-phase write once, React settles on the second pass. The analyzer sees
// the write (a Warning); an Error would claim it runs at every render.
import { useState } from "react";

export function App() {
  const [a, setA] = useState(0);
  const [b] = useState(() => {
    setA(a + 1);
    return 0;
  });
  return <div>{a}{b}</div>;
}
