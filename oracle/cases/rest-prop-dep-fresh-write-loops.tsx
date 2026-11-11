// oracle: react=loop
// #295: the same with a fresh object written: `[rest]` moves at every render.
import { useState, useEffect } from "react";

function C({ a, ...rest }) {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
  }, [rest]);
  return <div>{a}</div>;
}

export function App() {
  return <C a={1} x={1} />;
}
