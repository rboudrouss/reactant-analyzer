// oracle: react=converges
// #265: a prop function returning one value keeps the dep still.
import { useState, useEffect } from "react";

function C({ get }) {
  const [n, setN] = useState(0);
  const d = get();
  useEffect(() => {
    setN(n + 1);
  }, [d]);
  return <div>{n}</div>;
}

export function App() {
  return <C get={() => 1} />;
}
