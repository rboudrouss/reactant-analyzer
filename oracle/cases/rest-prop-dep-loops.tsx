// oracle: react=loop
// #295: `...rest` is a new object at every render, so an effect on `[rest]`
// re-runs after each one, and its own write re-renders it.
import { useState, useEffect } from "react";

function C({ a, ...rest }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [rest]);
  return <div>{a}{n}</div>;
}

export function App() {
  return <C a={1} x={1} />;
}
