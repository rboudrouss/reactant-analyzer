// oracle: react=converges
// #295: `rest.x` is the prop `x`, which the child's own re-render keeps: the
// effect runs once, whatever it writes.
import { useState, useEffect } from "react";

function C({ a, ...rest }) {
  const [n, setN] = useState(0);
  const [s, setS] = useState({});
  useEffect(() => {
    setN(n + 1);
    setS({});
  }, [rest.x]);
  return <div>{a}{n}</div>;
}

export function App() {
  return <C a={1} x={1} />;
}
