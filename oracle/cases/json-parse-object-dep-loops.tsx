// oracle: react=loop
// #306: `JSON.parse("{}")` builds a new object at every render, so the effect
// re-runs after each counter write.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const o = JSON.parse("{}");
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
