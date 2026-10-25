// oracle: react=converges
// An effect with no deps that writes the value the slot already holds: the
// eager `Object.is` bail-out skips the render.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(0);
  });
  return <div>{n}</div>;
}
