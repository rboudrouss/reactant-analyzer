// oracle: react=loop
// #265: what a prop function returns is computed at each render of the
// child: a new object at each, so the effect re-runs after every one.
import { useState, useEffect } from "react";

function C({ get }) {
  const [s, setS] = useState({});
  const d = get();
  useEffect(() => {
    setS({});
  }, [d]);
  return <div />;
}

export function App() {
  return <C get={() => ({})} />;
}
