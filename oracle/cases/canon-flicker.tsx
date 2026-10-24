// oracle: react=converges
// `Flicker`: a mount-only effect costs one extra render, then stops.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  useEffect(() => {
    setS(42);
  }, []);
  return <div>{s}</div>;
}
